//! Non-interactive (headless) mode: `maki "prompt" --print`.
//!
//! Wire format intentionally matches Claude Code so existing scripts work
//! unchanged. Keep `PrintResult` fields a strict subset of theirs. `StreamJson`
//! is JSONL with the same shape, `Text` prints the raw response only.
//!
//! We adopt new fields when Claude Code adds them but never invent our own.
//! Check their docs before changing anything here.

use std::io::{self, Read};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use clap::ValueEnum;
use color_eyre::Result;
use color_eyre::eyre::{Context, eyre};
use futures_lite::future::or;
use maki_agent::headless::{self, HeadlessHandle, HeadlessParams};
use maki_agent::permissions::PluginRuleStore;
use maki_agent::session::Resumed;
use maki_agent::tools::QUESTION_TOOL_NAME;
use maki_agent::{
    AgentConfig, AgentEvent, DoneReason, Envelope, ImageSource, PermissionsConfig, SteerKind,
};
use maki_config::{ModelPolicy, ProjectConfig, SessionDefaults};
use maki_lua::session_snapshot::{HeadlessMeta, HeadlessSnapshot, MODE_BUILD};
use maki_lua::{EventHandle, SessionEndReason};
use maki_providers::model::Model;
use maki_providers::{EMPTY_RESPONSE_MARKER, TokenUsage, add_cost};
use maki_shmem::Speaker;
use maki_shmem::broadcast::{
    Broadcaster, SessionSnapshot as ShmemSnapshot, SessionState as ShmemState,
};
use maki_shmem::control::{AckResult, Command};
use maki_storage::StateDir;
use maki_storage::id::SessionRef;
use maki_storage::sessions::SessionClaim;
use serde::Serialize;
use serde_json::Value;

const SESSION_LINE_PREFIX: &str = "session: ";

/// How often a quiet run still polls haunt's commands, so a `Cancel` is not
/// stuck behind a tool that emits nothing.
const COMMAND_POLL: Duration = Duration::from_millis(250);

// Fails fast: silently dropping an image the caller explicitly attached
// would be worse than erroring.
fn load_images(paths: &[PathBuf]) -> Result<Vec<ImageSource>> {
    paths
        .iter()
        .map(|path| {
            let media_type = maki_ui::image::media_type_for(path)
                .ok_or_else(|| eyre!("unsupported image type: {}", path.display()))?;
            maki_ui::image::load_file_image(path, media_type)
                .map_err(|e| eyre!("failed to load image: {e}"))
        })
        .collect()
}

#[derive(Clone, ValueEnum)]
pub enum OutputFormat {
    Text,
    Json,
    StreamJson,
}

#[derive(Serialize)]
struct PrintResult {
    #[serde(rename = "type")]
    result_type: &'static str,
    subtype: &'static str,
    is_error: bool,
    duration_ms: u128,
    num_turns: u32,
    result: String,
    stop_reason: Option<DoneReason>,
    session_id: SessionRef,
    total_cost_usd: f64,
    usage: TokenUsage,
}

#[derive(Serialize)]
struct InitEvent<'a> {
    #[serde(rename = "type")]
    event_type: &'static str,
    subtype: &'static str,
    cwd: &'a str,
    session_id: &'a SessionRef,
    tools: &'a [String],
    model: &'a str,
}

#[derive(Serialize)]
struct AssistantEvent<'a> {
    #[serde(rename = "type")]
    event_type: &'static str,
    message: AssistantMessage<'a>,
    session_id: &'a SessionRef,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_tool_use_id: Option<&'a str>,
}

#[derive(Serialize)]
struct AssistantMessage<'a> {
    model: &'a str,
    role: &'static str,
    content: &'a Value,
    usage: &'a TokenUsage,
}

#[derive(Serialize)]
struct UserEvent<'a> {
    #[serde(rename = "type")]
    event_type: &'static str,
    message: UserMessage<'a>,
    session_id: &'a SessionRef,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_tool_use_id: Option<&'a str>,
}

#[derive(Serialize)]
struct UserMessage<'a> {
    role: &'static str,
    content: &'a Value,
}

