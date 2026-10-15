//! A stronger model reading the transcripts of workers on weaker ones, in the
//! background. Its output is the expensive side, so silence is its usual
//! answer, and no worker ever waits on it: a plan or a question goes out
//! through a tool that returns at once, and whatever comes back lands between
//! turns.
//!
//! One supervisor serves a whole session, the main agent and every subagent,
//! each in a thread of its own. The threads share one system prompt, so the
//! provider caches that prefix once for all of them.

mod render;

use std::collections::HashMap;
use std::iter;
use std::mem;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, PoisonError, Weak};

use async_lock::Semaphore;
use maki_config::ModelPolicy;
use maki_providers::provider::{self, Provider};
use maki_providers::retry::RetryPolicy;
use maki_providers::{
    ContentBlock, Message, Model, ModelTier, RequestOptions, Role, ThinkingConfig, Timeouts,
    estimate_message_tokens, model_registry,
};
use maki_storage::id::{MakiId, SessionRef};
use serde_json::{Value, json};
use tracing::{info, warn};

use super::instructions::load_instruction_text;
use super::streaming::{StreamError, StreamRequest, stream_with_retry};
use crate::cancel::CancelToken;
use crate::tools::{LocalTool, ToolAudience, local_tool};
use crate::{AgentConfig, Envelope, EventSender, RunLedger};

pub(crate) const TOOL_NAME: &str = "supervisor";
const TOOL_FIELD: &str = "message";
const TOOL_DESCRIPTION: &str = "Send a message to your supervisor, a stronger model reviewing your work in the background: your plan of work, a question, or a status. Returns at once and never blocks. A reply, if any, arrives later as a <supervisor> message; no reply means carry on.";
const TOOL_ACK: &str =
    "Sent. Keep working; a reply may arrive later as a <supervisor> message, or none.";
pub(crate) const KICKOFF: &str = "A supervisor reviews your transcript in the background. Before acting, send it a short plan of work with the `supervisor` tool, adding any questions, then start at once without waiting. Its rare replies arrive as <supervisor> messages: corrections, answers, pointers to instruction files. Follow them; do not reply to acknowledge them.";
pub(crate) const HALTED_NOTE: &str = "The supervisor stopped this run:";
pub(crate) const HALTED_CALL: &str = "Not run: the supervisor stopped the run.";
const MAIN_WORKER: &str = "main";
const SYSTEM: &str = include_str!("../../prompts/supervisor.md");
const SILENT: &str = "OK";
const NOTE: &str = "NOTE";
const STOP: &str = "STOP";
/// Room for thinking. The answer itself is one line.
const REVIEW_OUTPUT_BUDGET: u32 = 8_000;
/// A session can run many subagents at once, and each review is a call to the
/// priciest model around.
const MAX_CONCURRENT_REVIEWS: usize = 2;
/// A thread past this share of the window starts over from the goal.
const THREAD_WINDOW_DIVISOR: u32 = 2;
/// The chars/4 floor the token estimator uses.
const CHARS_PER_TOKEN: usize = 4;

static SUPERVISORS: LazyLock<Mutex<HashMap<MakiId, Weak<Supervisor>>>> =
    LazyLock::new(Mutex::default);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

pub(crate) struct Supervisor {
    provider: Arc<dyn Provider>,
    model: Model,
    system: String,
    batch_turns: u32,
    batch_tokens: u32,
    max_notes: u32,
    reviews: Semaphore,
}

/// What a worker tells its supervisor thread about itself.
pub(crate) struct Worker<'a> {
    pub task_id: Option<&'a str>,
    pub goal: &'a str,
    pub ledger: Arc<RunLedger>,
    pub session_id: Option<SessionRef>,
    pub retry: RetryPolicy,
}

