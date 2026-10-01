use std::sync::{Arc, Mutex};

use flume::Sender;
use maki_config::providers::{Protocol, ProviderPlan};
use maki_storage::id::SessionRef;
use serde::Deserialize;
use serde_json::Value;
use tracing::warn;

use crate::model::{Model, ModelFamily, ThinkingSupport};
use crate::provider::{BoxFuture, Provider};
use crate::providers::aperture::NO_PATH_PREFIX;
use crate::providers::openai_compat::{OpenAiCompatConfig, OpenAiCompatProvider, impl_stream_body};
use crate::spec::{
    ApertureRoute, AuthDoc, CatalogDoc, GeneratedDocs, LoginConfig, Native, ProviderSpec,
};
use crate::{
    AgentError, Message, ProviderEvent, ProviderUsage, RequestOptions, StreamResponse, UsageLimit,
    dialect,
};

use super::{KeyHeader, KeyPool, KeyRotation, ResolvedAuth, Timeouts, key_rotation};

const SLUG: &str = "zai";
const DISPLAY_NAME: &str = "Z.AI";
const ENV_VAR: &str = "ZHIPU_API_KEY";
const BASE_URL: &str = "https://api.z.ai/api/paas/v4";
const CODING_BASE_URL: &str = "https://api.z.ai/api/coding/paas/v4";
const DEFAULT_MODEL: &str = "zai/glm-5.1";
const CODING_MODEL: &str = "zai/glm-5-code";
const LOGIN_URL: &str = "https://z.ai/manage-apikey/apikey-list";
const MAX_TOKENS_FIELD: &str = "max_tokens";
const AUTH_NOTE: &str = "(shared across both endpoints)";

static CONFIG_STANDARD: OpenAiCompatConfig = OpenAiCompatConfig {
    slug: SLUG,
    api_key_env: ENV_VAR,
    base_url: BASE_URL,
    max_tokens_field: MAX_TOKENS_FIELD,
    include_stream_usage: false,
    provider_name: DISPLAY_NAME,
};

const PLANS: &[(&str, ProviderPlan)] = &[
    (
        "standard",
        ProviderPlan {
            display_name: "Pay-as-you-go",
            base_url: BASE_URL,
            default_model: Some(DEFAULT_MODEL),
            login_url: None,
        },
    ),
    (
        "coding",
        ProviderPlan {
            display_name: "Coding plan",
            base_url: CODING_BASE_URL,
            default_model: Some(CODING_MODEL),
            login_url: None,
        },
    ),
];

/// The gateway gets no path prefix: Z.AI's API has no `/v1` segment at all
/// (`/api/paas/v4/chat/completions`), so the upstream base url must carry the
/// whole path and any prefix would double it up.
pub(crate) const SPEC: ProviderSpec = ProviderSpec {
    slug: SLUG,
    display_name: DISPLAY_NAME,
    api_key_env: ENV_VAR,
    family: ModelFamily::Glm,
    supports_thinking: false,
    accepts_arbitrary_models: false,
    fallback_max_output: Some(16_000),
    fallback_context_window: 128_000,
    models_toml: include_str!("../../../models/zai.toml"),
    pricing_schedule: None,
    native: Some(Native {
        new: create,
        with_auth: create_with_auth,
        aperture: Some(ApertureRoute {
            path_prefix: NO_PATH_PREFIX,
        }),
    }),
    login: Some(LoginConfig {
        protocol: Protocol::Openai,
        default_base_url: BASE_URL,
        default_model: DEFAULT_MODEL,
        plans: Some(PLANS),
        login_url: Some(LOGIN_URL),
        needs_url: false,
    }),
    docs: GeneratedDocs {
        api_urls: &[BASE_URL, CODING_BASE_URL],
        features: None,
        auth: AuthDoc::EnvVarWith(AUTH_NOTE),
        catalog: CatalogDoc::Table,
        trailing_notes: &[],
    },
};

fn create(timeouts: Timeouts) -> Result<Box<dyn Provider>, AgentError> {
    Ok(Box::new(Zai::new(timeouts)?))
}

fn create_with_auth(
    auth: Arc<Mutex<ResolvedAuth>>,
    timeouts: Timeouts,
    system_prefix: Option<String>,
) -> Result<Box<dyn Provider>, AgentError> {
    Ok(Box::new(
        Zai::with_auth(auth, timeouts)?.with_system_prefix(system_prefix),
    ))
}