#[derive(Serialize)]
struct RetryEvent<'a> {
    #[serde(rename = "type")]
    event_type: &'static str,
    subtype: &'static str,
    attempt: u32,
    retry_delay_ms: u64,
    error: &'a str,
    session_id: &'a SessionRef,
}

enum VerboseOutput {
    StreamJson,
    Json(Vec<Value>),
}

impl VerboseOutput {
    fn emit(&mut self, value: &impl Serialize) -> Result<()> {
        match self {
            Self::StreamJson => println!("{}", serde_json::to_string(value)?),
            Self::Json(events) => events.push(serde_json::to_value(value)?),
        }
        Ok(())
    }
}

pub struct PrintParams {
    pub model: Model,
    pub prompt: Option<String>,
    pub image_paths: Vec<PathBuf>,
    pub format: OutputFormat,
    pub verbose: bool,
    pub config: AgentConfig,
    pub permissions_config: PermissionsConfig,
    pub timeouts: maki_providers::Timeouts,
    pub lua_handle: EventHandle,
    pub defaults: SessionDefaults,
    pub model_policy: Arc<ModelPolicy>,
    pub plugin_rules: Arc<PluginRuleStore>,
    pub project_config: ProjectConfig,
    /// Which session this run continues and writes under, and where. Resolved
    /// by the caller from the same flags every other entry point reads.
    pub resumed: Resumed,
    /// The right to write [`Self::resumed`]'s session, taken when it was
    /// resolved and held for the whole run.
    pub claim: SessionClaim,
    pub storage: StateDir,
}

