use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use crate::app::tasks::TaskOutcome;
use crate::chat::{Chat, DONE_TEXT, history_to_display};
use crate::components::rewind_picker::{RewindEntry, display_msg_index_for_turn};
use crate::components::settings_picker::UserSettings;
use crate::components::{Action, LoadedSession};
use maki_agent::ToolOutput;
use maki_agent::agent::estimate_message_tokens;
use maki_providers::{ContentBlock, Message, Model, Role, TokenUsage};
use maki_storage::id::MakiId;
use maki_storage::sessions::{SessionMeta, StoredSubagent};

use crate::AppSession;

use super::session_state::{SessionState, rules_to_stored, stored_to_rules};
use super::{App, Mode, PendingInput, PlanState, Status};

/// The shortest gap between two writes that carry only UI state.
const SOFT_SAVE_DELAY: Duration = Duration::from_millis(1000);

/// What `App::checkpoint` last handed to the writer: which session, how far
/// along it was, and when. The id is part of it because a session swapped into
/// the tab starts its revisions back at zero and would otherwise look older
/// than the stamp left by the one it replaced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Sent {
    pub id: MakiId,
    pub revision: u64,
    pub content_revision: u64,
    pub at: Instant,
}

/// The one content check: `App::checkpoint` saves a session only when this
/// holds, and the shutdown report reuses it to say which tabs were saved, so
/// the report and the disk can never disagree.
pub(crate) fn session_has_content(session: &AppSession) -> bool {
    !session.messages().is_empty()
        || session.meta.input_draft.is_some()
        || !session.meta.queued_messages.is_empty()
        || session.meta.mode != Some(maki_storage::sessions::StoredMode::Build)
}

impl App {
    pub(crate) fn has_content(&self) -> bool {
        session_has_content(&self.state.session)
    }

    /// The event loop runs this once per frame per session. It syncs whatever
    /// the session mirrors from live state, then writes only if a mutator
    /// really changed something. No dirty flags and no per-event save calls,
    /// so there is nothing left to forget.
    pub(crate) fn checkpoint(&mut self) {
        self.checkpoint_with(SOFT_SAVE_DELAY);
    }

    /// A checkpoint for the paths that get no later frame, so a draft typed a
    /// keystroke ago still reaches disk: shutdown, and swapping the session out
    /// from under the tab.
    pub(crate) fn checkpoint_now(&mut self) {
        self.checkpoint_with(Duration::ZERO);
    }

    pub(super) fn checkpoint_with(&mut self, soft_delay: Duration) {
        let snapshot = self.shared_history.as_ref().map(|h| h.load_full());
        let meta = self.build_meta();
        AppSession::checkpoint(
            &mut self.state.session,
            snapshot.as_deref(),
            meta,
            self.state.token_usage,
        );
        self.publish_session_name();
        if !self.has_content() {
            // A draft typed and then deleted is already on disk, and a file with
            // nothing in it is a session the picker still offers to resume. Idle
            // only: submitting empties the draft a frame before the agent mirrors
            // the prompt back, and that gap is not an abandoned session.
            let id = self.state.session.id;
            if self.status == Status::Idle && self.last_sent.take_if(|last| last.id == id).is_some()
            {
                self.storage_writer.delete(id, |_| {});
            }
            return;
        }
        let session = &self.state.session;
        let sent = Sent {
            id: session.id,
            revision: session.revision(),
            content_revision: session.content_revision(),
            at: Instant::now(),
        };
        if let Some(last) = &self.last_sent
            && last.id == sent.id
        {
            if last.revision == sent.revision {
                return;
            }
            // Only UI state moved: a keystroke in the draft, the queue or a
            // session rule. Each one costs a meta record plus an fsync, so they
            // land at most once per `soft_delay`, which bounds what a crash
            // takes with it. Anything the agent produced skips the wait.
            if last.content_revision == sent.content_revision && last.at.elapsed() < soft_delay {
                return;
            }
        }

        self.storage_writer.send(Arc::clone(&self.state.session));
        self.last_sent = Some(sent);
    }

