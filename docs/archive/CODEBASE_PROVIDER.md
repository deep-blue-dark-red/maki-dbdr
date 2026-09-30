# Codebase: Provider System

## Model Registry

**`maki-providers/src/model.rs`**

`ModelEntry` — static table entry per model with prefix, tier, family, pricing, context window.

**Prefix lookup** (longest prefix wins): `claude-sonnet-4-20250514` matches `claude-sonnet-4`. Fresh model snapshots resolve without table churn.

`Model` — resolved model object: provider, tier, family, pricing, capabilities.
- `from_spec("provider/model")` — parse spec string
- `from_tier(provider, tier)` — pick best model for tier
- `from_base` — fallback for unknown models

`TokenUsage` — tracks input/output/cache tokens. `cost(pricing, fast)` computes USD cost. Fast mode derives cache multiplier from pricing.

`ModelFamily` — gates features like `supports_tool_examples()`.
`ModelTier` — Strong / Medium / Weak / Compaction.

Unknown models fall back to provider defaults so specs are never invalid. `dynamic::lookup_model` lets user-configured providers override the base kind.

## Provider Trait

**`maki-providers/src/provider.rs`**

`ProviderKind` — enum of 12 providers:
- Anthropic (Swappable to Bedrock)
- OpenAI (with Platform)
- Google Gemini
- GitHub Copilot
- Mistral
- Z.AI
- DeepSeek
- OpenRouter
- Ollama
- llama.cpp
- Synthetic (test)
- TensorX

Each carries: `base_url()`, `api_key_env()`, `family()`, defaults for context/output, feature flags.

**`Provider` trait** — one required method `stream_message()` returning `BoxFuture`. Optional hooks:
- `list_models()` — discover available models
- `refresh_auth()` — refresh expired auth
- `reload_auth()` — reload credentials
- `rotate_key()` — rotate API key in pool
- `adjust_model()` — provider-specific model adjustments

`ProviderKind::create(timeouts)` — factory returning `Box<dyn Provider>`.
`from_model()` — creates provider instance from `Model`. Uses dynamic slug if present, else `ProviderKind::create()`. Falls back to `UnconfiguredProvider` on error.

`fetch_all_models()` — concurrent async discovery. Calls `list_models()` on every configured provider in parallel via `smol::spawn`, collects `ModelBatch` results.

## HTTP Infrastructure

**`maki-providers/src/providers/mod.rs`**

Shared HTTP layer:
- `Timeouts` — connect (10s) / stream (300s) / low-speed (30s)
- `next_sse_line()` — timeout-aware SSE line reader. Resets deadline on each received line.
- `SseErrorPayload` — normalizes provider-specific errors into HTTP status codes
- `send_request()` — central HTTP call with optional API logging (`LOG_API` flag). Writes byte-exact `.mlog` records via `wire_log`. `send_request_with_fragments()` variant lets a provider pass per-message wire fragments for message-granular dedup.
- `LoggingBody` — wraps response body for logging, accumulates raw bytes, writes a `RESPONSE` record on completion.

## API Logging Format

**`maki-providers/src/wire_log.rs`** — `MLOG` binary log (behind `LOG_API`).

Chat APIs are stateless, so every turn re-sends the whole conversation plus identical tool/system blocks. `wire_log` stores each request/response once and **byte-exactly** (raw wire message always recoverable), deduplicating repeats two ways in one file:
- **Fragment interning** (Anthropic): the wire body is split at `messages` boundaries (`fragment_body`); each fragment is interned once as a `DEF` and referenced by id. Immune to the sliding `cache_control` window. Fragments are only trusted when they reconstruct the exact bytes, else it falls back.
- **Byte diff** (other providers): each request is a prefix/suffix patch (`compute_patch`) against the previous raw body, with full-record keyframes.

Record layout: `u64 ts_ms | u8 type | u32 len | payload` (`type & 0x80` = zstd). Per-session interning state lives in a `LazyLock<Mutex<HashMap<PathBuf, SessionLog>>>`.

**`maki-providers/src/bin/mlog.rs`** — standalone viewer: `mlog [--raw|--transcript] <file>`.

**`KeyPool`** — round-robin API key rotation. Resolves from:
1. Env var (comma-separated for pooling)
2. Saved credentials file
3. `providers.toml`

## Message Types

**`maki-providers/src/types.rs`**

`ContentBlock` — variant enum:
- `Text`
- `Thinking`
- `RedactedThinking`
- `ToolUse`
- `ToolResult`
- `Image`

`Message` — `{role, content, display_text}`. `display_text: Some("")` marks synthetic/system-injected messages invisible in UI. Conveniences: `user()`, `user_with_images()`, `synthetic()`, `tool_uses()`.

`ProviderEvent` — streaming events pushed via `flume::Sender`:
- `TextDelta`
- `ThinkingDelta`
- `ToolUseStart`

`StopReason` — normalized: `EndTurn`, `ToolUse`, `MaxTokens`. Converter per provider: `from_anthropic`, `from_openai`, `from_google`.

`ThinkingConfig` — `Off` / `Adaptive` / `Budget(u32)`. `apply_to_body()` injects the right JSON into API request body. Handles Anthropic's version branching (budget_tokens before 4.7, adaptive+effort from 4.7+).

`RequestOptions` — carries `thinking` config and `fast` mode flag.
`StreamResponse` — final result: `{message, usage, stop_reason}`.

## Per-Provider Implementations

**`maki-providers/src/providers/`** — per-provider modules, each implementing the `Provider` trait.

| File | Provider |
|---|---|
| `anthropic/mod.rs` | Anthropic |
| `anthropic/bedrock.rs` | Anthropic via AWS Bedrock |
| `anthropic/shared.rs` | Shared anthropic helpers |
| `openai/mod.rs` | OpenAI |
| `openai/platform.rs` | OpenAI platform specific |
| `openai/responses.rs` | OpenAI Responses API |
| `openai/auth.rs` | OpenAI OAuth |
| `google.rs` | Google Gemini |
| `copilot/mod.rs` | GitHub Copilot |
| `copilot/auth.rs` | Copilot auth |
| `mistral.rs` | Mistral |
| `zai/mod.rs` | Z.AI |
| `deepseek.rs` | DeepSeek |
| `openrouter.rs` | OpenRouter |
| `ollama.rs` | Ollama |
| `llama_cpp.rs` | llama.cpp |
| `local.rs` | Local providers (Ollama, LlamaCpp) |
| `synthetic.rs` | Synthetic test provider |
| `tensorx.rs` | TensorX |
| `openai_compat.rs` | OpenAI-compatible APIs |
| `dynamic.rs` | User-configured dynamic providers |
| `custom.rs` | providers.toml custom entries |

## Error Handling

**`maki-providers/src/error.rs`** — `AgentError` types, provider-specific error normalization.

## Retry

**`maki-providers/src/retry.rs`** — exponential backoff with jitter for API calls. `stream_with_retry()` in agent crate wraps provider calls to emit `AgentEvent::Retry`.