pub fn run(params: PrintParams) -> Result<()> {
    let PrintParams {
        model,
        prompt,
        image_paths,
        format,
        verbose,
        config,
        permissions_config,
        timeouts,
        lua_handle,
        defaults,
        model_policy,
        plugin_rules,
        project_config,
        resumed,
        claim,
        storage,
    } = params;

    let prompt = match prompt {
        Some(p) => p,
        None => {
            let mut buf = String::new();
            io::stdin().read_to_string(&mut buf).context("read stdin")?;
            buf
        }
    };

    let images = load_images(&image_paths)?;

    let prompt_slots = lua_handle.collect_prompt_slots();

    let cwd = std::env::current_dir().unwrap_or_else(|_| ".".into());
    let (mcp_handle, mcp_config_errors) = smol::block_on(maki_agent::mcp::start_connected(
        &cwd,
        project_config.clone(),
    ));
    if !mcp_config_errors.is_empty() {
        eprintln!("MCP config error: {mcp_config_errors}");
    }

    // Read before `resumed` moves into the spawn: haunt's table shows the
    // stored title for `-r`, the prompt's first line for a fresh run.
    let broadcast_title = resumed
        .session
        .as_ref()
        .map(|s| s.title.clone())
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| prompt.lines().next().unwrap_or("maki -p").to_owned());
    // The prompt is moved into the spawn; haunt's chat feed quotes it.
    let broadcast_prompt = prompt.clone();
    let initial_context = resumed.context_size;

    let (handle, mut events) = maki_agent::headless::spawn(HeadlessParams {
        model: model.clone(),
        config,
        permissions_config,
        timeouts,
        prompt,
        images,
        prompt_slots,
        excluded_tools: vec![QUESTION_TOOL_NAME],
        mcp_handle,
        initial_wd: cwd,
        resumed,
        claim,
        storage,
        defaults,
        model_policy,
        plugin_rules,
        project_config,
    });

    let HeadlessHandle {
        tool_names,
        session_id,
        cwd,
        task,
    } = handle;
    let start = Instant::now();

    // Same per-process feed the TUI writes: latest-wins snapshots on a ring
    // haunt attaches to, parked and unlinked when this handle drops. A headless
    // run is always `Working` until it ends, when the session is removed.
    let broadcaster = Broadcaster::start().ok();
    let broadcast_status = |last_tool: &str, context_used: u32, cost_usd: f64| {
        if let Some(b) = &broadcaster {
            b.update(ShmemSnapshot {
                state: ShmemState::Working,
                context_used,
                context_limit: model.context_window,
                cost_usd,
                session_id: session_id.to_string(),
                title: broadcast_title.clone(),
                last_tool: last_tool.to_owned(),
            });
        }
    };

    let mut verbose_out = match format {
        OutputFormat::StreamJson => Some(VerboseOutput::StreamJson),
        _ if verbose => Some(VerboseOutput::Json(Vec::new())),
        _ => None,
    };

    match &mut verbose_out {
        Some(out) => out.emit(&InitEvent {
            event_type: "system",
            subtype: "init",
            cwd: &cwd,
            session_id: &session_id,
            tools: &tool_names,
            model: &model.id,
        })?,
        // Text mode never says which session it wrote, and a later `-r`
        // needs it. On stderr so a pipe reading the answer is unaffected.
        None => eprintln!("{SESSION_LINE_PREFIX}{session_id}"),
    }

    let mut result_text = String::new();
    let mut is_error = false;
    let mut num_turns: u32 = 0;
    let mut usage = TokenUsage::default();
    // Summed as the turns land: rates move mid-run, and only a turn knows the
    // rate it paid.
    let mut cost = None;
    let mut stop_reason: Option<DoneReason> = None;

    let snapshot = HeadlessSnapshot::default();
    snapshot.install(
        &lua_handle,
        HeadlessMeta {
            id: session_id.to_string(),
            cwd: cwd.clone(),
            model: model.spec(),
        },
        // `maki -p` always runs the agent in build mode
        // (`headless::spawn` hardcodes `AgentMode::Build`).
        || MODE_BUILD,
    );

    let mut cancelled = false;
    let mut last_tool = String::new();
    let mut context_used = initial_context;
    broadcast_status(&last_tool, context_used, 0.0);
    let broadcast_chat = |speaker: Speaker, text: &str| {
        if let Some(b) = &broadcaster {
            b.chat(&session_id.to_string(), speaker, text);
        }
    };
    broadcast_chat(Speaker::User, &broadcast_prompt);
    loop {
        // Ticks keep the command path live while a single long tool emits
        // nothing; the stream end and a tick are told apart by the wrapper.
        let next = smol::block_on(or(async { Some(events.next().await) }, async {
            smol::Timer::after(COMMAND_POLL).await;
            None
        }));
        let envelope = match next {
            Some(Some(envelope)) => envelope,
            // The run's event stream has ended.
            Some(None) => break,
            None => {
                cancelled |= apply_headless_commands(broadcaster.as_ref(), &session_id);
                if cancelled {
                    break;
                }
                continue;
            }
        };
        // Folded in first, so a plugin handling `TurnEnd` finds the finished
        // totals when it calls `maki.session.read()`.
        snapshot.observe(&envelope);
        maki_lua::agent_autocmd::dispatch(
            &lua_handle,
            &session_id,
            &envelope,
            envelope.subagent.is_some(),
        );
        let Envelope {
            ref event,
            ref subagent,
            ..
        } = envelope;
        let parent_tool_use_id = subagent.as_ref().map(|s| s.parent_tool_use_id.as_str());

        match event {
            AgentEvent::TextDelta { text } => {
                if parent_tool_use_id.is_none() {
                    result_text.push_str(text);
                }
            }
            AgentEvent::ThinkingDelta { .. } => {}
            AgentEvent::ToolStart(ts) => {
                last_tool = ts.tool.to_string();
                broadcast_status(&last_tool, context_used, cost.unwrap_or_default());
            }
            AgentEvent::ToolPending { .. }
            | AgentEvent::ToolOutput { .. }
            | AgentEvent::ToolDone(_)
            | AgentEvent::QueueItemConsumed { .. }
            | AgentEvent::QueueDrained
            | AgentEvent::CacheKeptWarm { .. }
            | AgentEvent::AutoCompacting { .. }
            | AgentEvent::CompactionDone { .. }
            | AgentEvent::AuthRequired
            | AgentEvent::PermissionRequest { .. }
            | AgentEvent::SubagentHistory { .. }
            | AgentEvent::ToolSnapshot { .. }
            | AgentEvent::ToolHeaderSnapshot { .. }
            | AgentEvent::LiveToolBuf { .. }
            | AgentEvent::CompactionStart { .. }
            | AgentEvent::RenameResult { .. }
            | AgentEvent::Nudge
            | AgentEvent::PromptProgress { .. }
            | AgentEvent::StreamClosed
            | AgentEvent::TurnToolsDone { .. } => {}
            AgentEvent::Steered { .. } => {
                if let Some(reason) = dropped_prompt(event, parent_tool_use_id) {
                    is_error = true;
                    result_text = reason.to_owned();
                }
            }
            AgentEvent::Retry {
                attempt,
                message,
                delay_ms,
            } => {
                if let Some(out) = &mut verbose_out {
                    out.emit(&RetryEvent {
                        event_type: "system",
                        subtype: "api_retry",
                        attempt: *attempt,
                        retry_delay_ms: *delay_ms,
                        error: message,
                        session_id: &session_id,
                    })?;
                }
            }
            AgentEvent::TurnComplete(tc) => {
                add_cost(&mut cost, tc.cost);
                if let Some(ctx) = tc.context_size {
                    context_used = ctx;
                }
                broadcast_status(&last_tool, context_used, cost.unwrap_or_default());
                if parent_tool_use_id.is_none()
                    && let Some(text) = tc.message.first_text_content()
                    && text != EMPTY_RESPONSE_MARKER
                {
                    broadcast_chat(Speaker::Assistant, text);
                }
                if let Some(out) = &mut verbose_out {
                    let content_value = serde_json::to_value(&tc.message.content)?;
                    out.emit(&AssistantEvent {
                        event_type: "assistant",
                        message: AssistantMessage {
                            model: &tc.model,
                            role: "assistant",
                            content: &content_value,
                            usage: &tc.usage,
                        },
                        session_id: &session_id,
                        parent_tool_use_id,
                    })?;
                }
            }
            AgentEvent::ToolResultsSubmitted { message } => {
                if let Some(out) = &mut verbose_out {
                    let content_value = serde_json::to_value(&message.content)?;
                    out.emit(&UserEvent {
                        event_type: "user",
                        message: UserMessage {
                            role: "user",
                            content: &content_value,
                        },
                        session_id: &session_id,
                        parent_tool_use_id,
                    })?;
                }
            }
            AgentEvent::Done {
                usage: u,
                num_turns: turns,
                reason,
                cost: dcost,
                ..
            } => {
                num_turns = *turns;
                usage = *u;
                // The agent's own ledger also counts compaction spend, which
                // summing the turns misses.
                cost = (*dcost).or(cost);
                stop_reason = Some(*reason);
                break;
            }
            AgentEvent::Error { message } => {
                is_error = true;
                result_text = message.clone();
                break;
            }
        }
        cancelled |= apply_headless_commands(broadcaster.as_ref(), &session_id);
        if cancelled {
            break;
        }
    }
    if let Some(b) = &broadcaster {
        b.remove(&session_id.to_string());
    }
    if cancelled {
        stop_reason = Some(DoneReason::Cancelled);
        smol::block_on(task.cancel());
    } else {
        smol::block_on(headless::await_shutdown(task));
    }
    lua_handle.end_sessions_blocking([session_id.id()], SessionEndReason::Completed);

    let duration_ms = start.elapsed().as_millis();
    // Zero on an unpriced model, which is what its turns reported too.
    let total_cost_usd = cost.unwrap_or_default();

    match format {
        OutputFormat::Text => {
            print!("{result_text}");
        }
        OutputFormat::Json | OutputFormat::StreamJson => {
            let result = PrintResult {
                result_type: "result",
                subtype: if is_error { "error" } else { "success" },
                is_error,
                duration_ms,
                num_turns,
                result: result_text,
                stop_reason,
                session_id,
                total_cost_usd,
                usage,
            };
            match verbose_out {
                Some(VerboseOutput::Json(mut events)) => {
                    events.push(serde_json::to_value(&result)?);
                    println!("{}", serde_json::to_string(&events)?);
                }
                _ => println!("{}", serde_json::to_string(&result)?),
            }
        }
    }

    Ok(())
}

