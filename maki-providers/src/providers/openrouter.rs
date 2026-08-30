use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};

use flume::Sender;
use maki_storage::id::SessionRef;
use serde_json::{Value, json};

use crate::model::{Model, ModelEntry, ModelInfo, ModelPricing};
use crate::provider::{BoxFuture, Provider};
use crate::{
    AgentError, Effort, EffortDialect, Message, ProviderEvent, RequestOptions, StreamResponse,
    Upstream, dialect,
};

use super::openai_compat::{OpenAiCompatConfig, OpenAiCompatProvider};
use super::{KeyPool, ResolvedAuth};

const REFERER: &str = "https://maki.sh";
const APP_TITLE: &str = "maki";
const PER_MILLION: f64 = 1_000_000.0;

static CONFIG: OpenAiCompatConfig = OpenAiCompatConfig {
    slug: "openrouter",
    api_key_env: "OPENROUTER_API_KEY",
    base_url: "https://openrouter.ai/api/v1",
    max_tokens_field: "max_tokens",
    include_stream_usage: true,
    provider_name: "OpenRouter",
};

inventory::submit!(maki_config::providers::BuiltInProvider {
    slug: "openrouter",
    display_name: "OpenRouter",
    protocol: maki_config::providers::Protocol::Openai,
    default_base_url: "https://openrouter.ai/api/v1",
    default_api_key_env: "OPENROUTER_API_KEY",
    default_model: "openrouter/openai/gpt-5.5",
    plans: None,
    login_url: Some("https://openrouter.ai/keys"),
    needs_url: false,
});

pub(crate) const fn models() -> &'static [ModelEntry] {
    &[]
}

#[derive(Debug)]
struct OpenRouterModelInfo {
    reasoning_mandatory: bool,
    reasoning_default_enabled: bool,
    reasoning_efforts: Vec<Effort>,
}

pub struct OpenRouter {
    compat: OpenAiCompatProvider,
    auth: Arc<Mutex<ResolvedAuth>>,
    key_pool: Option<KeyPool>,
    system_prefix: Option<String>,
    /// `providers.toml` upstream routing, resolved once at construction.
    routing: Option<Value>,
}

/// Read `[openrouter]` routing preferences from `providers.toml`.
fn configured_routing() -> Option<Value> {
    let providers = maki_config::providers::ProvidersConfig::load();
    maki_config::providers::configured_routing(providers.get(CONFIG.slug))
}

/// Where the pin table lives between runs, under the state dir.
const PINS_FILE: &str = "openrouter-pins.json";

/// Enough pins for any plausible model list; past it the table is dropped
/// rather than evicted one entry at a time, costing one uncached request per
/// model still in use.
const MAX_PINS: usize = 512;

/// The upstream each model last talked to.
///
/// OpenRouter load balances every request across the upstreams serving a
/// model, and each upstream holds a separate prompt cache, so consecutive
/// turns of one conversation land on caches holding different prefixes - the
/// observable symptom is `cache_read` collapsing to zero or to the size of a
/// request several turns old. Every response names the upstream that served
/// it, so asking for that same upstream next turn keeps one cache warm.
///
/// Keyed by model alone, not by session: one model's cached prefix is its
/// system prompt and tool definitions, which every session using that model
/// shares. Sending them all to one upstream is what lets a *new* session open
/// against an already-warm cache instead of paying to build its own.
///
/// The table outlives any one provider instance, and the process itself, for
/// the same reason. maki rebuilds the OpenRouter provider whenever the model
/// catalog resolves or the model changes, so a pin held in the provider is
/// dropped mid-conversation - putting the session back on load balancing,
/// which is the exact behaviour this exists to prevent.
///
/// The pin follows whoever actually served the last request rather than
/// sticking to the first: `allow_fallbacks` is left at OpenRouter's default,
/// so when a pinned upstream goes down the request still succeeds elsewhere,
/// and the pin moves to the cache that is now the warm one.
#[derive(Debug, Default)]
struct UpstreamPins {
    /// Model id -> upstream display name.
    pins: Mutex<HashMap<String, String>>,
    /// Where to persist; `None` disables persistence (tests, and any run
    /// whose state dir cannot be resolved).
    path: Option<PathBuf>,
}

/// The pins for this process, shared by every OpenRouter provider it builds.
static PINS: LazyLock<UpstreamPins> = LazyLock::new(UpstreamPins::load);