    /// Mirrors the session's name outwards: to the config global the status
    /// line reads, and to the friendly symlink beside the API log.
    ///
    /// Both are pure functions of the session id and title, and a checkpoint
    /// runs every frame, so they are gated on the name actually moving. The
    /// symlink side in particular reaches the filesystem — a `create_dir_all`
    /// and a `stat` at minimum, an unlink and a `symlink` once a log exists —
    /// and none of that belongs on a 16ms tick that usually has nothing to say.
    fn publish_session_name(&mut self) {
        let session = &self.state.session;
        if self
            .published_name
            .as_ref()
            .is_some_and(|(id, title)| *id == session.id && title == &session.title)
        {
            return;
        }
        *maki_config::CURRENT_SESSION_NAME.lock().unwrap() = Some(session.title.clone());
        maki_providers::update_api_log_symlink(
            &session.id.to_string(),
            self.published_name
                .as_ref()
                .filter(|(id, _)| *id == session.id)
                .map(|(_, title)| title.as_str()),
            &session.title,
            session.created_at,
        );
        self.published_name = Some((session.id, session.title.clone()));
    }

    /// The draft as `SessionMeta` wants it, rebuilt only when the buffer moves.
    ///
    /// `TextBuffer::value` joins every line into a fresh `String`, and a
    /// checkpoint runs every frame, so a long pasted prompt was re-joined and
    /// then compared against the stored one 60 times a second while the agent
    /// streamed — only to be thrown away, since nothing had changed. Handing
    /// back the same `Arc` reduces that to a refcount bump, and lets the meta
    /// comparison downstream settle on a pointer check.
    fn draft(&mut self) -> Option<Arc<str>> {
        let revision = self.input_box.buffer.revision();
        if let Some((built_at, draft)) = &self.draft_mirror
            && *built_at == revision
        {
            return draft.clone();
        }
        let value = self.input_box.buffer.value();
        let draft = (!value.is_empty()).then(|| Arc::from(value));
        self.draft_mirror = Some((revision, draft.clone()));
        draft
    }

    /// Everything the session mirrors from live state, built field by field so
    /// a new `SessionMeta` field forces a decision here. Every frame calls it,
    /// so it stays cheap: an idle UI has an empty draft, queue and rule list,
    /// and an empty `Vec` does not allocate.
    fn build_meta(&mut self) -> SessionMeta {
        let input_draft = self.draft();
        let state = &self.state;
        SessionMeta {
            mode: Some(state.mode.into()),
            plan_path: state.plan.path().map(|p| p.to_string_lossy().into_owned()),
            plan_written: state.plan.is_ready(),
            session_rules: rules_to_stored(&self.permissions.session_rules_snapshot()),
            context_size: state.context_size,
            input_draft,
            queued_messages: if self.recoverable_queue.is_empty() {
                self.queue.text_messages()
            } else {
                self.recoverable_queue.clone()
            },
            thinking: Some(state.thinking.into()),
            fast: state.fast,
            workflow: state.workflow,
            yolo: self.permissions.persisted_yolo(),
        }
    }

    /// Called where the set of subagent tabs changes, not at checkpoint time:
    /// the turn-end path clears `chat_index` right after pruning it, so a later
    /// rebuild would only ever find an empty map.
    pub(super) fn sync_subagents(&mut self) {
        let mut ordered: Vec<_> = self.chat_index.iter().collect();
        ordered.sort_by_key(|&(_, chat_index)| chat_index);
        let subagents = ordered
            .into_iter()
            .map(|(tool_id, &chat_index)| {
                let chat = &self.chats[chat_index];
                StoredSubagent {
                    tool_use_id: tool_id.clone(),
                    name: chat.name.clone(),
                    model: chat.model_id.clone(),
                }
            })
            .collect();
        self.state.session_mut().set_subagents(subagents);
    }

    pub(super) fn save_input_history(&self) {
        if let Err(e) = self.input_box.history().save(&self.storage) {
            tracing::warn!(error = %e, "input history save failed");
        }
    }

    pub(super) fn reset_ui_chrome(&mut self) {
        self.chats.clear();
        let mut main = Chat::new(
            "Main".into(),
            self.ui_config.clone(),
            self.lua_event_handle.clone(),
        );
        main.set_restore_channel(self.restore_event_tx.clone());
        self.chats.push(main);
        self.active_chat = 0;
        self.chat_index.clear();
        self.status = super::Status::Idle;
        self.clear_exit_request();
        self.queue.clear();
        self.recoverable_queue.clear();
        self.close_all_overlays();
        self.pending_input = PendingInput::None;
        self.status_bar.clear_flash();
        self.task_picker_original = None;
        self.last_esc = None;
        self.restoring = Arc::new(AtomicBool::new(false));
        self.plan_form.reset();
    }