impl Supervisor {
    /// Weak, like the mailboxes: the supervisor lives while some worker of the
    /// session does, and the next one to start after that builds a fresh one.
    pub(crate) fn for_session(
        session: Option<&SessionRef>,
        config: &AgentConfig,
        worker: &Model,
        policy: &ModelPolicy,
        timeouts: Timeouts,
    ) -> Option<Arc<Self>> {
        if !config.supervisor {
            return None;
        }
        let Some(session) = session else {
            return Self::build(config, worker, policy, timeouts).map(Arc::new);
        };
        let mut live = lock(&SUPERVISORS);
        live.retain(|_, supervisor| supervisor.strong_count() > 0);
        if let Some(supervisor) = live.get(&session.id()).and_then(Weak::upgrade) {
            return Some(supervisor);
        }
        let supervisor = Arc::new(Self::build(config, worker, policy, timeouts)?);
        live.insert(session.id(), Arc::downgrade(&supervisor));
        Some(supervisor)
    }

    fn build(
        config: &AgentConfig,
        worker: &Model,
        policy: &ModelPolicy,
        timeouts: Timeouts,
    ) -> Option<Self> {
        let spec = config
            .supervisor_model
            .clone()
            .or_else(|| model_registry::spec_for_tier(&worker.provider, ModelTier::Strong))
            .or_else(|| model_registry::spec_for_tier_any(ModelTier::Strong))?;
        let built = Model::from_spec_with_policy(&spec, policy)
            .map_err(|e| e.to_string())
            .and_then(|mut model| {
                provider::from_model(&mut model, timeouts)
                    .map(|provider| (model, Arc::from(provider)))
                    .map_err(|e| e.to_string())
            });
        let (model, provider) = match built {
            Ok(built) => built,
            Err(error) => {
                warn!(%spec, %error, "supervisor model unavailable, running unsupervised");
                return None;
            }
        };
        Some(Self {
            provider,
            model,
            system: system_prompt(),
            batch_turns: config.supervisor_batch_turns,
            batch_tokens: config.supervisor_batch_tokens,
            max_notes: config.supervisor_max_notes,
            reviews: Semaphore::new(MAX_CONCURRENT_REVIEWS),
        })
    }

    pub(crate) fn outranks(&self, worker: &Model) -> bool {
        self.model.tier > worker.tier
    }

    /// `seen` is where the worker's history stands now: the goal travels on
    /// its own, so nothing before it is rendered again.
    pub(crate) fn attach(self: &Arc<Self>, worker: Worker<'_>, seen: usize) -> WorkerLink {
        let (ticks, rx) = flume::unbounded();
        let inbox = Arc::new(Mutex::new(Inbox::default()));
        let (sink, sink_rx) = flume::unbounded();
        let thread = Thread {
            supervisor: Arc::clone(self),
            label: worker.task_id.unwrap_or(MAIN_WORKER).to_owned(),
            goal: worker.goal.to_owned(),
            ledger: worker.ledger,
            session_id: worker.session_id,
            retry: worker.retry,
            messages: Vec::new(),
            notes_given: 0,
            sink: EventSender::new(sink, 0),
            sink_rx,
        };
        WorkerLink {
            seen,
            ticks,
            inbox: Arc::clone(&inbox),
            urgent: Arc::default(),
            _thread: smol::spawn(thread.run(rx, inbox)),
        }
    }
}

fn system_prompt() -> String {
    let instructions = std::env::current_dir()
        .map(|cwd| load_instruction_text(&cwd.to_string_lossy()))
        .unwrap_or_default();
    if instructions.trim().is_empty() {
        return SYSTEM.to_owned();
    }
    format!(
        "{SYSTEM}\nThe instruction files workers follow:\n<instructions>{instructions}\n</instructions>"
    )
}

pub(crate) fn tool_definition() -> Value {
    json!({
        "name": TOOL_NAME,
        "description": TOOL_DESCRIPTION,
        "input_schema": {
            "type": "object",
            "properties": {
                TOOL_FIELD: { "type": "string", "description": "Plan, question or status" }
            },
            "required": [TOOL_FIELD],
        },
    })
}

pub(crate) struct Note {
    pub turn: u32,
    pub text: String,
}

#[derive(Default)]
struct Inbox {
    notes: Vec<Note>,
    stop: Option<String>,
}