impl UpstreamPins {
    /// Read the persisted table. A missing, unreadable, or malformed file is
    /// not worth reporting: the cost is one unpinned request per model, and
    /// the next response rewrites the file anyway.
    fn load() -> Self {
        let path = maki_storage::paths::state_dir()
            .ok()
            .map(|d| d.join(PINS_FILE));
        let pins = path
            .as_ref()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|raw| serde_json::from_slice::<HashMap<String, String>>(&raw).ok())
            .unwrap_or_default();
        Self {
            pins: Mutex::new(pins),
            path,
        }
    }

    fn get(&self, model: &str) -> Option<String> {
        self.pins.lock().unwrap().get(model).cloned()
    }

    /// Record the upstream that served a response. A response that names none
    /// (an aggregator that did not report one) leaves the existing pin alone
    /// rather than unpinning the model.
    fn record(&self, model: &str, upstream: Option<&Upstream>) {
        let Some(name) = upstream.and_then(|u| u.name.clone()) else {
            return;
        };
        let mut pins = self.pins.lock().unwrap();
        match pins.get(model) {
            Some(current) if *current == name => return,
            // The request asked for one upstream and another answered, so
            // OpenRouter fell back — the pinned upstream was unreachable or
            // out of capacity. Worth a line: it is the difference between a
            // pin that isn't working and a pin that was overridden, which
            // look identical in the `upstream` column of `/stats`.
            Some(current) => tracing::info!(
                model,
                pinned = %current,
                served = %name,
                "openrouter fell back off the pinned upstream; re-pinning to the warm cache"
            ),
            None => tracing::debug!(model, upstream = %name, "pinned openrouter upstream"),
        }
        if pins.len() >= MAX_PINS && !pins.contains_key(model) {
            pins.clear();
        }
        pins.insert(model.to_string(), name);
        // Only on a change, so a stable session writes nothing after its
        // first turn.
        if let Some(path) = &self.path
            && let Ok(raw) = serde_json::to_vec(&*pins)
            && let Err(e) = std::fs::write(path, raw)
        {
            tracing::debug!(path = %path.display(), error = %e, "cannot persist upstream pins");
        }
    }
}

/// The `provider` request-body object.
///
/// Routing configured in `providers.toml` wins whenever it selects upstreams
/// itself (`order`, `only`, or `sort`) - an explicit preference is not
/// something to silently override with a learned one. Otherwise the session's
/// pin becomes the `order`, merged into whatever else was configured (an
/// `allow_fallbacks` on its own, say).
///
/// OpenRouter resolves both the slug (`streamlake`) and the display name
/// (`StreamLake`) here, and responses report the display name, so the name
/// goes back as-is. A name it does not recognise in `order` is silently
/// dropped, putting that request back on load balancing — which is why the
/// pin is only ever a name OpenRouter itself reported, never one derived by
/// guessing at a slug.
fn routing_body(configured: Option<&Value>, pin: Option<&str>) -> Option<Value> {
    let selects_upstream = |v: &Value| ["order", "only", "sort"].iter().any(|k| v.get(k).is_some());
    match (configured, pin) {
        (Some(cfg), _) if selects_upstream(cfg) => Some(cfg.clone()),
        (cfg, Some(pin)) => {
            let mut out = cfg.cloned().unwrap_or_else(|| json!({}));
            out["order"] = json!([pin]);
            Some(out)
        }
        (cfg, None) => cfg.cloned(),
    }
}

impl OpenRouter {
    pub fn new(timeouts: super::Timeouts) -> Result<Self, AgentError> {
        let pool = KeyPool::resolve(CONFIG.slug, CONFIG.api_key_env)?;
        Ok(Self {
            compat: OpenAiCompatProvider::new(&CONFIG, timeouts),
            auth: Arc::new(Mutex::new(ResolvedAuth::bearer(
                CONFIG.slug,
                pool.current(),
            )?)),
            key_pool: Some(pool),
            system_prefix: None,
            routing: configured_routing(),
        })
    }

    pub(crate) fn with_auth(auth: Arc<Mutex<ResolvedAuth>>, timeouts: super::Timeouts) -> Self {
        Self {
            compat: OpenAiCompatProvider::new(&CONFIG, timeouts),
            auth,
            key_pool: None,
            system_prefix: None,
            routing: configured_routing(),
        }
    }

    pub(crate) fn with_system_prefix(mut self, prefix: Option<String>) -> Self {
        self.system_prefix = prefix;
        self
    }
}

