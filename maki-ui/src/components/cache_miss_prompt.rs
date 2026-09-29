use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use super::Overlay;
use super::form::render_form;
use crate::agent::shared_queue::QueuedMessage;
use crate::theme;
use maki_providers::format_tokens;

/// A y/n gate in the permission-prompt style: shown when a turn will likely
/// miss the prompt cache (idle past the timeout, or the model changed) and
/// resending the context uncached costs more than the configured thresholds.
pub(crate) struct CacheMissPrompt {
    /// The held message and the numbers that triggered the gate.
    pending: Option<(QueuedMessage, Risk)>,
}

/// What the gate shows: the size and list price of the resend, next to the
/// configured limits that decided it was worth asking about.
pub(crate) struct Risk {
    pub tokens: u32,
    pub cost: f64,
    pub idle_minutes: u64,
    pub token_threshold: u32,
    pub cost_threshold: f64,
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
        Self { pending: None }
    }

    pub(crate) fn open(&mut self, msg: QueuedMessage, risk: Risk) {
        self.pending = Some((msg, risk));
    }

    /// Takes the held message on yes or no; `None` while waiting or on an
    /// unrelated key.
    pub(crate) fn handle_key(&mut self, key: KeyEvent) -> Option<Answer> {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                self.pending.take().map(|(msg, _)| Answer::Proceed(msg))
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                self.pending.take().map(|(msg, _)| Answer::Decline(msg))
            }
            _ => None,
        }
    }

    pub(crate) fn build_lines(&self) -> Vec<Line<'static>> {
        let t = theme::current();
        let warn = Style::new().fg(t.todo_in_progress.fg.unwrap_or_default());
        let Some((_, risk)) = &self.pending else {
            return Vec::new();
        };
        let dim = |s: String| Span::styled(s, t.tool_dim);
        let hot = |s: String| Span::styled(s, warn);
        vec![
            Line::raw(""),
            Line::from(Span::styled(
                "  cache miss likely — the whole context goes out as input",
                t.tool_dim,
            )),
            Line::from(vec![
                Span::raw("  "),
                dim("resending ".into()),
                hot(format!("{} t", format_tokens(risk.tokens))),
                dim(" · estimated input cost: ".into()),
                hot(format!("${:.2}", risk.cost)),
            ]),
            Line::from(vec![
                Span::raw("  "),
                dim("limits: idle ".into()),
                hot(format!("{} min", risk.idle_minutes)),
                dim(" · tokens ".into()),
                hot(format_tokens(risk.token_threshold)),
                dim(" · cost ".into()),
                hot(format!("${:.2}", risk.cost_threshold)),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::key;

    fn risk() -> Risk {
        Risk {
            tokens: 150_000,
            cost: 0.5,
            idle_minutes: 5,
            token_threshold: 100_000,
            cost_threshold: 0.1,
        }
    }

    fn opened() -> CacheMissPrompt {
        let mut prompt = CacheMissPrompt::new();
        prompt.open(
            QueuedMessage {
                text: "go".into(),
                images: vec![],
            },
            risk(),
        );
        prompt
    }

    fn text(prompt: &CacheMissPrompt) -> String {
        prompt
            .build_lines()
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect::<Vec<_>>()
            .join("")
    }

    #[test]
    fn the_gate_names_the_resend_and_the_limits() {
        let t = text(&opened());
        assert!(t.contains("resending 150.0k t"), "{t}");
        assert!(t.contains("estimated input cost: $0.50"), "{t}");
        assert!(
            t.contains("limits: idle 5 min · tokens 100.0k · cost $0.10"),
            "{t}"
        );
    }

    #[test]
    fn answering_takes_the_message_and_the_numbers() {
        let mut prompt = opened();
        assert!(matches!(
            prompt.handle_key(key(KeyCode::Char('y'))),
            Some(Answer::Proceed(_))
        ));
        assert!(!prompt.is_open());
        assert!(prompt.build_lines().is_empty());
    }
}