pub(crate) struct Tick {
    turn: u32,
    text: String,
    /// The worker wrote to the supervisor, so its batch should not wait.
    urgent: bool,
}

/// The worker's end. Dropping it ends the thread, along with any review in
/// flight: nobody would be left to hear the answer.
pub(crate) struct WorkerLink {
    seen: usize,
    ticks: flume::Sender<Tick>,
    inbox: Arc<Mutex<Inbox>>,
    urgent: Arc<AtomicBool>,
    _thread: smol::Task<()>,
}

impl WorkerLink {
    /// Sends what the worker added since the last call. Rendering happens here
    /// rather than in the thread, so no message is cloned to cross over.
    pub(crate) fn observe(&mut self, history: &[Message], turn: u32) {
        let start = self.seen.min(history.len());
        self.seen = history.len();
        let mut text = String::new();
        render::render(&history[start..], &mut text);
        let urgent = self.urgent.swap(false, Ordering::Relaxed);
        if text.is_empty() && !urgent {
            return;
        }
        let _ = self.ticks.send(Tick { turn, text, urgent });
    }

    /// A compaction rewrote history, so the summary is read from the top.
    pub(crate) fn rebase(&mut self) {
        self.seen = 0;
    }

    pub(crate) fn take_notes(&self) -> Vec<Note> {
        mem::take(&mut lock(&self.inbox).notes)
    }

    pub(crate) fn stop_reason(&self) -> Option<String> {
        lock(&self.inbox).stop.clone()
    }

    /// A link whose supervisor has already spoken, for driving the loop.
    #[cfg(test)]
    pub(crate) fn scripted(
        notes: Vec<Note>,
        stop: Option<String>,
    ) -> (Self, flume::Receiver<Tick>) {
        let (ticks, rx) = flume::unbounded();
        let link = Self {
            seen: 1,
            ticks,
            inbox: Arc::new(Mutex::new(Inbox { notes, stop })),
            urgent: Arc::default(),
            _thread: smol::spawn(async {}),
        };
        (link, rx)
    }

    pub(crate) fn tool(&self) -> LocalTool {
        let urgent = Arc::clone(&self.urgent);
        local_tool(ToolAudience::all(), move |_, _| {
            urgent.store(true, Ordering::Relaxed);
            Box::pin(async { Ok(TOOL_ACK.to_owned()) })
        })
    }
}

#[derive(Debug, PartialEq)]
enum Ruling {
    Silent,
    Note(String),
    Stop(String),
}

/// Anything off protocol reads as silence: the supervisor is an opinion,
/// never a precondition.
fn parse(answer: &str) -> Ruling {
    let answer = answer.trim();
    let (word, rest) = answer
        .split_once(char::is_whitespace)
        .unwrap_or((answer, ""));
    let rest = rest.trim();
    if rest.is_empty() {
        return Ruling::Silent;
    }
    match word.trim_end_matches(':') {
        NOTE => Ruling::Note(rest.to_owned()),
        STOP => Ruling::Stop(rest.to_owned()),
        _ => Ruling::Silent,
    }
}

struct Thread {
    supervisor: Arc<Supervisor>,
    label: String,
    goal: String,
    ledger: Arc<RunLedger>,
    session_id: Option<SessionRef>,
    retry: RetryPolicy,
    /// Append only, so every review is a cache hit on all the ones before it.
    messages: Vec<Message>,
    notes_given: u32,
    /// Review traffic must not reach the worker's view. Drained after each
    /// review so it does not pile up.
    sink: EventSender,
    sink_rx: flume::Receiver<Envelope>,
}

