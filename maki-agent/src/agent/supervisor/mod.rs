//! A stronger model checking the work of agents on weaker ones. What it reads
//! is a [`Policy`], composed of parts:
//!
//! - `transcript`: compact batches of the worker's transcript every few turns.
//! - `actions`: every action tool asks for a one-line `why`, so actions arrive
//!   with their explanations.
//! - `report`: at the end of a run, the closing claim, the actions, and
//!   evidence the harness takes itself: the diff of the working tree and the
//!   configured checks.
//!
//! Messages the worker sends (its plan, questions) and the review of a run's
//! end come with every policy.
//!
//! Its output is the expensive side, so silence is its usual answer. No
//! worker waits on it mid-run: a plan or a question goes out through a tool
//! that returns at once, and whatever comes back lands between turns. Only an
//! ending run waits, and only for a bounded time.
//!
//! One supervisor serves a whole session, the main agent and every subagent,
//! each in a thread of its own. The threads share one system prompt, so the
//! provider caches that prefix once for all of them. Each thread leaves an
//! [`episode::Episode`]: the policy, the reviews, and the outcomes, which is
//! what comparing or tuning policies needs.
//!
//! Workers must not depend on it. A review that fails or runs past its
//! deadline reads as silence, and a few in a row take the supervisor offline
//! for a while, so an unreachable model costs a handful of failed calls.

mod episode;
mod evidence;
mod render;

pub(crate) use render::strip_why;

use std::collections::HashMap;
use std::fmt::Write;
use std::iter;
use std::mem;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, Instant};

use async_lock::Semaphore;
use futures_lite::future;
use maki_config::{
    ModelPolicy, SUPERVISOR_POLICY_ACTIONS, SUPERVISOR_POLICY_REPORT, SUPERVISOR_POLICY_TRANSCRIPT,
};
use maki_providers::provider::{self, Provider};
use maki_providers::retry::RetryPolicy;
use maki_providers::{
    ContentBlock, Message, Model, ModelTier, RequestOptions, Role, ThinkingConfig, Timeouts,
    estimate_message_tokens, model_registry,
};
use maki_storage::id::{MakiId, SessionRef};
use serde::Serialize;
use serde_json::{Value, json};
use smol::Timer;
use tracing::{info, warn};

use self::episode::{Episode, Evidence, Kind, Origin, Review, Verdict};
use super::instructions::load_instruction_text;
use super::streaming::{StreamError, StreamRequest, stream_with_retry};
use crate::cancel::CancelToken;
use crate::tools::{LocalTool, ToolAudience, local_tool};
use crate::{AgentConfig, Envelope, EventSender, RunLedger};

pub(crate) const TOOL_NAME: &str = "supervisor";
const TOOL_FIELD: &str = "message";
const TOOL_DESCRIPTION: &str = "Send a message to your supervisor, a stronger model checking your work: your plan of work, a question, or a status. Returns at once and never blocks. A reply, if any, arrives later as a <supervisor> message; no reply means carry on.";
const TOOL_ACK: &str =
    "Sent. Keep working; a reply may arrive later as a <supervisor> message, or none.";
const TOOL_ACK_OFFLINE: &str = "Not sent: the supervisor is unavailable. Carry on without it.";
const TOOL_MISSING_MESSAGE: &str = "`message` is required";
const KICKOFF_PLAN: &str = "A supervisor checks your work. Before acting, send it your plan of work with the `supervisor` tool, adding any questions, then start at once without waiting.";
const KICKOFF_WHY: &str = " Give every action a one-line `why` that ties it to the plan.";
const KICKOFF_REPORT: &str = " End with a message saying what you did and, for anything that departs from the plan, why. The supervisor compares that with your actual changes.";
const KICKOFF_REPLIES: &str = " Its replies arrive as <supervisor> messages: follow them, and do not reply to acknowledge them.";
pub(crate) const HALTED_NOTE: &str = "The supervisor stopped this run:";
pub(crate) const HALTED_CALL: &str = "Not run: the supervisor stopped the run.";
const MAIN_WORKER: &str = "main";
const NO_CLAIM: &str = "(no closing message)";
const ENDED_MARK: &str = "<ended/>\n";
const SYSTEM: &str = include_str!("../../prompts/supervisor.md");
const SILENT: &str = "OK";
const NOTE: &str = "NOTE";
const STOP: &str = "STOP";
/// Room for thinking. The answer itself is one line.
const REVIEW_OUTPUT_BUDGET: u32 = 8_000;
/// A session can run many subagents at once, and each review is a call to the
/// priciest model around.
const MAX_CONCURRENT_REVIEWS: usize = 2;
/// A thread past this share of the window starts over from goal and plan.
const THREAD_WINDOW_DIVISOR: u32 = 2;
/// The chars/4 floor the token estimator uses.
const CHARS_PER_TOKEN: usize = 4;
const CLAIM_CAP: usize = 3_000;
/// The newest actions are kept when a run took more than fit.
const ACTIONS_CAP: usize = 8_000;
const ACTIONS_CUT: &str = "[earlier actions cut]\n";
/// A review slower than this counts as a failure.
const REVIEW_DEADLINE: Duration = Duration::from_secs(120);
/// Failed reviews in a row that take the supervisor offline.
const FAILURES_TO_TRIP: u32 = 3;
const OFFLINE_FOR: Duration = Duration::from_secs(300);