    pub(crate) fn restore_display(&mut self) {
        let restoring = Arc::new(AtomicBool::new(true));
        self.restoring = restoring.clone();

        let (display_msgs, restore_items) = history_to_display(
            self.state.session.messages(),
            self.state.session.tool_outputs(),
            &self.ui_config.tool_output_lines,
        );
        self.main_chat().load_messages(display_msgs);
        let cost = self.state.cost;
        let context_size = self.state.context_size;
        let main = self.main_chat();
        main.cost = cost;
        main.context_size = context_size;
        if let Some(draft) = self.state.session.meta.input_draft.clone() {
            self.input_box.set_input(draft.to_string());
            self.input_box.buffer.move_to_end();
        }

        self.fire_restore_items(restore_items);

        // Read, not taken: the live chats below are the source `sync_subagents`
        // mirrors back, so emptying the session here would only make the next
        // checkpoint write the same list again.
        for sa in self.state.session.subagents().to_vec() {
            // A subagent reaches disk when it spawns but its transcript only
            // when it ends, so one without an entry here never got to finish:
            // leftovers from a kill mid-turn. It has nothing to show, and
            // restoring it would park a task no agent backs at the top of the
            // picker, running forever. `sync_subagents` below drops it for good.
            let Some(messages) = self.state.session.subagent_messages().get(&sa.tool_use_id) else {
                continue;
            };
            let (display, items) = history_to_display(
                messages,
                self.state.session.tool_outputs(),
                &self.ui_config.tool_output_lines,
            );
            self.chat_index
                .insert(sa.tool_use_id.clone(), self.chats.len());
            let mut chat = Chat::subagent(
                &sa.tool_use_id,
                sa.name,
                self.ui_config.clone(),
                self.lua_event_handle.clone(),
            );
            chat.set_restore_channel(self.restore_event_tx.clone());
            chat.model_id = sa.model;
            chat.load_messages(display);
            // The session file keeps the transcript but never how it ended,
            // so a reload admits that instead of guessing.
            chat.mark_finished(TaskOutcome::Unknown, DONE_TEXT);
            self.fire_restore_items(items);
            self.chats.push(chat);
        }

        self.sync_subagents();

        let eh = &self.lua_event_handle;
        if eh.is_disconnected() {
            self.restoring
                .store(false, std::sync::atomic::Ordering::Relaxed);
        } else {
            eh.send_restore_complete(restoring);
        }
    }

    fn fire_restore_items(&self, items: Vec<maki_lua::RestoreItem>) {
        let Some(tx) = &self.restore_event_tx else {
            return;
        };
        let eh = &self.lua_event_handle;
        let theme_gen = crate::theme::generation();
        for mut item in items {
            item.theme_gen = Some(theme_gen);
            eh.request_restore(item, tx.clone());
        }
    }

    /// Resume at process start: the agent was already spawned with this
    /// history, so no respawn follows and the restored queue must be
    /// flushed here.
    pub(crate) fn restore_resumed_session(&mut self) {
        self.permissions
            .load_session_rules(stored_to_rules(&self.state.session.meta.session_rules));
        self.restore_display();
        self.flush_restored_queue();
        for w in self.state.warnings.drain(..) {
            self.status_bar.flash(w);
        }
    }

    /// The one funnel for handing a history over. When the UI installs one the
    /// agent did not give it (rewind, load, new session), the mirror handle
    /// goes away in the same breath, so no later checkpoint can bring the
    /// agent's stale copy back. Only `respawn_agent` hands a live mirror in.
    fn install_local_history(&mut self) -> LoadedSession {
        self.shared_history = None;
        LoadedSession {
            messages: self.state.session.messages().to_vec(),
            model_spec: self.state.session.model.clone(),
        }
    }

    pub(super) fn reset_session(&mut self) -> Vec<Action> {
        self.checkpoint_now();
        self.flush_turn_stats();
        self.reset_ui_chrome();
        self.state.token_usage = TokenUsage::default();
        self.state.cost = None;
        self.state.context_size = 0;
        self.turn_history.clear();
        self.state.plan = PlanState::None;
        if self.state.mode == Mode::Plan {
            self.enter_plan();
        }
        // Fire before the swap. A handler cleaning up after the session
        // that just ended needs its id, and the stamp always reads
        // whichever session is current.
        self.fire_session_autocmd("SessionReset", serde_json::json!({}));
        self.state.session = Arc::new(AppSession::new(
            &self.state.session.model,
            &self.state.session.cwd,
        ));
        maki_otel::emit::session_started(
            maki_otel::emit::START_FRESH,
            Some(&self.state.session.id.to_string()),
        );
        self.install_local_history();
        vec![Action::NewSession]
    }