impl Thread {
    /// Reviews run one at a time, so ticks landing during one wait in the
    /// channel and join the next batch.
    async fn run(mut self, ticks: flume::Receiver<Tick>, inbox: Arc<Mutex<Inbox>>) {
        let mut pending = String::new();
        let mut first_turn = None;
        let mut last_turn = 0;
        let mut turns = 0;
        while let Ok(tick) = ticks.recv_async().await {
            let mut urgent = false;
            for tick in iter::once(tick).chain(ticks.drain()) {
                first_turn.get_or_insert(tick.turn);
                last_turn = tick.turn;
                turns += 1;
                urgent |= tick.urgent;
                pending.push_str(&tick.text);
            }
            let ripe = urgent
                || turns >= self.supervisor.batch_turns
                || pending.len() / CHARS_PER_TOKEN >= self.supervisor.batch_tokens as usize;
            if !ripe {
                continue;
            }
            let batch = mem::take(&mut pending);
            let first = first_turn.take().unwrap_or(last_turn);
            turns = 0;
            match self.review(&batch, first, last_turn).await {
                Ruling::Silent => {}
                Ruling::Note(text) if self.notes_given < self.supervisor.max_notes => {
                    self.notes_given += 1;
                    lock(&inbox).notes.push(Note {
                        turn: last_turn,
                        text,
                    });
                }
                Ruling::Note(_) => {
                    info!(worker = %self.label, "supervisor note over budget dropped")
                }
                Ruling::Stop(reason) => {
                    lock(&inbox).stop = Some(reason);
                    return;
                }
            }
        }
    }

    async fn review(&mut self, batch: &str, first: u32, last: u32) -> Ruling {
        let supervisor = Arc::clone(&self.supervisor);
        let goal = if self.messages.is_empty() {
            format!("<goal worker=\"{}\">\n{}\n</goal>\n", self.label, self.goal)
        } else {
            String::new()
        };
        let notes_left = supervisor.max_notes.saturating_sub(self.notes_given);
        self.messages.push(Message::user(format!(
            "{goal}<transcript turns=\"{first}-{last}\" notes_left=\"{notes_left}\">\n{batch}</transcript>"
        )));
        let result = {
            let _permit = supervisor.reviews.acquire().await;
            stream_with_retry(
                StreamRequest {
                    provider: &*supervisor.provider,
                    model: &supervisor.model,
                    messages: &self.messages,
                    system: &supervisor.system,
                    tools: &Value::Array(Vec::new()),
                    opts: RequestOptions {
                        thinking: ThinkingConfig::Adaptive,
                        fast: false,
                    },
                    output_budget: REVIEW_OUTPUT_BUDGET,
                    session_id: self.session_id.as_ref(),
                    retry: self.retry,
                },
                None,
                &self.sink,
                &CancelToken::none(),
                &mut None,
                &mut 0,
            )
            .await
        };
        self.sink_rx.drain().for_each(drop);
        let response = match result {
            Ok(response) => response,
            Err(error) => {
                let error = match error {
                    StreamError::Cancelled { .. } => "cancelled".to_owned(),
                    StreamError::Other(e) => e.to_string(),
                };
                warn!(worker = %self.label, %error, "supervisor review failed");
                self.messages.pop();
                return Ruling::Silent;
            }
        };
        let usage = response.usage;
        self.ledger.add(
            usage,
            supervisor.model.billed_cost(&usage, false),
            supervisor.model.list_cost(&usage, false),
        );
        let answer = response
            .message
            .first_text_content()
            .unwrap_or(SILENT)
            .trim()
            .to_owned();
        let ruling = parse(&answer);
        info!(
            worker = %self.label,
            model = %supervisor.model.id,
            first_turn = first,
            last_turn = last,
            batch_bytes = batch.len(),
            input_tokens = usage.input,
            cache_read = usage.cache_read,
            output_tokens = usage.output,
            ruling = ?ruling,
            "supervisor reviewed"
        );
        self.messages.push(Message {
            role: Role::Assistant,
            content: vec![ContentBlock::Text { text: answer }],
            ..Default::default()
        });
        if estimate_message_tokens(&self.messages)
            > supervisor.model.context_window / THREAD_WINDOW_DIVISOR
        {
            self.messages.clear();
        }
        ruling
    }
}

#[cfg(test)]
mod tests {
    use maki_providers::provider::BoxFuture;
    use maki_providers::{AgentError, ProviderEvent, StopReason, StreamResponse, TokenUsage};
    use test_case::test_case;

    use super::*;