const QUOTA_LIMIT_URL: &str = "https://api.z.ai/api/monitor/usage/quota/limit";

/// Z.AI serves permanent account problems behind a 429: `1113` is an empty
/// balance, `1309` an expired coding plan. Waiting cures none of them, so
/// `stream_message` rewrites them to 402, which the retry loop treats as
/// final. `1311` (a model the plan does not include) is deliberately not
/// here: the gateway answers it intermittently even for included models, and
/// logged stretches end in success at a median of three retries, so it draws
/// the bounded rate-limit budget via `AgentError::retry_kind` instead.
const PERMANENT_CODES: [&str; 2] = ["1113", "1309"];

const PLAN_GATED_CODE: &str = "1311";

fn permanent_account_error(status: u16, message: &str) -> bool {
    (status == 429 || status >= 500) && permanent_account_body(message)
}

/// The body half of [`permanent_account_error`], shared with
/// `AgentError::retry_kind`: any provider whose gateway answers in Z.AI's
/// format carries these, not just the `zai` one.
pub(crate) fn permanent_account_body(message: &str) -> bool {
    PERMANENT_CODES.iter().any(|code| message.contains(code)) || message.contains("nsufficien")
}

pub(crate) fn plan_gated_body(message: &str) -> bool {
    message.contains(PLAN_GATED_CODE)
}

/// First GLM that takes thinking parameters at all. A floor instead of a prefix
/// allowlist, so the next GLM gets thinking without us shipping a release.
const THINKING_SINCE: (u32, u32) = (5, 2);

#[derive(Deserialize)]
struct QuotaResponse {
    data: QuotaData,
}

#[derive(Deserialize, Default)]
struct QuotaData {
    #[serde(default)]
    limits: Vec<QuotaLimit>,
    #[serde(default)]
    level: Option<String>,
}

#[derive(Deserialize)]
struct QuotaLimit {
    #[serde(rename = "type")]
    kind: String,
    unit: u32,
    percentage: u32,
    #[serde(default, rename = "nextResetTime")]
    next_reset_time: Option<u64>,
}

fn quota_label(kind: &str, unit: u32) -> String {
    match (kind, unit) {
        ("TOKENS_LIMIT", 3) => "5-hour tokens".into(),
        ("TOKENS_LIMIT", 6) => "Weekly tokens".into(),
        ("TIME_LIMIT", _) => "Subscription time".into(),
        _ => format!("{kind} #{unit}"),
    }
}

impl From<QuotaResponse> for ProviderUsage {
    fn from(resp: QuotaResponse) -> Self {
        ProviderUsage {
            plan: resp.data.level,
            limits: resp
                .data
                .limits
                .into_iter()
                .map(|l| UsageLimit {
                    label: quota_label(&l.kind, l.unit),
                    percentage: Some(l.percentage),
                    reset_at: l.next_reset_time,
                    detail: None,
                })
                .collect(),
            by_model_today: vec![],
        }
    }
}

inventory::submit!(SPEC.config_row());

pub struct Zai {
    compat: OpenAiCompatProvider,
    auth: Arc<Mutex<ResolvedAuth>>,
    key_pool: Option<KeyPool>,
    system_prefix: Option<String>,
}

impl Zai {
    pub fn new(timeouts: super::Timeouts) -> Result<Self, AgentError> {
        let pool = KeyPool::resolve("zai", CONFIG_STANDARD.api_key_env)?;
        let mut auth = ResolvedAuth::bearer("zai", pool.current())?;
        let provider_config = maki_config::providers::ProvidersConfig::load();
        if let Some(url) =
            maki_config::providers::resolve_base_url("zai", provider_config.get("zai"))
        {
            auth.base_url = Some(url);
        }
        Ok(Self {
            compat: OpenAiCompatProvider::new(&CONFIG_STANDARD, timeouts)?,
            auth: Arc::new(Mutex::new(auth)),
            key_pool: Some(pool),
            system_prefix: None,
        })
    }

    pub(crate) fn with_auth(
        auth: Arc<Mutex<ResolvedAuth>>,
        timeouts: super::Timeouts,
    ) -> Result<Self, AgentError> {
        Ok(Self {
            compat: OpenAiCompatProvider::new(&CONFIG_STANDARD, timeouts)?,
            auth,
            key_pool: None,
            system_prefix: None,
        })
    }

    pub(crate) fn with_system_prefix(mut self, prefix: Option<String>) -> Self {
        self.system_prefix = prefix;
        self
    }
}