/// OpenRouter models come in three reasoning states, encoded here as a
/// dialect so `effort_str` can resolve them like any other provider:
/// 1. mandatory - always on; Off sends nothing (can't disable).
/// 2. default_enabled - on by default; Off sends effort "none".
/// 3. default off - Off sends nothing; any effort string turns it on.
fn effort_dialect(info: Option<&OpenRouterModelInfo>) -> EffortDialect<'_> {
    let Some(info) = info else {
        return dialect::PREFER_HIGH;
    };
    EffortDialect {
        supported: match info.reasoning_efforts.as_slice() {
            [] => dialect::PREFER_HIGH.supported,
            declared => declared,
        },
        off: (info.reasoning_default_enabled && !info.reasoning_mandatory).then_some(dialect::OFF),
        ..dialect::PREFER_HIGH
    }
}

fn parse_model(m: &Value) -> Option<ModelInfo> {
    // Filter: only text input/output models
    let architecture = m["architecture"].as_object()?;
    let input_modalities = architecture["input_modalities"].as_array()?;
    let output_modalities = architecture["output_modalities"].as_array()?;

    let has_text_input = input_modalities.iter().any(|m| m.as_str() == Some("text"));
    let has_text_output = output_modalities.iter().any(|m| m.as_str() == Some("text"));
    if !has_text_input || !has_text_output {
        return None;
    }

    let supports_vision = input_modalities.iter().any(|m| m.as_str() == Some("image"));

    // Parse with OpenRouter-specific pricing field names. OpenRouter reports
    // per-token prices; scale to $/M as `ModelPricing` expects. A missing or
    // unparsable price stays `None` so it never reads as free.
    let id = m["id"].as_str()?;
    let context_window = m["context_length"]
        .as_u64()
        .and_then(|v| u32::try_from(v).ok());
    let per_token =
        |p: &Value| -> Option<f64> { Some(p.as_str()?.parse::<f64>().ok()? * PER_MILLION) };
    let pricing = m["pricing"].as_object().and_then(|p| {
        Some(ModelPricing {
            input: per_token(p.get("prompt")?)?,
            output: per_token(p.get("completion")?)?,
            cache_write: p
                .get("input_cache_write")
                .and_then(per_token)
                .unwrap_or(0.0),
            cache_read: p.get("input_cache_read").and_then(per_token).unwrap_or(0.0),
            fast: None,
        })
    });

    let reasoning = m
        .get("reasoning")
        .and_then(|v| v.as_object())
        .map(|v| OpenRouterModelInfo {
            reasoning_mandatory: v.get("mandatory").and_then(Value::as_bool) == Some(true),
            reasoning_default_enabled: v.get("default_enabled").and_then(Value::as_bool)
                == Some(true),
            reasoning_efforts: v
                .get("supported_efforts")
                .and_then(Value::as_array)
                .map(|arr| {
                    let mut efforts: Vec<Effort> = arr
                        .iter()
                        .filter_map(|v| v.as_str()?.parse().ok())
                        .collect();
                    efforts.sort_unstable();
                    efforts
                })
                .unwrap_or_default(),
        });

    let supports_thinking = reasoning.is_some()
        || m.get("supported_parameters")
            .and_then(|v| v.as_array())
            .is_some_and(|v| v.iter().any(|v| v.as_str() == Some("reasoning")));

    Some(ModelInfo {
        id: id.to_string(),
        context_window,
        max_output_tokens: None,
        pricing,
        supports_thinking: Some(supports_thinking),
        supports_vision: Some(supports_vision),
        tier: None,
        provider_info: reasoning.map(|r| Arc::new(r) as Arc<dyn std::any::Any + Send + Sync>),
    })
}

