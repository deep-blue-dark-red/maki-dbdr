use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use crate::chat::{Chat, DONE_TEXT, history_to_display};
use crate::components::DisplayRole;
use crate::components::rewind_picker::RewindEntry;
use crate::components::{Action, LoadedSession};
use maki_providers::{Model, TokenUsage};
use maki_storage::id::MakiId;
use maki_storage::sessions::StoredSubagent;

use crate::AppSession;

use super::session_state::{SessionState, stored_to_rules};
use super::{App, Mode, PendingInput, PlanState};
use crate::agent::QueuedMessage;

/// The single content predicate: `App::save_session` persists a session
/// iff this holds, and the shutdown path reuses it to tell which tabs were
/// saved, so the report and the disk can never disagree. Sync the session
/// first (`save_session` does).
pub(crate) fn session_has_content(session: &AppSession) -> bool {
    !session.messages.is_empty()
        || session.meta.input_draft.is_some()
        || !session.meta.queued_messages.is_empty()
        || session.meta.mode != Some(maki_storage::sessions::StoredMode::Build)
}

impl App {
    pub(crate) fn has_content(&self) -> bool {
        session_has_content(&self.state.session)
    }

    pub(crate) fn save_session(&mut self) {
        self.state.sync_session(
            &self.shared_history,
            &self.shared_tool_outputs,
            &self.permissions,
        );
        self.sync_ephemeral_state();
        if !self.has_content() {
            return;
        }
        self.enqueue_save();
    }

    fn sync_ephemeral_state(&mut self) {
        let draft = self.input_box.buffer.value();
        self.state.session.meta.input_draft = if draft.is_empty() { None } else { Some(draft) };

        self.state.session.meta.queued_messages = self.queue.text_messages();

        self.state.session.meta.subagents = self
            .chats
            .iter()
            .skip(1)
            .zip(self.chat_index.iter())
            .map(|(chat, (tool_id, _))| StoredSubagent {
                tool_use_id: tool_id.clone(),
                name: chat.name.clone(),
                prompt: None,
                model: chat.model_id.clone(),
            })
            .collect();
    }

    pub(super) fn save_input_history(&self) {
        if let Err(e) = self.input_box.history().save(&self.storage) {
            tracing::warn!(error = %e, "input history save failed");
        }
    }

    pub(super) fn enqueue_save(&self) {
        self.storage_writer
            .send(Box::new(self.state.session.clone()));
    }

    pub(super) fn reset_ui_chrome(&mut self) {
        self.chats.clear();
        let mut main = Chat::new("Main".into(), self.ui_config);
        main.set_restore_channel(self.lua_event_handle.clone(), self.restore_event_tx.clone());
        self.chats.push(main);
        self.active_chat = 0;
        self.chat_index.clear();
        self.status = super::Status::Idle;
        self.queue.clear();
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
            &self.state.session.messages,
            &self.state.session.tool_outputs,
            &self.ui_config.tool_output_lines,
        );
        self.main_chat().load_messages(display_msgs);
        self.main_chat().token_usage = self.state.token_usage;
        self.main_chat().context_size = self.state.context_size;
        if let Some(draft) = self.state.session.meta.input_draft.take() {
            self.input_box.set_input(draft);
            self.input_box.buffer.move_to_end();
        }

        for text in std::mem::take(&mut self.state.session.meta.queued_messages) {
            let msg = QueuedMessage {
                text,
                images: Vec::new(),
            };
            self.queue_and_notify(msg);
        }

        self.fire_restore_items(restore_items);

        for sa in std::mem::take(&mut self.state.session.meta.subagents) {
            let idx = self.chats.len();
            self.chat_index.insert(sa.tool_use_id.clone(), idx);
            let mut chat = Chat::new(sa.name, self.ui_config);
            chat.set_restore_channel(self.lua_event_handle.clone(), self.restore_event_tx.clone());
            chat.model_id = sa.model;
            if let Some(messages) = self.state.session.subagent_messages.get(&sa.tool_use_id) {
                let (display, items) = history_to_display(
                    messages,
                    &self.state.session.tool_outputs,
                    &self.ui_config.tool_output_lines,
                );
                chat.load_messages(display);
                chat.mark_finished(DisplayRole::Done, DONE_TEXT);
                self.fire_restore_items(items);
            }
            self.chats.push(chat);
        }