    pub(super) fn open_goto_picker(&mut self) -> Vec<Action> {
        self.checkpoint_now();
        match self.goto_picker.open(self.state.session.messages()) {
            Ok(()) => vec![],
            Err(msg) => {
                self.status_bar.flash(msg);
                vec![]
            }
        }
    }

    pub(super) fn scroll_to_turn(&mut self, entry: RewindEntry) -> Vec<Action> {
        let segment_idx = entry.segment_index;
        self.main_chat().scroll_to_segment(segment_idx);
        vec![]
    }

    pub(super) fn open_rewind_picker(&mut self) -> Vec<Action> {
        match self.rewind_picker.open(self.state.session.messages()) {
            Ok(()) => vec![],
            Err(msg) => {
                self.status_bar.flash(msg);
                vec![]
            }
        }
    }

    pub(super) fn rewind_to(&mut self, entry: RewindEntry) -> Vec<Action> {
        // The live size came from the provider, so it also counts the system
        // prompt and the tool schemas, a baseline the estimator cannot see.
        // Subtract only what we drop, or the gauge collapses until the next
        // turn measures it again. An emptied history is a fresh session though,
        // baseline included.
        let baseline = self
            .state
            .context_size
            .saturating_sub(estimate_message_tokens(self.state.session.messages()));
        let session = self.state.session_mut();
        session.truncate_messages(entry.turn_index);
        session.prune_orphans(|m| m.tool_uses().map(|(id, _, _)| id.to_owned()).collect());
        session.update_title_if_default();
        let kept = estimate_message_tokens(self.state.session.messages());
        self.state.context_size = if kept == 0 { 0 } else { baseline + kept };

        self.reset_ui_chrome();
        self.restore_display();

        self.input_box.set_input(entry.prompt_text);
        self.input_box.buffer.move_to_end();

        vec![Action::LoadSession(Box::new(self.install_local_history()))]
    }

    pub(super) fn goto_turn(&mut self, turn_str: &str) -> Vec<Action> {
        let turn_num: usize = match turn_str.parse() {
            Ok(n) if n > 0 => n,
            _ => {
                self.status_bar.flash("Usage: /goto <turn number>".into());
                return vec![];
            }
        };
        let mut user_count = 0usize;
        for (msg_idx, msg) in self.state.session.messages().iter().enumerate() {
            if matches!(msg.role, Role::User) {
                user_count += 1;
                if user_count == turn_num {
                    let display_idx =
                        display_msg_index_for_turn(self.state.session.messages(), msg_idx);
                    self.main_chat().scroll_to_segment(display_idx);
                    self.checkpoint_now();
                    return vec![];
                }
            }
        }
        self.status_bar.flash(format!("Turn {turn_num} not found"));
        vec![]
    }

    pub(crate) fn apply_loaded_session(
        &mut self,
        session: AppSession,
        fallback_model: &Model,
    ) -> LoadedSession {
        self.checkpoint_now();
        self.permissions
            .load_session_rules(stored_to_rules(&session.meta.session_rules));
        self.state =
            SessionState::from_session(session, fallback_model, &self.storage, &self.model_policy);
        for w in self.state.warnings.drain(..) {
            self.status_bar.flash(w);
        }
        self.reset_ui_chrome();
        self.restore_display();

        self.install_local_history()
    }

    pub(crate) fn load_session(&mut self, session_id: MakiId) -> Vec<Action> {
        let session = match AppSession::load(session_id, &self.storage) {
            Ok(s) => s,
            Err(e) => {
                self.status_bar
                    .flash(format!("Failed to load session: {e}"));
                return vec![];
            }
        };
        let loaded = self.apply_loaded_session(session, &self.state.model.clone());
        vec![Action::LoadSession(Box::new(loaded))]
    }

    pub(super) fn shift_session(&mut self, delta: i32) -> Vec<Action> {
        self.checkpoint_now();
        let settings = UserSettings::load();
        let summaries_res = if settings.global_sessions {
            AppSession::list_all(&self.storage)
        } else {
            AppSession::list(&self.state.session.cwd, &self.storage)
        };
        let summaries = match summaries_res {
            Ok(list) => list,
            Err(e) => {
                self.status_bar
                    .flash(format!("Failed to list sessions: {e}"));
                return vec![];
            }
        };

        if summaries.len() <= 1 {
            self.status_bar
                .flash("No other sessions to switch to".into());
            return vec![];
        }

        let current_id = &self.state.session.id;
        let current_pos = summaries.iter().position(|s| s.id == *current_id);

        let target_idx = match current_pos {
            Some(pos) => {
                if delta < 0 {
                    (pos + 1) % summaries.len()
                } else {
                    (pos + summaries.len() - 1) % summaries.len()
                }
            }
            None => 0,
        };

        let target_id = summaries[target_idx].id;
        let target_title = &summaries[target_idx].title;

        let actions = self.load_session(target_id);
        self.status_bar
            .flash(format!("Switched to session: {target_title}"));
        actions
    }