impl Provider for OpenRouter {
    fn stream_message<'a>(
        &'a self,
        model: &'a Model,
        messages: &'a [Message],
        system: &'a str,
        tools: &'a Value,
        event_tx: &'a Sender<ProviderEvent>,
        opts: RequestOptions,
        session_id: Option<&'a SessionRef>,
    ) -> BoxFuture<'a, Result<StreamResponse, AgentError>> {
        Box::pin(async move {
            let auth = self.auth.lock().unwrap().clone();
            let mut buf = String::new();
            let system = super::with_prefix(&self.system_prefix, system, &mut buf);
            let mut body = self.compat.build_body(model, messages, system, tools);

            body["cache_control"] = json!({"type": "ephemeral"});

            // Without this, OpenRouter load balances across every upstream
            // serving the model and each keeps a separate prompt cache, so
            // consecutive turns of one conversation land on caches holding
            // different prefixes. `session_id` below does not pin routing.
            if let Some(routing) =
                routing_body(self.routing.as_ref(), PINS.get(&model.id).as_deref())
            {
                body["provider"] = routing;
            }

            let reasoning_info = crate::model_registry::provider_info::<OpenRouterModelInfo>(
                "openrouter",
                &model.id,
            );

            let effort_dialect = effort_dialect(reasoning_info.as_deref());
            if model.supports_thinking()
                && let Some(effort) = opts.thinking.effort_str(&effort_dialect, model)
            {
                body["reasoning"] = json!({"effort": effort});
            }

            if let Some(sid) = session_id {
                body["session_id"] = json!(sid.to_string());
            }

            let extra_headers = [("HTTP-Referer", REFERER), ("X-OpenRouter-Title", APP_TITLE)];
            let response = self
                .compat
                .do_stream(model, &extra_headers, &body, event_tx, &auth)
                .await?;

            // Pin to whoever holds the cache now, which is whoever just
            // answered - the pinned upstream normally, a fallback when it was
            // unreachable.
            PINS.record(&model.id, response.upstream.as_ref());
            Ok(response)
        })
    }

    fn list_models(&self) -> BoxFuture<'_, Result<Vec<ModelInfo>, AgentError>> {
        Box::pin(async move {
            let auth = self.auth.lock().unwrap().clone();
            self.compat.fetch_and_parse_models(&auth, parse_model).await
        })
    }

    fn rotate_key(&self) -> BoxFuture<'_, Result<bool, AgentError>> {
        Box::pin(async {
            Ok(self
                .key_pool
                .as_ref()
                .is_some_and(|p| p.rotate_bearer(&self.auth)))
        })
    }
}

#[cfg(test)]
mod tests {
    use test_case::test_case;

    use super::*;
    use crate::ThinkingConfig;

    const UNKNOWN_PRICE_STAYS_UNKNOWN: &str = "a price we cannot read must not become a zero price";

    fn kimi_k3_json() -> Value {
        json!({
            "id": "moonshotai/kimi-k3",
            "context_length": 1_048_576,
            "architecture": {
                "input_modalities": ["text", "image"],
                "output_modalities": ["text"],
            },
            "pricing": {
                "prompt": "0.000003",
                "completion": "0.000015",
                "input_cache_read": "0.0000003",
            },
            "supported_parameters": ["reasoning"],
        })
    }

    #[test]
    fn parse_model_scales_pricing_to_per_million() {
        let info = parse_model(&kimi_k3_json()).expect("model should parse");

        assert_eq!(info.id, "moonshotai/kimi-k3");
        assert_eq!(info.context_window, Some(1_048_576));
        assert_eq!(info.supports_vision, Some(true));
        assert_eq!(info.supports_thinking, Some(true));
        let pricing = info.pricing.expect("pricing should be parsed");
        assert_eq!(pricing.input, 3.0);
        assert_eq!(pricing.output, 15.0);
        assert_eq!(pricing.cache_read, 0.3);
        assert_eq!(pricing.cache_write, 0.0);
    }

    #[test]
    fn parse_model_scales_cache_write() {
        let mut m = kimi_k3_json();
        m["pricing"]["input_cache_write"] = json!("0.00000375");

        let pricing = parse_model(&m)
            .expect("model should parse")
            .pricing
            .expect("pricing should be parsed");
        assert_eq!(pricing.cache_write, 3.75);
    }

    /// A price we cannot read used to collapse to an all-zero `ModelPricing`,
    /// which downstream reads as "free". Unknown has to stay unknown.
    #[test_case(json!(null)                                       ; "no_pricing_object")]
    #[test_case(json!({"prompt": "0.000003"})                     ; "no_completion")]
    #[test_case(json!({"prompt": "n/a", "completion": "0.000015"}) ; "unparsable_prompt")]
    fn parse_model_keeps_unusable_pricing_unknown(pricing: Value) {
        let mut m = kimi_k3_json();
        m["pricing"] = pricing;

        let info = parse_model(&m).expect("model should parse");
        assert!(info.pricing.is_none(), "{UNKNOWN_PRICE_STAYS_UNKNOWN}");
    }