impl_stream_body!(Zai);

impl Provider for Zai {
    fn stream_message<'a>(
        &'a self,
        model: &'a Model,
        messages: &'a [Message],
        system: &'a str,
        tools: &'a Value,
        event_tx: &'a Sender<ProviderEvent>,
        opts: RequestOptions,
        _session_id: Option<&'a SessionRef>,
    ) -> BoxFuture<'a, Result<StreamResponse, AgentError>> {
        Box::pin(async move {
            let (auth, mut body) = self.stream_body(&self.auth, model, messages, system, tools);
            if model.supports_thinking() {
                opts.thinking
                    .apply_reasoning_effort(&mut body, &dialect::GLM, model);
            }
            match self
                .compat
                .do_stream(model, &[], &body, event_tx, &auth)
                .await
            {
                Err(AgentError::Api {
                    status, message, ..
                }) if permanent_account_error(status, &message) => {
                    warn!(status, "permanent account error, bailing out");
                    Err(AgentError::api(402, message))
                }
                result => result,
            }
        })
    }

    fn list_models(&self) -> BoxFuture<'_, Result<Vec<crate::model::ModelInfo>, AgentError>> {
        Box::pin(async move {
            let auth = self.auth.lock().unwrap_or_else(|e| e.into_inner()).clone();
            self.compat.do_list_models(&auth).await
        })
    }

    fn fetch_usage(&self) -> BoxFuture<'_, Result<Option<ProviderUsage>, AgentError>> {
        Box::pin(async move {
            let auth = self.auth.lock().unwrap_or_else(|e| e.into_inner()).clone();
            let body = self.compat.get_text(&auth, QUOTA_LIMIT_URL).await?;
            let parsed: QuotaResponse = serde_json::from_str(&body)?;
            Ok(Some(parsed.into()))
        })
    }

    fn keys(&self) -> Option<KeyRotation<'_>> {
        key_rotation(&self.key_pool, &self.auth, KeyHeader::Bearer)
    }

    fn adjust_model(&self, model: &mut Model) {
        adjust_model(model);
    }
}

/// `glm-5.3` -> `(5, 3)`, `glm-5.3-flash` -> `(5, 3)`, `glm-5-code` -> `(5, 0)`.
/// Same trick as `claude_version`.
fn glm_version(model_id: &str) -> Option<(u32, u32)> {
    let mut parts = model_id.strip_prefix("glm-")?.split(['-', '.']);
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    Some((major, minor))
}

