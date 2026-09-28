use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use super::Overlay;
use super::form::render_form;
use crate::agent::shared_queue::QueuedMessage;
use crate::theme;

/// A y/n gate in the permission-prompt style: shown when a turn will likely
/// miss the prompt cache (idle past the timeout, or the model changed) and
/// resending the context uncached costs more than the configured thresholds.
pub(crate) struct CacheMissPrompt {
    pending: Option<QueuedMessage>,
    cost: f64,
}

impl Overlay for CacheMissPrompt {
    fn is_open(&self) -> bool {
        self.pending.is_some()
    }

    fn close(&mut self) {
        self.pending = None;
    }
}

pub(crate) enum Answer {
    Proceed(QueuedMessage),
    /// Declined: the message comes back so its text can return to the input.
    Decline(QueuedMessage),
}

impl CacheMissPrompt {
    pub(crate) fn new() -> Self {
        Self {
            pending: None,
            cost: 0.0,
        }
    }

    pub(crate) fn open(&mut self, msg: QueuedMessage, cost: f64) {
        self.pending = Some(msg);
        self.cost = cost;
    }

    /// Takes the held message on yes or no; `None` while waiting or on an
    /// unrelated key.
    pub(crate) fn handle_key(&mut self, key: KeyEvent) -> Option<Answer> {
        let msg = match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                self.pending.take().map(Answer::Proceed)
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                self.pending.take().map(Answer::Decline)
            }
            _ => None,
        };
        if msg.is_some() {
            self.cost = 0.0;
        }
        msg
    }

    pub(crate) fn build_lines(&self) -> Vec<Line<'static>> {
        let t = theme::current();
        let warn = Style::new().fg(t.todo_in_progress.fg.unwrap_or_default());
        vec![
            Line::raw(""),
            Line::from(Span::styled(
                "  cache miss likely — the whole context goes out as input",
                t.tool_dim,
            )),
            Line::from(vec![
                Span::raw("  "),
                Span::styled("estimated input cost: ", t.tool_dim),
                Span::styled(format!("${:.2}", self.cost), warn),
            ]),
            Line::raw(""),
            Line::from(vec![
                Span::raw("  "),
                Span::styled("Proceed? ", t.tool_dim),
                Span::styled("y/n", t.item_desc),
            ]),
        ]
    }

    pub(crate) fn view(&self, frame: &mut Frame, area: ratatui::prelude::Rect) {
        if !self.is_open() {
            return;
        }
        let lines = self.build_lines();
        let t = theme::current();
        render_form(&t, " Cache Miss Warning ", frame, area, lines, (0, 0));
    }

    pub(crate) fn height(&self, width: u16) -> u16 {
        let inner_width = width.saturating_sub(2);
        let lines = self.build_lines();
        use ratatui::widgets::Wrap;
        let para = ratatui::widgets::Paragraph::new(lines).wrap(Wrap { trim: false });
        para.line_count(inner_width) as u16 + 2
    }
}