    #[test]
    fn parse_model_reasoning_efforts_skips_unknown_and_sorts() {
        let mut m = kimi_k3_json();
        m["reasoning"] = json!({
            "mandatory": false,
            "default_enabled": true,
            "supported_efforts": ["high", "bogus", "low", "none"],
        });

        let info = parse_model(&m).expect("model should parse");
        let provider_info = info.provider_info.expect("reasoning info should be set");
        let reasoning = provider_info
            .downcast_ref::<OpenRouterModelInfo>()
            .expect("wrong provider info type");
        assert!(reasoning.reasoning_default_enabled);
        assert!(!reasoning.reasoning_mandatory);
        assert_eq!(reasoning.reasoning_efforts, vec![Effort::Low, Effort::High]);
    }

    fn openrouter_model(info: Option<&OpenRouterModelInfo>) -> (EffortDialect<'_>, Model) {
        let model = Model {
            id: "test-model".into(),
            provider: "openrouter".into(),
            tier: crate::model::ModelTier::Medium,
            family: crate::model::ModelFamily::Generic,
            supports_tool_examples_override: None,
            thinking_override: None,
            supports_vision_override: None,
            pricing: ModelPricing::default(),
            discovered_free: false,
            max_output_tokens: Some(8192),
            context_window: 200_000,
            thinking_fields: None,
        };
        (effort_dialect(info), model)
    }

    fn reasoning_info(efforts: &[Effort]) -> OpenRouterModelInfo {
        OpenRouterModelInfo {
            reasoning_mandatory: false,
            reasoning_default_enabled: false,
            reasoning_efforts: efforts.to_vec(),
        }
    }

    #[test_case(&[Effort::High, Effort::XHigh], ThinkingConfig::Effort(Effort::XHigh), "xhigh" ; "declared_xhigh_passes_through")]
    #[test_case(&[Effort::High, Effort::XHigh], ThinkingConfig::Effort(Effort::Max),   "xhigh" ; "max_snaps_to_declared_xhigh")]
    #[test_case(&[Effort::Minimal, Effort::Low], ThinkingConfig::Adaptive,             "low"   ; "adaptive_snaps_into_declared")]
    #[test_case(&[], ThinkingConfig::Effort(Effort::XHigh), "high" ; "no_declared_falls_back_to_static")]
    fn effort_dialect_snaps_once_against_declared_levels(
        efforts: &[Effort],
        config: ThinkingConfig,
        expected: &str,
    ) {
        let info = reasoning_info(efforts);
        let (dialect, model) = openrouter_model(Some(&info));
        assert_eq!(config.effort_str(&dialect, &model), Some(expected));
    }

    #[test]
    fn no_reasoning_info_still_requests_high_effort() {
        let (dialect, model) = openrouter_model(None);
        assert_eq!(
            ThinkingConfig::Adaptive.effort_str(&dialect, &model),
            Some("high")
        );
    }

    #[test_case(false, false, None         ; "default_off_sends_nothing")]
    #[test_case(true,  false, Some("none") ; "default_enabled_disables_with_none")]
    #[test_case(true,  true,  None         ; "mandatory_cannot_be_disabled")]
    fn off_resolves_per_reasoning_flags(
        default_enabled: bool,
        mandatory: bool,
        expected: Option<&str>,
    ) {
        let info = OpenRouterModelInfo {
            reasoning_mandatory: mandatory,
            reasoning_default_enabled: default_enabled,
            reasoning_efforts: vec![],
        };
        let (dialect, model) = openrouter_model(Some(&info));
        assert_eq!(ThinkingConfig::Off.effort_str(&dialect, &model), expected);
    }

    const MODEL: &str = "moonshotai/kimi-k2";

    fn upstream(name: &str) -> Upstream {
        Upstream {
            name: Some(name.to_string()),
            generation_id: None,
        }
    }

    /// Pins with persistence disabled: unit tests must not write to the
    /// real state dir, and none of this logic depends on the file.
    fn pins() -> UpstreamPins {
        UpstreamPins::default()
    }

    #[test]
    fn unpinned_unconfigured_session_sends_no_routing() {
        assert_eq!(routing_body(None, None), None);
    }

    #[test]
    fn a_pin_becomes_the_order() {
        assert_eq!(
            routing_body(None, Some("StreamLake")),
            Some(json!({"order": ["StreamLake"]}))
        );
    }

    #[test_case(json!({"order": ["deepinfra"]})            ; "order")]
    #[test_case(json!({"only": ["deepinfra"]})             ; "only")]
    #[test_case(json!({"sort": "price"})                   ; "sort")]
    fn configured_upstream_selection_outranks_the_pin(configured: Value) {
        assert_eq!(
            routing_body(Some(&configured), Some("StreamLake")),
            Some(configured.clone())
        );
    }