fn adjust_model(model: &mut Model) {
    let Some(version) = glm_version(&model.id) else {
        return;
    };
    if version >= THINKING_SINCE {
        model.thinking_override = Some(ThinkingSupport::Yes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ModelTier;
    use test_case::test_case;

    const SAMPLE_BODY: &str = r#"{"code":200,"data":{"limits":[
        {"type":"TOKENS_LIMIT","unit":3,"percentage":16,"nextResetTime":1777819631597},
        {"type":"TOKENS_LIMIT","unit":6,"percentage":4,"nextResetTime":1778262784969},
        {"type":"TIME_LIMIT","unit":5,"percentage":0,"nextResetTime":1780336384978}
    ],"level":"lite"}}"#;

    const LONG_CONTEXT: u32 = 1_000_000;
    const GLM_5_CONTEXT: u32 = 200_000;

    #[test_case("zai/glm-5.3", Some(ThinkingSupport::Yes) ; "glm_5_3_supports_thinking")]
    #[test_case("zai/glm-5.3-flash", Some(ThinkingSupport::Yes) ; "glm_5_3_flash_supports_thinking")]
    #[test_case("zai/glm-5.4", Some(ThinkingSupport::Yes) ; "future_glm_stays_above_the_floor")]
    #[test_case("zai/glm-5.2", Some(ThinkingSupport::Yes) ; "glm_5_2_supports_thinking")]
    #[test_case("zai/glm-5.1", None ; "glm_5_1_no_thinking")]
    #[test_case("zai/glm-5-code", None ; "glm_5_code_reads_as_5_0")]
    #[test_case("zai/glm-4.7", None ; "glm_4_7_no_thinking")]
    fn adjust_model_sets_thinking_support(spec: &str, expected: Option<ThinkingSupport>) {
        let mut model = Model::from_spec(spec).unwrap();
        adjust_model(&mut model);
        assert_eq!(model.thinking_override, expected);
    }

    #[test_case("zai/glm-5.3", LONG_CONTEXT ; "glm_5_3_1m_context")]
    #[test_case("zai/glm-5.3-flash", LONG_CONTEXT ; "glm_5_3_flash_1m_context")]
    #[test_case("zai/glm-5.1", GLM_5_CONTEXT ; "glm_5_1_keeps_200k")]
    #[test_case("zai/glm-5.4", GLM_5_CONTEXT ; "unknown_glm_5_x_falls_back_to_glm_5_entry")]
    fn model_entry_context_window(spec: &str, expected: u32) {
        assert_eq!(Model::from_spec(spec).unwrap().context_window, expected);
    }

    /// The longest prefix wins, so without its own entry the cheap flash model
    /// would answer as the strong `glm-5.3` and get billed like it.
    #[test]
    fn glm_5_3_flash_does_not_ride_the_glm_5_3_entry() {
        let flash = Model::from_spec("zai/glm-5.3-flash").unwrap();
        assert_eq!(flash.tier, ModelTier::Weak);
        assert!(flash.supports_vision());
    }

    /// These two shared `glm-5`'s row and so billed at its cheaper rate. That
    /// hurt: `glm-5.1` is the provider default, so a plain session under-
    /// reported what it spent.
    #[test_case("zai/glm-5.1" ; "glm_5_1_bills_above_glm_5")]
    #[test_case("zai/glm-5.2" ; "glm_5_2_bills_above_glm_5")]
    fn glm_5_x_does_not_ride_the_glm_5_entry(spec: &str) {
        let glm_5 = Model::from_spec("zai/glm-5").unwrap().pricing;
        let pricing = Model::from_spec(spec).unwrap().pricing;
        assert!(pricing.input > glm_5.input);
        assert!(pricing.output > glm_5.output);
        assert!(pricing.cache_read > glm_5.cache_read);
    }

    const PLAN_LACKS_MODEL_BODY: &str =
        r#"{"error":{"code":"1311","message":"当前订阅套餐暂未开放GLM-5.3-Highspeed权限"}}"#;
    const INSUFFICIENT_BODY: &str = r#"{"error":{"code":"1113","message":"账户已欠费"}}"#;
    const RATE_LIMITED_BODY: &str = r#"{"error":{"code":"1302","message":"触发速率限制"}}"#;

    /// A real throttle on the same status must keep its backoff.
    #[test_case(429, PLAN_LACKS_MODEL_BODY, false ; "plan_gate_is_not_permanent")]
    #[test_case(429, INSUFFICIENT_BODY, true ; "insufficient_balance")]
    #[test_case(500, INSUFFICIENT_BODY, true ; "insufficient_balance_on_5xx")]
    #[test_case(429, RATE_LIMITED_BODY, false ; "rate_limit_stays_retryable")]
    #[test_case(400, PLAN_LACKS_MODEL_BODY, false ; "other_status_stays")]
    fn permanent_account_errors_are_detected(status: u16, body: &str, expected: bool) {
        assert_eq!(permanent_account_error(status, body), expected);
    }

    #[test]
    fn parse_quota_response() {
        let parsed: QuotaResponse = serde_json::from_str(SAMPLE_BODY).unwrap();
        let usage: ProviderUsage = parsed.into();
        assert_eq!(usage.plan.as_deref(), Some("lite"));
        assert_eq!(usage.limits.len(), 3);
        assert_eq!(usage.limits[0].label, "5-hour tokens");
        assert_eq!(usage.limits[0].percentage, Some(16));
        assert_eq!(usage.limits[0].reset_at, Some(1777819631597));
        assert_eq!(usage.limits[1].label, "Weekly tokens");
        assert_eq!(usage.limits[2].label, "Subscription time");
        assert_eq!(usage.limits[2].reset_at, Some(1780336384978));
    }

    #[test]
    fn parse_quota_unknown_unit_falls_back() {
        let body = r#"{"code":200,"data":{"limits":[
            {"type":"TOKENS_LIMIT","unit":9,"percentage":50}
        ]}}"#;
        let parsed: QuotaResponse = serde_json::from_str(body).unwrap();
        let usage: ProviderUsage = parsed.into();
        assert!(usage.plan.is_none());
        assert_eq!(usage.limits[0].label, "TOKENS_LIMIT #9");
        assert_eq!(usage.limits[0].percentage, Some(50));
        assert_eq!(usage.limits[0].reset_at, None);
    }
}