    const GOAL: &str = "fix the parser";
    const NOTE_TEXT: &str = "read site/docs/AGENTS.md first";
    const STOP_TEXT: &str = "deleting the repo";

    /// Answers in order, feeding the thread its next tick on each request so
    /// a test can stage reviews one after another without racing the loop.
    struct Scripted {
        answers: Mutex<Vec<Result<String, AgentError>>>,
        then: Mutex<Vec<Tick>>,
        ticks: Mutex<Option<flume::Sender<Tick>>>,
        seen: Mutex<Vec<String>>,
    }

    impl Provider for Scripted {
        fn stream_message<'a>(
            &'a self,
            _: &'a Model,
            messages: &'a [Message],
            _: &'a str,
            _: &'a Value,
            _: &'a flume::Sender<ProviderEvent>,
            _: RequestOptions,
            _: Option<&'a SessionRef>,
        ) -> BoxFuture<'a, Result<StreamResponse, AgentError>> {
            let last = messages.last().and_then(Message::first_text_content);
            lock(&self.seen).push(last.unwrap_or_default().to_owned());
            let mut ticks = lock(&self.ticks);
            match lock(&self.then).pop() {
                Some(tick) => ticks.as_ref().unwrap().send(tick).unwrap(),
                None => *ticks = None,
            }
            let answer = lock(&self.answers).remove(0);
            Box::pin(async move {
                answer.map(|text| StreamResponse {
                    message: Message {
                        role: Role::Assistant,
                        content: vec![ContentBlock::Text { text }],
                        ..Default::default()
                    },
                    usage: TokenUsage::default(),
                    stop_reason: Some(StopReason::EndTurn),
                    upstream: None,
                })
            })
        }

        fn list_models(&self) -> BoxFuture<'_, Result<Vec<maki_providers::ModelInfo>, AgentError>> {
            Box::pin(async { unimplemented!() })
        }
    }

    fn tick(turn: u32, urgent: bool) -> Tick {
        Tick {
            turn,
            text: format!("ASSISTANT: turn {turn}\n"),
            urgent,
        }
    }

    struct Run {
        reviews: Vec<String>,
        inbox: Inbox,
    }

    /// `first` is queued before the loop starts; `then` arrive one per review.
    fn run(
        batch_turns: u32,
        max_notes: u32,
        answers: Vec<Result<String, AgentError>>,
        first: Vec<Tick>,
        mut then: Vec<Tick>,
    ) -> Run {
        let (tx, rx) = flume::unbounded();
        for tick in first {
            tx.send(tick).unwrap();
        }
        then.reverse();
        let provider = Arc::new(Scripted {
            answers: Mutex::new(answers),
            then: Mutex::new(then),
            ticks: Mutex::new(Some(tx)),
            seen: Mutex::default(),
        });
        let supervisor = Arc::new(Supervisor {
            provider: Arc::clone(&provider) as Arc<dyn Provider>,
            model: Model::from_spec("anthropic/claude-sonnet-4-20250514").unwrap(),
            system: SYSTEM.to_owned(),
            batch_turns,
            batch_tokens: u32::MAX,
            max_notes,
            reviews: Semaphore::new(MAX_CONCURRENT_REVIEWS),
        });
        let (sink, sink_rx) = flume::unbounded();
        let thread = Thread {
            supervisor,
            label: MAIN_WORKER.to_owned(),
            goal: GOAL.to_owned(),
            ledger: Arc::default(),
            session_id: None,
            retry: RetryPolicy::default(),
            messages: Vec::new(),
            notes_given: 0,
            sink: EventSender::new(sink, 0),
            sink_rx,
        };
        let inbox = Arc::new(Mutex::new(Inbox::default()));
        if lock(&provider.then).is_empty() {
            lock(&provider.ticks).take();
        }
        smol::block_on(thread.run(rx, Arc::clone(&inbox)));
        let inbox = mem::take(&mut *lock(&inbox));
        Run {
            reviews: mem::take(&mut *lock(&provider.seen)),
            inbox,
        }
    }

    fn ok() -> Result<String, AgentError> {
        Ok(SILENT.to_owned())
    }

    #[test_case(SILENT, Ruling::Silent ; "ok_is_silent")]
    #[test_case("NOTE use the cache", Ruling::Note("use the cache".into()) ; "note")]
    #[test_case("NOTE: use the cache", Ruling::Note("use the cache".into()) ; "note_with_colon")]
    #[test_case("STOP  looping\n", Ruling::Stop("looping".into()) ; "stop_trims")]
    #[test_case("NOTE", Ruling::Silent ; "empty_note_is_silent")]
    #[test_case("Looks good to me", Ruling::Silent ; "prose_is_silent")]
    fn parse_answer(answer: &str, expected: Ruling) {
        assert_eq!(parse(answer), expected);
    }

    #[test_case(2, false, 0 ; "below_threshold_waits")]
    #[test_case(3, false, 1 ; "threshold_reviews")]
    #[test_case(1, true, 1 ; "worker_message_reviews_at_once")]
    fn batching(ticks: u32, urgent: bool, expected: usize) {
        let first = (1..=ticks).map(|turn| tick(turn, urgent)).collect();
        let run = run(3, 1, vec![ok()], first, Vec::new());
        assert_eq!(run.reviews.len(), expected);
    }

    #[test]
    fn ticks_during_a_review_wait_for_the_next() {
        let run = run(
            1,
            1,
            vec![ok(), ok()],
            vec![tick(1, false)],
            vec![tick(2, false)],
        );
        assert_eq!(run.reviews.len(), 2);
        assert!(run.reviews[0].contains(GOAL));
        assert!(run.reviews[0].contains("turns=\"1-1\""));
        assert!(!run.reviews[1].contains(GOAL));
        assert!(run.reviews[1].contains("turns=\"2-2\""));
    }

    #[test]
    fn notes_past_the_budget_are_dropped() {
        let note = || Ok(format!("{NOTE} {NOTE_TEXT}"));
        let run = run(
            1,
            1,
            vec![note(), note()],
            vec![tick(1, false)],
            vec![tick(2, false)],
        );
        assert_eq!(run.inbox.notes.len(), 1);
        assert_eq!(run.inbox.notes[0].turn, 1);
        assert_eq!(run.inbox.notes[0].text, NOTE_TEXT);
    }

    #[test]
    fn stop_ends_the_thread() {
        let run = run(
            1,
            1,
            vec![Ok(format!("{STOP} {STOP_TEXT}")), ok()],
            vec![tick(1, false)],
            vec![tick(2, false)],
        );
        assert_eq!(run.reviews.len(), 1);
        assert_eq!(run.inbox.stop.as_deref(), Some(STOP_TEXT));
    }

    #[test]
    fn failed_review_is_silent_and_resends_the_goal() {
        let run = run(
            1,
            1,
            vec![Err(AgentError::api(400, "bad request")), ok()],
            vec![tick(1, false)],
            vec![tick(2, false)],
        );
        assert!(run.inbox.notes.is_empty() && run.inbox.stop.is_none());
        assert!(run.reviews[1].contains(GOAL));
    }

    fn link() -> (WorkerLink, flume::Receiver<Tick>) {
        WorkerLink::scripted(Vec::new(), None)
    }

    #[test]
    fn observe_sends_only_the_delta() {
        let (mut link, rx) = link();
        let history = vec![Message::user(GOAL.into()), Message::user("next".into())];
        link.observe(&history, 1);
        link.observe(&history, 2);
        let sent: Vec<Tick> = rx.drain().collect();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].text, "USER: next\n");
        assert!(!sent[0].urgent);
    }

    #[test]
    fn tool_call_marks_the_next_tick_urgent() {
        let (mut link, rx) = link();
        let ctx = crate::tools::test_support::stub_ctx(&crate::AgentMode::Build);
        let answer = smol::block_on((link.tool().handler)(json!({TOOL_FIELD: "plan"}), ctx));
        assert_eq!(answer.as_deref(), Ok(TOOL_ACK));
        link.observe(&[Message::user(GOAL.into())], 1);
        assert!(rx.drain().next().unwrap().urgent);
    }
}