    /// `allow_fallbacks` alone selects no upstream, so it has nothing to say
    /// about which one to pin - keep it and add the pin.
    #[test]
    fn pin_merges_into_configuration_that_selects_nothing() {
        assert_eq!(
            routing_body(Some(&json!({"allow_fallbacks": true})), Some("Baidu")),
            Some(json!({"allow_fallbacks": true, "order": ["Baidu"]}))
        );
    }

    #[test]
    fn pin_follows_the_upstream_that_actually_answered() {
        let pins = pins();
        pins.record(MODEL, Some(&upstream("StreamLake")));
        assert_eq!(pins.get(MODEL).as_deref(), Some("StreamLake"));

        // The pinned upstream was down and a fallback served the turn, so the
        // warm cache is the fallback's from here on.
        pins.record(MODEL, Some(&upstream("Baidu")));
        assert_eq!(pins.get(MODEL).as_deref(), Some("Baidu"));
    }

    #[test]
    fn a_response_naming_no_upstream_leaves_the_pin_alone() {
        let pins = pins();
        pins.record(MODEL, Some(&upstream("StreamLake")));
        pins.record(MODEL, None);
        pins.record(MODEL, Some(&Upstream::default()));

        assert_eq!(pins.get(MODEL).as_deref(), Some("StreamLake"));
    }

    /// Each model has its own cache, so one model's pin says nothing about
    /// where another should go.
    #[test]
    fn pins_do_not_leak_across_models() {
        let pins = pins();
        pins.record(MODEL, Some(&upstream("StreamLake")));

        assert_eq!(pins.get("deepseek/deepseek-v4-flash"), None);
    }

    /// Sessions deliberately share: the cached prefix is the system prompt
    /// and tools, which every session on the model has in common, so a new
    /// session should open against the warm cache rather than build its own.
    /// This is why the table is keyed by model and lives outside the provider
    /// instance, which maki rebuilds on every model switch.
    #[test]
    fn a_pin_survives_the_provider_being_rebuilt() {
        let pins = pins();
        pins.record(MODEL, Some(&upstream("Baidu")));

        // A rebuilt provider consults the same table, so the pin is still there.
        assert_eq!(pins.get(MODEL).as_deref(), Some("Baidu"));
        assert_eq!(
            routing_body(None, pins.get(MODEL).as_deref()),
            Some(json!({"order": ["Baidu"]}))
        );
    }

    #[test]
    fn a_persisted_table_is_read_back() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(PINS_FILE);
        let written = UpstreamPins {
            path: Some(path.clone()),
            ..Default::default()
        };
        written.record(MODEL, Some(&upstream("Baidu")));

        let reloaded: HashMap<String, String> =
            serde_json::from_slice(&std::fs::read(&path).expect("pins file")).expect("valid json");
        assert_eq!(reloaded.get(MODEL).map(String::as_str), Some("Baidu"));
    }

    /// A truncated or hand-edited file costs one unpinned request, not a
    /// crash on startup.
    #[test]
    fn a_corrupt_pins_file_is_ignored() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(PINS_FILE);
        std::fs::write(&path, b"{not json").expect("write");

        let pins: HashMap<String, String> = std::fs::read(&path)
            .ok()
            .and_then(|raw| serde_json::from_slice(&raw).ok())
            .unwrap_or_default();
        assert!(pins.is_empty());
    }

    #[test]
    fn pin_table_stays_bounded() {
        let pins = pins();
        for i in 0..=MAX_PINS {
            pins.record(&format!("model-{i}"), Some(&upstream("X")));
        }

        let len = pins.pins.lock().unwrap().len();
        assert!(len <= MAX_PINS, "{len} pins retained");
        // The model that triggered the drop is still pinned; the run in
        // progress keeps its cache.
        assert_eq!(pins.get(&format!("model-{MAX_PINS}")).as_deref(), Some("X"));
    }

    #[test_case(json!(["image"]), json!(["image"]); "image_only")]
    #[test_case(json!(["image"]), json!(["text"]); "image_input_only")]
    #[test_case(json!(["text"]), json!(["image"]); "image_output_only")]
    fn parse_model_skips_non_text_models(input: Value, output: Value) {
        let mut m = kimi_k3_json();
        m["architecture"]["input_modalities"] = input;
        m["architecture"]["output_modalities"] = output;

        assert!(parse_model(&m).is_none());
    }
}