static SUPERVISORS: LazyLock<Mutex<HashMap<MakiId, Weak<Supervisor>>>> =
    LazyLock::new(Mutex::default);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Everything that decides what the supervisor reads and when, in one value,
/// so a run can be recorded against it and it can be varied as a whole.
#[derive(Debug, Default, Serialize)]
pub(crate) struct Policy {
    pub transcript: bool,
    pub why: bool,
    pub report: bool,
    pub batch_turns: u32,
    pub batch_tokens: u32,
    pub max_notes: u32,
    pub exit_wait_secs: u64,
    pub checks: Vec<String>,
}

impl Policy {
    fn from_config(config: &AgentConfig) -> Self {
        let mut policy = Self {
            batch_turns: config.supervisor_batch_turns,
            batch_tokens: config.supervisor_batch_tokens,
            max_notes: config.supervisor_max_notes,
            exit_wait_secs: config.supervisor_exit_wait_secs,
            checks: config.supervisor_checks.clone(),
            ..Self::default()
        };
        for part in &config.supervisor_policy {
            match part.as_str() {
                SUPERVISOR_POLICY_TRANSCRIPT => policy.transcript = true,
                SUPERVISOR_POLICY_ACTIONS => policy.why = true,
                SUPERVISOR_POLICY_REPORT => policy.report = true,
                _ => warn!(%part, "unknown supervisor policy part ignored"),
            }
        }
        policy
    }

    fn kickoff(&self) -> String {
        let mut kickoff = KICKOFF_PLAN.to_owned();
        if self.why {
            kickoff.push_str(KICKOFF_WHY);
        }
        if self.report {
            kickoff.push_str(KICKOFF_REPORT);
        }
        kickoff.push_str(KICKOFF_REPLIES);
        kickoff
    }
}

pub(crate) struct Supervisor {
    provider: Arc<dyn Provider>,
    model: Model,
    system: String,
    policy: Arc<Policy>,
    reviews: Semaphore,
    health: Arc<Mutex<Health>>,
    episodes: Option<PathBuf>,
}

/// Past the trip point every failure restarts the offline window, so a
/// supervisor that is still down after it costs one call per window.
#[derive(Default)]
struct Health {
    failures: u32,
    offline_until: Option<Instant>,
}

impl Health {
    fn online(&self, now: Instant) -> bool {
        self.offline_until.is_none_or(|until| now >= until)
    }

    /// True when this failure took the supervisor offline.
    fn record(&mut self, ok: bool, now: Instant) -> bool {
        if ok {
            *self = Self::default();
            return false;
        }
        self.failures += 1;
        let trips = self.failures >= FAILURES_TO_TRIP;
        if trips {
            self.offline_until = Some(now + OFFLINE_FOR);
        }
        trips
    }
}

fn online(health: &Mutex<Health>) -> bool {
    lock(health).online(Instant::now())
}