/// Applies commands haunt sent this run: `Cancel` stops it, `Prompt` is
/// refused (a headless run takes no new input), `Refresh` never reaches here —
/// the broadcaster thread answers it inline. Returns whether the run was
/// cancelled.
fn apply_headless_commands(broadcaster: Option<&Broadcaster>, session_id: &SessionRef) -> bool {
    let Some(broadcaster) = broadcaster else {
        return false;
    };
    let mut cancelled = false;
    for pending in broadcaster.drain_commands() {
        let result = if pending.session_id != session_id.to_string() {
            AckResult::NoSession
        } else {
            match pending.command {
                Command::Cancel => {
                    cancelled = true;
                    AckResult::Ok
                }
                Command::Prompt(_) => AckResult::Rejected,
                Command::Refresh => AckResult::Ok,
            }
        };
        broadcaster.ack(&pending.session_id, pending.request_id, result);
    }
    cancelled
}

/// Nothing reached the model, so the reason is the only answer there is. A
/// script checking the exit status should see that the prompt never ran.
fn dropped_prompt<'a>(event: &'a AgentEvent, parent_tool_use_id: Option<&str>) -> Option<&'a str> {
    match event {
        AgentEvent::Steered {
            kind: SteerKind::MessageDropped,
            text,
        } if parent_tool_use_id.is_none() => Some(text),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maki_providers::TokenUsage;
    use test_case::test_case;

    const DROP_REASON: &str = "blocked by a plugin";
    const PARENT_TOOL_USE_ID: &str = "toolu_parent";

    #[test_case(SteerKind::MessageDropped, None, Some(DROP_REASON) ; "top_level_drop_is_the_result")]
    #[test_case(SteerKind::MessageDropped, Some(PARENT_TOOL_USE_ID), None ; "subagent_drop_is_ignored")]
    #[test_case(SteerKind::MessageRewritten, None, None ; "other_steers_are_ignored")]
    fn dropped_prompt_only_counts_a_top_level_drop(
        kind: SteerKind,
        parent: Option<&str>,
        expected: Option<&str>,
    ) {
        let event = AgentEvent::Steered {
            kind,
            text: DROP_REASON.into(),
        };
        assert_eq!(dropped_prompt(&event, parent), expected);
    }

    const PRINT_RESULT_FIELDS: &[&str] = &[
        "type",
        "subtype",
        "is_error",
        "num_turns",
        "result",
        "stop_reason",
        "session_id",
        "total_cost_usd",
        "usage",
        "duration_ms",
    ];
    const INIT_EVENT_FIELDS: &[&str] = &["type", "subtype", "cwd", "session_id", "tools", "model"];
    const RETRY_EVENT_FIELDS: &[&str] = &[
        "type",
        "subtype",
        "attempt",
        "retry_delay_ms",
        "error",
        "session_id",
    ];

    #[test]
    fn wire_format_required_fields() {
        let result = PrintResult {
            result_type: "result",
            subtype: "success",
            is_error: false,
            duration_ms: 1234,
            num_turns: 2,
            result: "done".into(),
            stop_reason: Some(DoneReason::EndTurn),
            session_id: SessionRef::generate(),
            total_cost_usd: 0.003,
            usage: TokenUsage::default(),
        };
        let json: Value = serde_json::to_value(&result).unwrap();
        for field in PRINT_RESULT_FIELDS {
            assert!(json.get(field).is_some(), "PrintResult missing: {field}");
        }

        let sid = SessionRef::generate();
        let init = InitEvent {
            event_type: "system",
            subtype: "init",
            cwd: "/tmp",
            session_id: &sid,
            tools: &["bash".into(), "read".into()],
            model: "test-model",
        };
        let json: Value = serde_json::to_value(&init).unwrap();
        for field in INIT_EVENT_FIELDS {
            assert!(json.get(field).is_some(), "InitEvent missing: {field}");
        }

        let retry = RetryEvent {
            event_type: "system",
            subtype: "api_retry",
            attempt: 2,
            retry_delay_ms: 3000,
            error: "rate_limit",
            session_id: &sid,
        };
        let json: Value = serde_json::to_value(&retry).unwrap();
        for field in RETRY_EVENT_FIELDS {
            assert!(json.get(field).is_some(), "RetryEvent missing: {field}");
        }
    }
}