        if let Some(eh) = &self.lua_event_handle {
            eh.send_restore_complete(restoring);
        } else {
            self.restoring
                .store(false, std::sync::atomic::Ordering::Relaxed);
        }
    }

    fn fire_restore_items(&self, items: Vec<maki_lua::RestoreItem>) {
        let (Some(eh), Some(tx)) = (&self.lua_event_handle, &self.restore_event_tx) else {
            return;
        };
        let theme_gen = crate::theme::generation();
        for mut item in items {
            item.theme_gen = Some(theme_gen);
            eh.request_restore(item, tx.clone());
        }
    }

    fn loaded_session_snapshot(&self) -> LoadedSession {
        LoadedSession {
            messages: self.state.session.messages.clone(),
            tool_outputs: self.state.session.tool_outputs.clone(),
            model_spec: self.state.session.model.clone(),
        }
    }

    pub(super) fn reset_session(&mut self) -> Vec<Action> {
        self.reset_ui_chrome();
        self.state.token_usage = TokenUsage::default();
        self.state.context_size = 0;
        self.state.plan = PlanState::None;
        if self.state.mode == Mode::Plan {
            self.enter_plan();
        }
        self.state.session = AppSession::new(&self.state.session.model, &self.state.session.cwd);
        self.fire_session_autocmd("SessionReset", serde_json::json!({}));
        vec![Action::NewSession]
    }

    pub(super) fn open_rewind_picker(&mut self) -> Vec<Action> {
        self.save_session();
        match self.rewind_picker.open(&self.state.session.messages) {
            Ok(()) => vec![],
            Err(msg) => {
                self.status_bar.flash(msg);
                vec![]
            }
        }
    }

    pub(super) fn rewind_to(&mut self, entry: RewindEntry) -> Vec<Action> {
        self.run_id += 1;

        self.state.session.messages.truncate(entry.turn_index);
        self.state
            .session
            .prune_orphans(|m| m.tool_uses().map(|(id, _, _)| id.to_owned()).collect());
        self.state.context_size =
            maki_agent::agent::estimate_message_tokens(&self.state.session.messages);

        self.reset_ui_chrome();
        self.restore_display();

        self.input_box.set_input(entry.prompt_text);
        self.input_box.buffer.move_to_end();

        self.state.session.update_title_if_default();
        self.enqueue_save();

        vec![Action::LoadSession(Box::new(
            self.loaded_session_snapshot(),
        ))]
    }

    pub(crate) fn apply_loaded_session(
        &mut self,
        session: AppSession,
        fallback_model: &Model,
    ) -> LoadedSession {
        self.permissions
            .load_session_rules(stored_to_rules(&session.meta.session_rules));
        self.state = SessionState::from_session(session, fallback_model, &self.storage);
        for w in self.state.warnings.drain(..) {
            self.status_bar.flash(w);
        }
        self.reset_ui_chrome();
        self.restore_display();

        self.enqueue_save();
        self.loaded_session_snapshot()
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
        self.save_session();
        let loaded = self.apply_loaded_session(session, &self.state.model.clone());
        vec![Action::LoadSession(Box::new(loaded))]
    }

    pub(crate) fn export_session_to_markdown(&self) -> String {
        use std::fmt::Write;
        let mut out = String::new();

        let _ = writeln!(out, "# Session: {}", self.state.session.title);
        let _ = writeln!(out, "- **Model:** `{}`", self.state.session.model);
        let _ = writeln!(out, "- **CWD:** `{}`", self.state.session.cwd);
        let _ = writeln!(out, "\n---\n");

        let main_msgs = format_messages(&self.state.session.messages, &self.state.session.tool_outputs);
        out.push_str(&main_msgs);

        if !self.state.session.subagent_messages.is_empty() {
            let _ = writeln!(out, "\n## Subagents\n");
            let mut subagents: Vec<_> = self.state.session.subagent_messages.keys().collect();
            subagents.sort();

            for tool_use_id in subagents {
                if let Some(messages) = self.state.session.subagent_messages.get(tool_use_id) {
                    let name = self.state.session.meta.subagents.iter()
                        .find(|sa| sa.tool_use_id == *tool_use_id)
                        .map(|sa| sa.name.as_str())
                        .unwrap_or("Subagent");
                    let _ = writeln!(out, "### {} ({})\n", name, tool_use_id);
                    let sub_msgs = format_messages(messages, &self.state.session.tool_outputs);
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
    messages: &[maki_providers::Message],
    tool_outputs: &std::collections::HashMap<String, maki_agent::ToolOutput>,
) -> String {
    use std::fmt::Write;
    use maki_providers::{ContentBlock, Role};
    let mut out = String::new();
    for message in messages {
        match message.role {
            Role::User => {
                let has_non_tool_result = message.content.iter().any(|block| {
                    !matches!(block, ContentBlock::ToolResult { .. })
                });
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
                        ContentBlock::ToolUse { id, name, input } => {
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
                                        let _ = writeln!(out, "```\n{}\n```", output_text.trim_end());
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