/// What a worker tells its supervisor thread about itself.
pub(crate) struct Worker<'a> {
    pub task_id: Option<&'a str>,
    pub goal: &'a str,
    pub model: &'a Model,
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
        let episodes = maki_storage::paths::state_dir()
            .map(|dir| dir.join(episode::EPISODES_DIR).join(episode::EPISODES_FILE))
            .map_err(|error| warn!(%error, "no state dir, supervisor episodes not recorded"))
            .ok();
        Some(Self {
            provider,
            model,
            system: system_prompt(),
            policy: Arc::new(Policy::from_config(config)),
            reviews: Semaphore::new(MAX_CONCURRENT_REVIEWS),
            health: Arc::default(),
            episodes,
        })
    }

    pub(crate) fn online(&self) -> bool {
        online(&self.health)
    }

    fn record(&self, ok: bool) {
        if lock(&self.health).record(ok, Instant::now()) {
            warn!(
                model = %self.model.id,
                offline_secs = OFFLINE_FOR.as_secs(),
                "supervisor offline after repeated failures"
            );
        }
    }

    pub(crate) fn outranks(&self, worker: &Model) -> bool {
        self.model.tier > worker.tier
    }

    pub(crate) fn kickoff(&self) -> String {
        self.policy.kickoff()
    }

    /// `seen` is where the worker's history stands now: what came before it
    /// belongs to earlier runs.
    pub(crate) fn attach(self: &Arc<Self>, worker: Worker<'_>, seen: usize) -> WorkerLink {
        let (events, rx) = flume::unbounded();
        let inbox = Arc::new(Mutex::new(Inbox::default()));
        let (sink, sink_rx) = flume::unbounded();
        let label = worker.task_id.unwrap_or(MAIN_WORKER).to_owned();
        let opening = format!("<goal worker=\"{label}\">\n{}\n</goal>\n", worker.goal);
        let episode = Episode::new(
            Origin {
                session: worker.session_id.as_ref().map(|s| s.as_str().to_owned()),
                worker: label.clone(),
                worker_model: worker.model.spec(),
                supervisor_model: self.model.spec(),
            },
            Arc::clone(&self.policy),
            Arc::clone(&worker.ledger),
            self.episodes.clone(),
        );
        let thread = Thread {
            supervisor: Arc::clone(self),
            policy: Arc::clone(&self.policy),
            label,
            unsent: opening.clone(),
            opening,
            planned: false,
            cwd: std::env::current_dir().unwrap_or_default(),
            ledger: worker.ledger,
            session_id: worker.session_id,
            retry: worker.retry,
            messages: Vec::new(),
            notes_given: 0,
            episode,
            sink: EventSender::new(sink, 0),
            sink_rx,
        };
        WorkerLink {
            seen,
            actions: String::new(),
            policy: Arc::clone(&self.policy),
            events,
            inbox: Arc::clone(&inbox),
            health: Arc::clone(&self.health),
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
    stop: Option<Note>,
}

pub(crate) enum Event {
    /// What a turn added to the transcript.
    Turn { turn: u32, text: String },
    /// Plan, question or status, through the tool.
    Said(String),
    /// The run ended. `report`, when the policy has one, holds the claim and
    /// the actions, and the thread adds the evidence. `settled` is dropped
    /// once the ruling is in the inbox, or once no ruling will come.
    Ended {
        report: Option<String>,
        turn: u32,
        settled: flume::Sender<()>,
    },
}

/// The worker's end. Dropping it ends the thread, along with any review in
/// flight: nobody would be left to hear the answer.
pub(crate) struct WorkerLink {
    seen: usize,
    /// Rendered since the last report.
    actions: String,
    policy: Arc<Policy>,
    events: flume::Sender<Event>,
    inbox: Arc<Mutex<Inbox>>,
    health: Arc<Mutex<Health>>,
    _thread: smol::Task<()>,
}

impl WorkerLink {
    /// Takes in what the worker added since the last call, as each part of
    /// the policy wants it.
    pub(crate) fn observe(&mut self, history: &[Message], turn: u32) {
        let start = self.seen.min(history.len());
        self.seen = history.len();
        let added = &history[start..];
        if self.policy.report {
            render::render_actions(added, turn, self.policy.why, &mut self.actions);
        }
        if self.policy.transcript {
            let mut text = String::new();
            render::render_transcript(added, &mut text);
            if !text.is_empty() {
                let _ = self.events.send(Event::Turn { turn, text });
            }
        }
    }

    /// A compaction rewrote history, and what it summarized was observed.
    pub(crate) fn rebase(&mut self, seen: usize) {
        self.seen = seen;
    }

    pub(crate) fn requires_why(&self) -> bool {
        self.policy.why
    }

    pub(crate) fn exit_wait(&self) -> Duration {
        Duration::from_secs(self.policy.exit_wait_secs)
    }

    /// Makes every action tool in `tools` ask for its `why` when the policy
    /// wants one, and offers the supervisor tool.
    pub(crate) fn supervise_tools(&self, tools: &mut Value) {
        if self.policy.why {
            render::require_why(tools);
        }
        if let Value::Array(definitions) = tools {
            definitions.push(tool_definition());
        }
    }

    /// Reports the run's end, with its closing claim and the actions since
    /// the last report when the policy has a report. The receiver disconnects
    /// when the ruling is in, at once if the thread has ended.
    pub(crate) fn exit(&mut self, history: &[Message], turn: u32) -> flume::Receiver<()> {
        let report = self.policy.report.then(|| {
            let claim = history
                .iter()
                .rev()
                .find(|message| matches!(message.role, Role::Assistant))
                .and_then(Message::first_text_content)
                .map_or(NO_CLAIM, str::trim);
            let actions = cut_front(mem::take(&mut self.actions), ACTIONS_CAP);
            format!(
                "<report>\n<claim>\n{}\n</claim>\n<actions>\n{actions}</actions>\n",
                render::clip(claim, CLAIM_CAP)
            )
        });
        let (settled, rx) = flume::bounded(0);
        let _ = self.events.send(Event::Ended {
            report,
            turn,
            settled,
        });
        rx
    }

    pub(crate) fn take_notes(&self) -> Vec<Note> {
        mem::take(&mut lock(&self.inbox).notes)
    }

    pub(crate) fn stop_reason(&self) -> Option<String> {
        lock(&self.inbox)
            .stop
            .as_ref()
            .map(|stop| stop.text.clone())
    }

    /// What arrived after the run stopped listening, for the next run. A stop
    /// that late has no run left to stop, so it travels as a note.
    pub(crate) fn into_leftovers(self) -> Vec<Note> {
        let mut inbox = lock(&self.inbox);
        let stop = inbox.stop.take();
        mem::take(&mut inbox.notes)
            .into_iter()
            .chain(stop)
            .collect()
    }

    /// A link whose supervisor has already spoken, for driving the loop.
    #[cfg(test)]
    pub(crate) fn scripted(notes: Vec<Note>, stop: Option<Note>) -> (Self, flume::Receiver<Event>) {
        Self::scripted_with(Policy::default(), notes, stop)
    }

    #[cfg(test)]
    pub(crate) fn scripted_with(
        policy: Policy,
        notes: Vec<Note>,
        stop: Option<Note>,
    ) -> (Self, flume::Receiver<Event>) {
        let (events, rx) = flume::unbounded();
        let link = Self {
            seen: 1,
            actions: String::new(),
            policy: Arc::new(policy),
            events,
            inbox: Arc::new(Mutex::new(Inbox { notes, stop })),
            health: Arc::default(),
            _thread: smol::spawn(async {}),
        };
        (link, rx)
    }

    pub(crate) fn tool(&self) -> LocalTool {
        let events = self.events.clone();
        let health = Arc::clone(&self.health);
        local_tool(ToolAudience::all(), move |input, _| {
            let answer = match input.get(TOOL_FIELD).and_then(Value::as_str) {
                None => Err(TOOL_MISSING_MESSAGE.to_owned()),
                Some(_) if !online(&health) => Ok(TOOL_ACK_OFFLINE.to_owned()),
                Some(message) => {
                    let _ = events.send(Event::Said(message.trim().to_owned()));
                    Ok(TOOL_ACK.to_owned())
                }
            };
            Box::pin(async move { answer })
        })
    }
}

fn cut_front(text: String, cap: usize) -> String {
    if text.len() <= cap {
        return text;
    }
    let from = text.ceil_char_boundary(text.len() - cap);
    let from = text[from..]
        .find('\n')
        .map_or(text.len(), |at| from + at + 1);
    format!("{ACTIONS_CUT}{}", &text[from..])
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

/// The transcript waiting for its batch to fill.
#[derive(Default)]
struct Pending {
    text: String,
    first_turn: Option<u32>,
    turns: u32,
}

struct Thread {
    supervisor: Arc<Supervisor>,
    policy: Arc<Policy>,
    label: String,
    /// Goal and plan, what a thread starting over must read again.
    opening: String,
    planned: bool,
    /// What the supervisor has not read yet. A review that fails or finds it
    /// offline leaves its input here for the next one, so a plan is never
    /// lost to a bad moment.
    unsent: String,
    cwd: PathBuf,
    ledger: Arc<RunLedger>,
    session_id: Option<SessionRef>,
    retry: RetryPolicy,
    /// Append only, so every review is a cache hit on all the ones before it.
    messages: Vec<Message>,
    notes_given: u32,
    episode: Episode,
    /// Review traffic must not reach the worker's view. Drained after each
    /// review so it does not pile up.
    sink: EventSender,
    sink_rx: flume::Receiver<Envelope>,
}

impl Thread {
    /// The baseline is taken first, while the worker is still waiting on its
    /// first answer. Reviews run one at a time, so events landing during one
    /// wait in the channel and join the next.
    async fn run(mut self, events: flume::Receiver<Event>, inbox: Arc<Mutex<Inbox>>) {
        let baseline = if self.policy.report {
            evidence::snapshot(self.cwd.clone()).await
        } else {
            None
        };
        let mut pending = Pending::default();
        let mut turn = 0;
        while let Ok(event) = events.recv_async().await {
            let mut said = String::new();
            let mut report = None;
            let mut ended = false;
            // Held to the end of the pass, which is what tells a waiting
            // worker the ruling is in.
            let mut settled = Vec::new();
            for event in iter::once(event).chain(events.drain()) {
                match event {
                    Event::Turn { turn: at, text } => {
                        pending.first_turn.get_or_insert(at);
                        pending.turns += 1;
                        pending.text.push_str(&text);
                        turn = at;
                    }
                    Event::Said(text) => {
                        let message = format!("<worker_message>\n{text}\n</worker_message>\n");
                        if !mem::replace(&mut self.planned, true) {
                            self.opening.push_str(&message);
                        }
                        said.push_str(&message);
                    }
                    Event::Ended {
                        report: claim,
                        turn: at,
                        settled: done,
                    } => {
                        report = claim;
                        ended = true;
                        turn = at;
                        settled.push(done);
                    }
                }
            }
            let ripe = ended
                || !said.is_empty()
                || pending.turns >= self.policy.batch_turns
                || pending.text.len() / CHARS_PER_TOKEN >= self.policy.batch_tokens as usize;
            if !ripe {
                continue;
            }
            let mut input = String::new();
            let batch = mem::take(&mut pending);
            if !batch.text.is_empty() {
                let first = batch.first_turn.unwrap_or(turn);
                let _ = write!(
                    input,
                    "<transcript turns=\"{first}-{turn}\">\n{}</transcript>\n",
                    batch.text
                );
            }
            input.push_str(&said);
            if ended {
                self.episode.ended = true;
                match report {
                    Some(report) => {
                        input.push_str(&report);
                        if self.supervisor.online() {
                            input.push_str(&self.evidence(baseline.clone(), turn).await);
                        }
                        input.push_str("</report>\n");
                    }
                    None if !input.is_empty() => input.push_str(ENDED_MARK),
                    None => {}
                }
            }
            if input.is_empty() {
                continue;
            }
            let kind = if ended {
                Kind::Exit
            } else if said.is_empty() {
                Kind::Batch
            } else {
                Kind::Message
            };
            match self.review(input, kind, turn).await {
                Ruling::Silent => {}
                Ruling::Note(text) if self.notes_given < self.policy.max_notes => {
                    self.notes_given += 1;
                    lock(&inbox).notes.push(Note { turn, text });
                }
                Ruling::Note(_) => {
                    info!(worker = %self.label, "supervisor note over budget dropped")
                }
                Ruling::Stop(text) => {
                    lock(&inbox).stop = Some(Note { turn, text });
                    return;
                }
            }
        }
    }

    async fn evidence(&mut self, baseline: Option<String>, turn: u32) -> String {
        let diff = evidence::diff(self.cwd.clone(), baseline).await;
        let mut out = diff.text;
        let check_exits = evidence::checks(&self.cwd, &self.policy.checks, &mut out).await;
        self.episode.reports.push(Evidence {
            turn,
            changed: diff.changed,
            check_exits,
        });
        out
    }

    /// Offline, nothing is sent and nothing is recorded against the
    /// supervisor's health, so the offline window is not extended by workers
    /// that merely waited.
    async fn review(&mut self, input: String, kind: Kind, turn: u32) -> Ruling {
        let supervisor = Arc::clone(&self.supervisor);
        let _permit = supervisor.reviews.acquire().await;
        let started = Instant::now();
        let body = mem::take(&mut self.unsent) + &input;
        if !supervisor.online() {
            self.unsent = body;
            self.episode
                .reviews
                .push(Review::new(kind, Verdict::Offline, turn, started));
            return Ruling::Silent;
        }
        let notes_left = self.policy.max_notes.saturating_sub(self.notes_given);
        self.messages.push(Message::user(format!(
            "{body}<notes_left>{notes_left}</notes_left>"
        )));
        let streamed = future::or(
            async {
                Some(
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
                    .await,
                )
            },
            async {
                Timer::after(REVIEW_DEADLINE).await;
                None
            },
        )
        .await;
        self.sink_rx.drain().for_each(drop);
        let outcome = match streamed {
            Some(Ok(response)) => Ok(response),
            Some(Err(StreamError::Cancelled { .. })) => Err("cancelled".to_owned()),
            Some(Err(StreamError::Other(e))) => Err(e.to_string()),
            None => Err(format!("no answer in {}s", REVIEW_DEADLINE.as_secs())),
        };
        supervisor.record(outcome.is_ok());
        let response = match outcome {
            Ok(response) => response,
            Err(error) => {
                warn!(worker = %self.label, %error, "supervisor review failed");
                self.messages.pop();
                self.unsent = body;
                self.episode
                    .reviews
                    .push(Review::new(kind, Verdict::Failed, turn, started));
                return Ruling::Silent;
            }
        };
        let usage = response.usage;
        let cost = supervisor.model.billed_cost(&usage, false);
        self.ledger
            .add(usage, cost, supervisor.model.list_cost(&usage, false));
        let answer = response
            .message
            .first_text_content()
            .unwrap_or(SILENT)
            .trim()
            .to_owned();
        let ruling = parse(&answer);
        let verdict = match ruling {
            Ruling::Silent => Verdict::Ok,
            Ruling::Note(_) => Verdict::Note,
            Ruling::Stop(_) => Verdict::Stop,
        };
        let mut review = Review::new(kind, verdict, turn, started);
        review.usage = usage;
        review.cost = cost;
        self.episode.reviews.push(review);
        info!(
            worker = %self.label,
            model = %supervisor.model.id,
            input_bytes = body.len(),
            input_tokens = usage.input,
            cache_read = usage.cache_read,
            output_tokens = usage.output,
            elapsed_ms = started.elapsed().as_millis() as u64,
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
            self.unsent = self.opening.clone();
        }
        ruling
    }
}

#[cfg(test)]
mod tests {
    use maki_providers::provider::BoxFuture;
    use maki_providers::{AgentError, ProviderEvent, StopReason, StreamResponse, TokenUsage};
    use tempfile::TempDir;
    use test_case::test_case;

    use super::*;

    const GOAL: &str = "fix the parser";
    const PLAN: &str = "1. reproduce 2. fix lexer 3. run tests";
    const QUESTION: &str = "keep the old API?";
    const CLAIM: &str = "Fixed the lexer; tests pass.";
    const NOTE_TEXT: &str = "read site/docs/AGENTS.md first";
    const STOP_TEXT: &str = "deleting the repo";
    const CHECK: &str = "echo checked; exit 3";
    const BATCH_TURNS: u32 = 3;

    /// Answers in order, feeding the thread its next event on each request
    /// so a test can stage reviews one after another without racing the
    /// loop.
    struct Scripted {
        answers: Mutex<Vec<Result<String, AgentError>>>,
        then: Mutex<Vec<Event>>,
        events: Mutex<Option<flume::Sender<Event>>>,
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
            let mut events = lock(&self.events);
            let mut then = lock(&self.then);
            if let Some(event) = then.pop() {
                events.as_ref().unwrap().send(event).unwrap();
            }
            if then.is_empty() {
                *events = None;
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

    fn said(text: &str) -> Event {
        Event::Said(text.to_owned())
    }

    fn turn(at: u32) -> Event {
        Event::Turn {
            turn: at,
            text: format!("ASSISTANT: turn {at}\n"),
        }
    }

    fn ended(report: bool) -> (Event, flume::Receiver<()>) {
        let (settled, rx) = flume::bounded(0);
        let report = report
            .then(|| format!("<report>\n<claim>\n{CLAIM}\n</claim>\n<actions>\n</actions>\n"));
        let event = Event::Ended {
            report,
            turn: 4,
            settled,
        };
        (event, rx)
    }

    fn offline() -> Health {
        Health {
            failures: FAILURES_TO_TRIP,
            offline_until: Some(Instant::now() + OFFLINE_FOR),
        }
    }

    fn ok() -> Result<String, AgentError> {
        Ok(SILENT.to_owned())
    }

    fn failure() -> Result<String, AgentError> {
        Err(AgentError::api(400, "bad request"))
    }

    fn policy() -> Policy {
        Policy {
            batch_turns: BATCH_TURNS,
            batch_tokens: u32::MAX,
            max_notes: 1,
            ..Policy::default()
        }
    }

    struct Script {
        policy: Policy,
        health: Health,
        answers: Vec<Result<String, AgentError>>,
        /// Queued before the loop starts.
        first: Vec<Event>,
        /// Arrive one per review.
        then: Vec<Event>,
    }

    impl Default for Script {
        fn default() -> Self {
            Self {
                policy: policy(),
                health: Health::default(),
                answers: Vec::new(),
                first: Vec::new(),
                then: Vec::new(),
            }
        }
    }

    struct Run {
        reviews: Vec<String>,
        inbox: Inbox,
    }

    fn run(script: Script) -> Run {
        let (tx, rx) = flume::unbounded();
        for event in script.first {
            tx.send(event).unwrap();
        }
        let mut then = script.then;
        then.reverse();
        let provider = Arc::new(Scripted {
            answers: Mutex::new(script.answers),
            then: Mutex::new(then),
            events: Mutex::new(Some(tx)),
            seen: Mutex::default(),
        });
        let policy = Arc::new(script.policy);
        let supervisor = Arc::new(Supervisor {
            provider: Arc::clone(&provider) as Arc<dyn Provider>,
            model: Model::from_spec("anthropic/claude-sonnet-4-20250514").unwrap(),
            system: SYSTEM.to_owned(),
            policy: Arc::clone(&policy),
            reviews: Semaphore::new(MAX_CONCURRENT_REVIEWS),
            health: Arc::new(Mutex::new(script.health)),
            episodes: None,
        });
        let cwd = TempDir::new().unwrap();
        let (sink, sink_rx) = flume::unbounded();
        let opening = format!("<goal worker=\"{MAIN_WORKER}\">\n{GOAL}\n</goal>\n");
        let origin = Origin {
            session: None,
            worker: MAIN_WORKER.into(),
            worker_model: String::new(),
            supervisor_model: String::new(),
        };
        let thread = Thread {
            supervisor,
            policy: Arc::clone(&policy),
            label: MAIN_WORKER.to_owned(),
            unsent: opening.clone(),
            opening,
            planned: false,
            cwd: cwd.path().to_path_buf(),
            ledger: Arc::default(),
            session_id: None,
            retry: RetryPolicy::default(),
            messages: Vec::new(),
            notes_given: 0,
            episode: Episode::new(origin, policy, Arc::default(), None),
            sink: EventSender::new(sink, 0),
            sink_rx,
        };
        let inbox = Arc::new(Mutex::new(Inbox::default()));
        if lock(&provider.then).is_empty() {
            lock(&provider.events).take();
        }
        smol::block_on(thread.run(rx, Arc::clone(&inbox)));
        let inbox = mem::take(&mut *lock(&inbox));
        Run {
            reviews: mem::take(&mut *lock(&provider.seen)),
            inbox,
        }
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

    #[test_case(&[], false, false, false ; "nothing")]
    #[test_case(&[SUPERVISOR_POLICY_TRANSCRIPT], true, false, false ; "transcript")]
    #[test_case(&[SUPERVISOR_POLICY_ACTIONS, SUPERVISOR_POLICY_REPORT], false, true, true ; "actions_and_report")]
    #[test_case(&["bogus", SUPERVISOR_POLICY_REPORT], false, false, true ; "unknown_part_ignored")]
    fn policy_parts_compose(parts: &[&str], transcript: bool, why: bool, report: bool) {
        let config = AgentConfig {
            supervisor_policy: parts.iter().map(|part| (*part).to_owned()).collect(),
            ..AgentConfig::default()
        };
        let policy = Policy::from_config(&config);
        assert_eq!(
            (policy.transcript, policy.why, policy.report),
            (transcript, why, report)
        );
    }

    #[test_case(false, false ; "plan_only")]
    #[test_case(true, true ; "why_and_report")]
    fn kickoff_asks_for_what_the_policy_reads(why: bool, report: bool) {
        let kickoff = Policy {
            why,
            report,
            ..Policy::default()
        }
        .kickoff();
        assert!(kickoff.starts_with(KICKOFF_PLAN) && kickoff.ends_with(KICKOFF_REPLIES));
        assert_eq!(kickoff.contains(KICKOFF_WHY), why);
        assert_eq!(kickoff.contains(KICKOFF_REPORT), report);
    }

    #[test]
    fn plan_is_reviewed_at_once_with_the_goal() {
        let run = run(Script {
            answers: vec![ok()],
            first: vec![said(PLAN)],
            ..Default::default()
        });
        assert_eq!(run.reviews.len(), 1);
        assert!(run.reviews[0].contains(GOAL) && run.reviews[0].contains(PLAN));
    }

    #[test_case(BATCH_TURNS - 1, 0 ; "below_threshold_waits")]
    #[test_case(BATCH_TURNS, 1 ; "threshold_reviews")]
    fn transcript_batches_fill_before_review(turns: u32, expected: usize) {
        let run = run(Script {
            answers: vec![ok()],
            first: (1..=turns).map(turn).collect(),
            ..Default::default()
        });
        assert_eq!(run.reviews.len(), expected);
    }

    #[test]
    fn message_takes_the_waiting_transcript_along() {
        let run = run(Script {
            answers: vec![ok()],
            first: vec![turn(1), said(QUESTION)],
            ..Default::default()
        });
        assert!(run.reviews[0].contains("<transcript turns=\"1-1\">"));
        assert!(run.reviews[0].contains(QUESTION));
    }

    #[test]
    fn messages_during_a_review_join_the_next_without_the_goal() {
        let run = run(Script {
            answers: vec![ok(), ok()],
            first: vec![said(PLAN)],
            then: vec![said(QUESTION)],
            ..Default::default()
        });
        assert_eq!(run.reviews.len(), 2);
        assert!(!run.reviews[1].contains(GOAL));
        assert!(run.reviews[1].contains(QUESTION));
    }

    #[test]
    fn report_carries_claim_evidence_and_checks() {
        let (event, settled) = ended(true);
        let run = run(Script {
            policy: Policy {
                report: true,
                checks: vec![CHECK.to_owned()],
                ..policy()
            },
            answers: vec![ok()],
            first: vec![event],
            ..Default::default()
        });
        let review = &run.reviews[0];
        assert!(review.contains(CLAIM));
        assert!(review.contains("<diff>unavailable"));
        assert!(review.contains(&format!("<check exit=\"3\">\n$ {CHECK}\nchecked\n")));
        assert!(review.contains("</report>"));
        assert!(settled.is_disconnected());
    }

    #[test]
    fn transcript_end_flushes_the_tail_marked_ended() {
        let (event, settled) = ended(false);
        let run = run(Script {
            answers: vec![ok()],
            first: vec![turn(1), event],
            ..Default::default()
        });
        assert!(run.reviews[0].contains("ASSISTANT: turn 1"));
        assert!(run.reviews[0].ends_with(&format!("{ENDED_MARK}<notes_left>1</notes_left>")));
        assert!(settled.is_disconnected());
    }

    #[test]
    fn end_with_nothing_new_costs_no_review() {
        let (event, settled) = ended(false);
        let run = run(Script {
            first: vec![event],
            ..Default::default()
        });
        assert!(run.reviews.is_empty());
        assert!(settled.is_disconnected());
    }

    #[test]
    fn failed_review_keeps_its_input_for_the_next() {
        let run = run(Script {
            answers: vec![failure(), ok()],
            first: vec![said(PLAN)],
            then: vec![said(QUESTION)],
            ..Default::default()
        });
        assert!(run.inbox.notes.is_empty() && run.inbox.stop.is_none());
        let retried = &run.reviews[1];
        assert_eq!(retried.matches(GOAL).count(), 1);
        assert!(retried.contains(PLAN) && retried.contains(QUESTION));
    }

    #[test]
    fn notes_past_the_budget_are_dropped() {
        let note = || Ok(format!("{NOTE} {NOTE_TEXT}"));
        let (event, _settled) = ended(true);
        let run = run(Script {
            answers: vec![note(), note()],
            first: vec![said(PLAN)],
            then: vec![event],
            ..Default::default()
        });
        assert_eq!(run.inbox.notes.len(), 1);
        assert_eq!(run.inbox.notes[0].text, NOTE_TEXT);
    }

    #[test]
    fn note_on_a_report_names_its_turn() {
        let (event, _settled) = ended(true);
        let run = run(Script {
            answers: vec![Ok(format!("{NOTE} {NOTE_TEXT}"))],
            first: vec![event],
            ..Default::default()
        });
        assert_eq!(run.inbox.notes[0].turn, 4);
    }

    #[test]
    fn stop_ends_the_thread() {
        let run = run(Script {
            answers: vec![Ok(format!("{STOP} {STOP_TEXT}")), ok()],
            first: vec![said(PLAN)],
            then: vec![said(QUESTION)],
            ..Default::default()
        });
        assert_eq!(run.reviews.len(), 1);
        assert_eq!(run.inbox.stop.unwrap().text, STOP_TEXT);
    }

    #[test]
    fn offline_supervisor_reads_nothing_and_releases_the_worker() {
        let (event, settled) = ended(true);
        let run = run(Script {
            policy: Policy {
                report: true,
                checks: vec![CHECK.to_owned()],
                ..policy()
            },
            health: offline(),
            first: vec![said(PLAN), event],
            ..Default::default()
        });
        assert!(run.reviews.is_empty());
        assert!(settled.is_disconnected());
    }

    #[test]
    fn repeated_failures_take_it_offline() {
        let then = (1..=FAILURES_TO_TRIP).map(|_| said(QUESTION)).collect();
        let run = run(Script {
            answers: (0..FAILURES_TO_TRIP).map(|_| failure()).collect(),
            first: vec![said(PLAN)],
            then,
            ..Default::default()
        });
        assert_eq!(run.reviews.len(), FAILURES_TO_TRIP as usize);
    }

    #[test_case(FAILURES_TO_TRIP - 1, false, true ; "below_trip_stays_online")]
    #[test_case(FAILURES_TO_TRIP, false, false ; "trip_goes_offline")]
    #[test_case(FAILURES_TO_TRIP, true, true ; "success_resets")]
    fn health_trips_on_consecutive_failures(failures: u32, then_ok: bool, online: bool) {
        let now = Instant::now();
        let mut health = Health::default();
        for _ in 0..failures {
            health.record(false, now);
        }
        if then_ok {
            health.record(true, now);
        }
        assert_eq!(health.online(now), online);
    }

    #[test]
    fn one_failure_after_the_window_goes_offline_again() {
        let mut health = offline();
        let later = Instant::now() + OFFLINE_FOR * 2;
        assert!(health.online(later));
        assert!(health.record(false, later));
        assert!(!health.online(later));
    }

    fn link(policy: Policy) -> (WorkerLink, flume::Receiver<Event>) {
        WorkerLink::scripted_with(policy, Vec::new(), None)
    }

    fn call_tool(link: &WorkerLink, input: Value) -> Result<String, String> {
        let ctx = crate::tools::test_support::stub_ctx(&crate::AgentMode::Build);
        smol::block_on((link.tool().handler)(input, ctx))
    }

    #[test]
    fn tool_sends_the_message_itself() {
        let (link, rx) = link(policy());
        let answer = call_tool(&link, json!({TOOL_FIELD: format!(" {PLAN}\n")}));
        assert_eq!(answer.as_deref(), Ok(TOOL_ACK));
        assert!(matches!(rx.try_recv(), Ok(Event::Said(text)) if text == PLAN));
    }

    #[test]
    fn offline_tool_says_so_and_sends_nothing() {
        let (link, rx) = link(policy());
        *lock(&link.health) = offline();
        let answer = call_tool(&link, json!({TOOL_FIELD: PLAN}));
        assert_eq!(answer.as_deref(), Ok(TOOL_ACK_OFFLINE));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn tool_without_a_message_is_an_error() {
        let (link, _rx) = link(policy());
        assert_eq!(
            call_tool(&link, json!({})),
            Err(TOOL_MISSING_MESSAGE.to_owned())
        );
    }

    fn worked() -> Vec<Message> {
        vec![
            Message::user(GOAL.into()),
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: "t1".into(),
                    name: "bash".into(),
                    input: json!({"command": "cargo test", render::WHY_FIELD: "step 3"}),
                    thought_signature: None,
                }],
                ..Default::default()
            },
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::Text { text: CLAIM.into() }],
                ..Default::default()
            },
        ]
    }

    fn reports(rx: &flume::Receiver<Event>) -> Vec<Option<String>> {
        rx.drain()
            .filter_map(|event| match event {
                Event::Ended { report, .. } => Some(report),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn exit_reports_the_claim_and_new_actions_once() {
        let (mut link, rx) = link(Policy {
            why: true,
            report: true,
            ..policy()
        });
        let history = worked();
        link.observe(&history, 2);
        let _first = link.exit(&history, 2);
        let _second = link.exit(&history, 2);
        let reports = reports(&rx);
        let first = reports[0].as_deref().unwrap();
        assert!(first.contains(CLAIM));
        assert!(first.contains("t2 bash {\"command\":\"cargo test\"}\n  why: step 3"));
        assert!(!reports[1].as_deref().unwrap().contains("bash"));
    }

    #[test]
    fn transcript_only_link_sends_turns_and_a_bare_end() {
        let (mut link, rx) = link(Policy {
            transcript: true,
            ..policy()
        });
        let history = worked();
        link.observe(&history, 2);
        assert!(matches!(rx.try_recv(), Ok(Event::Turn { turn: 2, .. })));
        let _settled = link.exit(&history, 2);
        assert_eq!(reports(&rx), [None]);
    }

    #[test]
    fn exit_on_an_ended_thread_releases_at_once() {
        let (mut link, rx) = link(policy());
        drop(rx);
        assert!(link.exit(&[], 1).is_disconnected());
    }

    #[test]
    fn late_stop_travels_as_a_note() {
        let note = Note {
            turn: 1,
            text: NOTE_TEXT.into(),
        };
        let stop = Note {
            turn: 2,
            text: STOP_TEXT.into(),
        };
        let (link, _rx) = WorkerLink::scripted(vec![note], Some(stop));
        let texts: Vec<String> = link.into_leftovers().into_iter().map(|n| n.text).collect();
        assert_eq!(texts, [NOTE_TEXT, STOP_TEXT]);
    }

    #[test]
    fn overlong_actions_keep_the_newest_whole_lines() {
        let text: String = (0..100).map(|n| format!("action {n}\n")).collect();
        let cut = cut_front(text, 50);
        assert!(cut.starts_with(ACTIONS_CUT));
        assert!(cut.ends_with("action 99\n"));
        assert!(cut.len() <= 50 + ACTIONS_CUT.len());
        assert!(cut[ACTIONS_CUT.len()..].starts_with("action "));
    }
}