    pub(crate) fn export_session_to_markdown(&self) -> String {
        use std::fmt::Write;
        let mut out = String::new();

        let _ = writeln!(out, "# Session: {}", self.state.session.title);
        let _ = writeln!(out, "- **Model:** `{}`", self.state.session.model);
        let _ = writeln!(out, "- **CWD:** `{}`", self.state.session.cwd);
        let _ = writeln!(out, "\n---\n");

        let main_msgs = format_messages(
            self.state.session.messages(),
            self.state.session.tool_outputs(),
        );
        out.push_str(&main_msgs);

        if !self.state.session.subagent_messages().is_empty() {
            let _ = writeln!(out, "\n## Subagents\n");
            let mut subagents: Vec<_> = self.state.session.subagent_messages().keys().collect();
            subagents.sort();

            for tool_use_id in subagents {
                if let Some(messages) = self.state.session.subagent_messages().get(tool_use_id) {
                    let name = self
                        .state
                        .session
                        .subagents()
                        .iter()
                        .find(|sa| sa.tool_use_id == *tool_use_id)
                        .map(|sa| sa.name.as_str())
                        .unwrap_or("Subagent");
                    let _ = writeln!(out, "### {} ({})\n", name, tool_use_id);
                    let sub_msgs = format_messages(messages, self.state.session.tool_outputs());
                    out.push_str(&sub_msgs);
                }
            }
        }

        out
    }

    pub(crate) fn export_session_to_json(&self) -> String {
        serde_json::to_string_pretty(&self.state.session).unwrap_or_default()
    }
}

fn format_messages(
    messages: &[Message],
    tool_outputs: &std::collections::HashMap<String, Arc<ToolOutput>>,
) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    for message in messages {
        match message.role {
            Role::User => {
                let has_non_tool_result = message
                    .content
                    .iter()
                    .any(|block| !matches!(block, ContentBlock::ToolResult { .. }));
                if !has_non_tool_result {
                    continue;
                }

                let _ = writeln!(out, "### User\n");
                for block in &message.content {
                    match block {
                        ContentBlock::Text { text } => {
                            if !text.is_empty() {
                                let _ = writeln!(out, "{}\n", text.trim_end());
                            }
                        }
                        ContentBlock::Image { source } => {
                            let _ = writeln!(out, "![Image]({})\n", source.to_data_url());
                        }
                        _ => {}
                    }
                }
            }
            Role::Assistant => {
                let _ = writeln!(out, "### Assistant\n");
                for block in &message.content {
                    match block {
                        ContentBlock::Text { text } => {
                            if !text.is_empty() {
                                let _ = writeln!(out, "{}\n", text.trim_end());
                            }
                        }
                        ContentBlock::Thinking { thinking, .. } => {
                            if !thinking.is_empty() {
                                let _ = writeln!(
                                    out,
                                    "<details>\n<summary>Thinking</summary>\n\n{}\n</details>\n",
                                    thinking.trim()
                                );
                            }
                        }
                        ContentBlock::RedactedThinking { data } => {
                            if !data.is_empty() {
                                let _ = writeln!(
                                    out,
                                    "<details>\n<summary>Thinking (Redacted)</summary>\n\n{}\n</details>\n",
                                    data.trim()
                                );
                            }
                        }
                        ContentBlock::ToolUse {
                            id, name, input, ..
                        } => {
                            let _ = writeln!(out, "**Tool Call:** `{}`", name);
                            let input_pretty = serde_json::to_string_pretty(input)
                                .unwrap_or_else(|_| input.to_string());
                            let _ = writeln!(out, "```json\n{}\n```", input_pretty.trim());

                            if let Some(tool_output) = tool_outputs.get(id) {
                                let output_text = tool_output.as_text();
                                if !output_text.is_empty() {
                                    let _ = writeln!(out, "**Output:**");
                                    if tool_output.is_markdown() {
                                        let _ = writeln!(out, "{}", output_text.trim_end());
                                    } else {
                                        let _ =
                                            writeln!(out, "```\n{}\n```", output_text.trim_end());
                                    }
                                }
                            } else {
                                let _ = writeln!(out);
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    out
}
