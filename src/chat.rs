//! `maki chat`: a TUI client to live maki processes — TUI, `maki -p`, and
//! SDK sessions — over the shmem feed. Sessions are discovered from their
//! status rings; the conversation is tailed from the session transcript the
//! process itself writes; Cancel and Prompt travel the control rings.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use color_eyre::Result;
use color_eyre::eyre::Context;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use maki_agent::session::StoredSession;
use maki_providers::{ContentBlock, Message};
use maki_shmem::broadcast::SessionState;
use maki_shmem::control::{Command, Commander};
use maki_shmem::feed::{Feed, FeedSession};
use maki_storage::StateDir;
use maki_storage::id::MakiId;

/// One rendered transcript entry.
#[derive(Debug, Clone, PartialEq)]
struct ChatEntry {
    speaker: &'static str,
    text: String,
    dim: bool,
}

/// The transcript of one session: its file, watched by mtime, and the
/// rendered entries read from it.
#[derive(Debug, Default)]
struct Transcript {
    path: Option<PathBuf>,
    mtime: Option<SystemTime>,
    entries: Vec<ChatEntry>,
}

impl Transcript {
    /// Loads the stored session read-only; a torn last line is dropped in
    /// memory by the loader itself.
    fn load(session_id: &str, state: &StateDir) -> Self {
        let mut this = Self::default();
        this.refresh(session_id, state, true);
        this
    }

    /// Reloads when the file changed on disk. A missing file just clears:
    /// a session the storage cannot see still streams via the ring feed.
    fn refresh(&mut self, session_id: &str, state: &StateDir, force: bool) {
        let path = self
            .path
            .clone()
            .or_else(|| find_session_file(session_id, state));
        let mtime = path
            .as_ref()
            .and_then(|p| std::fs::metadata(p).ok())
            .and_then(|m| m.modified().ok());
        if !force && self.path == path && self.mtime == mtime {
            return;
        }
        self.path = path;
        self.mtime = mtime;
        self.entries = self
            .path
            .as_ref()
            .and_then(|_| load_entries(session_id, state))
            .unwrap_or_default();
    }
}

fn load_entries(session_id: &str, state: &StateDir) -> Option<Vec<ChatEntry>> {
    let id: MakiId = session_id.parse().ok()?;
    let session = StoredSession::read_only(id, state).ok()?;
    Some(render_messages(session.messages()))
}

/// The `<id>.jsonl` a session writes: directly under this state dir's
/// `sessions/`, or under any project's when the layout nests them.
fn find_session_file(session_id: &str, state: &StateDir) -> Option<PathBuf> {
    let wanted = format!("{session_id}.jsonl");
    let direct = state.path().join("sessions").join(&wanted);
    if direct.exists() {
        return Some(direct);
    }
    let projects = state.path().join("projects");
    std::fs::read_dir(&projects)
        .ok()?
        .flatten()
        .map(|project| project.path().join("sessions").join(&wanted))
        .find(|path| path.exists())
}

/// One entry per content block; thinking is skipped, tools are markers.
fn render_messages(messages: &[Message]) -> Vec<ChatEntry> {
    let mut out = Vec::new();
    for message in messages {
        let speaker = match message.role {
            maki_providers::Role::User => "user",
            maki_providers::Role::Assistant => "maki",
        };
        let display = message.display_text.clone();
        for block in &message.content {
            match block {
                ContentBlock::Text { text } => {
                    let text = display.clone().unwrap_or_else(|| text.clone());
                    if !text.trim().is_empty() {
                        out.push(ChatEntry {
                            speaker,
                            text,
                            dim: false,
                        });
                    }
                }
                ContentBlock::ToolUse { name, .. } => out.push(ChatEntry {
                    speaker,
                    text: format!("→ {name}"),
                    dim: true,
                }),
                ContentBlock::Image { .. } => out.push(ChatEntry {
                    speaker,
                    text: "[image]".to_owned(),
                    dim: true,
                }),
                ContentBlock::ToolResult { content, is_error, .. } => out.push(ChatEntry {
                    speaker,
                    text: if *is_error {
                        format!("✗ {}", first_line(content))
                    } else {
                        format!("· {}", first_line(content))
                    },
                    dim: true,
                }),
                ContentBlock::Thinking { .. } | ContentBlock::RedactedThinking { .. } => {}
            }
        }
    }
    out
}

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or_default().to_owned()
}

/// A sent command whose ack has not landed yet.
struct Pending {
    request_id: u64,
    session_id: String,
    what: &'static str,
}

struct App {
    feed: Feed,
    commander: Option<Commander>,
    state: StateDir,
    /// A focused id still waiting for its process to appear; grabbed on
    /// the first tick that sees it.
    focus: Option<String>,
    selected: usize,
    /// Lines scrolled up from the newest; 0 follows the tail.
    scroll: usize,
    transcripts: HashMap<String, Transcript>,
    prompt: Option<String>,
    notice: String,
    pending: Vec<Pending>,
    quit: bool,
}

impl App {
    fn new(state: StateDir, focus: Option<String>) -> Self {
        let mut feed = Feed::new();
        feed.poll();
        let mut app = Self {
            feed,
            commander: Commander::start().ok(),
            state,
            focus: focus.clone(),
            selected: 0,
            scroll: 0,
            transcripts: HashMap::new(),
            prompt: None,
            notice: String::new(),
            pending: Vec::new(),
            quit: false,
        };
        if let Some(id) = focus {
            if let Some(pos) = app.feed.sessions.iter().position(|s| s.session_id == id) {
                app.selected = pos;
                app.focus = None;
            } else {
                app.notice = format!("session {id} is not broadcasting (yet)");
            }
        }
        app
    }

    fn session(&self) -> Option<&FeedSession> {
        self.feed.sessions.get(self.selected)
    }

    fn tick(&mut self) {
        if self.feed.poll() {
            self.selected = self.selected.min(self.feed.sessions.len().saturating_sub(1));
        }
        if let Some(id) = self.focus.clone()
            && let Some(pos) = self.feed.sessions.iter().position(|s| s.session_id == id)
        {
            self.selected = pos;
            self.focus = None;
            self.notice.clear();
        }
        if let Some(session) = self.session() {
            let id = session.session_id.clone();
            let transcript = self
                .transcripts
                .entry(id.clone())
                .or_insert_with(|| Transcript::load(&id, &self.state));
            transcript.refresh(&id, &self.state, false);
        }
        for ack in std::mem::take(&mut self.feed.acks) {
            if let Some(pos) = self.pending.iter().position(|p| {
                p.request_id == ack.request_id && p.session_id == ack.session_id
            }) {
                let what = self.pending.remove(pos).what;
                self.notice = format!("{what}: {}", ack_label(ack.result));
            }
        }
    }

    fn send(&mut self, what: &'static str, command: Command) {
        match (&self.commander, self.session()) {
            (Some(commander), Some(session)) => {
                let request_id = commander.send(&session.session_id, command);
                self.pending.push(Pending {
                    request_id,
                    session_id: session.session_id.clone(),
                    what,
                });
                self.notice = format!("{what} sent…");
            }
            _ => self.notice = "no live session selected".to_owned(),
        }
    }

    fn on_key(&mut self, key: KeyEvent) {
        if self.prompt.is_some() {
            self.on_prompt_key(key);
            return;
        }
        match (key.code, key.modifiers) {
            (KeyCode::Char('q') | KeyCode::Esc, _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                self.quit = true;
            }
            (KeyCode::Tab, _) => self.step_session(1),
            (KeyCode::BackTab, _) => self.step_session(-1),
            (KeyCode::Char('j') | KeyCode::Down, _) => self.scroll = self.scroll.saturating_add(1),
            (KeyCode::Char('k') | KeyCode::Up, _) => self.scroll = self.scroll.saturating_sub(1),
            (KeyCode::Char('g'), _) => self.scroll = usize::MAX,
            (KeyCode::Char('G'), _) => self.scroll = 0,
            (KeyCode::Char('r'), _) => {
                if let Some(session) = self.session() {
                    let id = session.session_id.clone();
                    if let Some(t) = self.transcripts.get_mut(&id) {
                        t.refresh(&id, &self.state, true);
                    }
                    self.notice = "transcript reloaded".to_owned();
                }
            }
            (KeyCode::Char('c'), _) => self.send("cancel", Command::Cancel),
            (KeyCode::Char('p'), _) => self.prompt = Some(String::new()),
            _ => {}
        }
    }

    fn on_prompt_key(&mut self, key: KeyEvent) {
        let Some(text) = self.prompt.as_mut() else {
            return;
        };
        match key.code {
            KeyCode::Esc => self.prompt = None,
            KeyCode::Enter => {
                let text = std::mem::take(text);
                self.prompt = None;
                if !text.trim().is_empty() {
                    self.send("prompt", Command::Prompt(text));
                }
            }
            KeyCode::Backspace => {
                text.pop();
            }
            KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => text.push(ch),
            _ => {}
        }
    }

    fn step_session(&mut self, dir: i32) {
        let len = self.feed.sessions.len();
        if len == 0 {
            return;
        }
        self.selected = (self.selected as i32 + dir).rem_euclid(len as i32) as usize;
        self.scroll = 0;
    }
}

fn state_color(state: SessionState, theme: &Theme) -> ratatui::style::Color {
    match state {
        SessionState::Working => theme.working,
        SessionState::Waiting => theme.waiting,
        SessionState::Idle => theme.idle,
    }
}

struct Theme {
    working: ratatui::style::Color,
    waiting: ratatui::style::Color,
    idle: ratatui::style::Color,
    dim: ratatui::style::Color,
    accent: ratatui::style::Color,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            working: ratatui::style::Color::Green,
            waiting: ratatui::style::Color::Yellow,
            idle: ratatui::style::Color::DarkGray,
            dim: ratatui::style::Color::DarkGray,
            accent: ratatui::style::Color::Cyan,
        }
    }
}

/// Runs the chat client. `focus` preselects a session id if it is live.
pub fn run(focus: Option<String>, state: StateDir) -> Result<()> {
    let mut terminal = TermGuard::setup()?;
    let mut app = App::new(state, focus);
    let theme = Theme::default();
    loop {
        app.tick();
        terminal.0.draw(|frame| draw(frame, &app, &theme))?;
        if crossterm::event::poll(Duration::from_millis(250)).context("poll input")?
            && let Ok(Event::Key(key)) = crossterm::event::read()
            && key.kind == crossterm::event::KeyEventKind::Press
        {
            app.on_key(key);
        }
        if app.quit {
            return Ok(());
        }
    }
}

struct TermGuard(ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>);

impl TermGuard {
    fn setup() -> Result<Self> {
        crossterm::terminal::enable_raw_mode().context("raw mode")?;
        crossterm::execute!(std::io::stdout(), crossterm::terminal::EnterAlternateScreen)
            .context("alternate screen")?;
        let backend = ratatui::backend::CrosstermBackend::new(std::io::stdout());
        Ok(Self(ratatui::Terminal::new(backend).context("terminal")?))
    }
}

impl Drop for TermGuard {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen);
    }
}

fn draw(frame: &mut Frame, app: &App, theme: &Theme) {
    let area = frame.area();
    let chunks = ratatui::layout::Layout::horizontal([
        ratatui::layout::Constraint::Percentage(34),
        ratatui::layout::Constraint::Percentage(66),
    ])
    .split(area);
    draw_sessions(frame, chunks[0], app, theme);
    draw_conversation(frame, chunks[1], app, theme);
}

fn draw_sessions(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(
            format!(" sessions ({}) ", app.feed.sessions.len()),
            Style::default().fg(theme.accent),
        ));
    let rows: Vec<Line> = if app.feed.sessions.is_empty() {
        vec![Line::from(Span::styled(
            " no maki process broadcasting",
            Style::default().fg(theme.dim),
        ))]
    } else {
        app.feed
            .sessions
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let selected = i == app.selected;
                let style = if selected {
                    Style::default().add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                Line::from(vec![
                    Span::styled(
                        format!("{:<7} ", s.state.label()),
                        style.fg(state_color(s.state, theme)),
                    ),
                    Span::styled(
                        format!("{} ", truncate(&s.title, area.width.saturating_sub(14) as usize)),
                        style,
                    ),
                ])
            })
            .collect()
    };
    frame.render_widget(Paragraph::new(rows).block(block), area);
}

fn draw_conversation(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let block = Block::default().borders(Borders::ALL);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width < 4 || inner.height < 3 {
        return;
    }

    let header_area = Rect {
        height: 1,
        width: inner.width,
        y: inner.y,
        x: inner.x,
    };
    let prompt_area = Rect {
        height: 1,
        y: inner.y + inner.height - 1,
        width: inner.width,
        x: inner.x,
    };
    let body_area = Rect {
        height: inner.height.saturating_sub(2),
        y: inner.y + 1,
        width: inner.width,
        x: inner.x,
    };

    let Some(session) = app.session() else {
        let hint = Line::from(Span::styled(
            " nothing live — start `maki`, `maki -p`, or `maki sdk`",
            Style::default().fg(theme.dim),
        ));
        frame.render_widget(Paragraph::new(hint), body_area);
        return;
    };

    static EMPTY: Transcript = Transcript {
        path: None,
        mtime: None,
        entries: Vec::new(),
    };
    let transcript = app
        .transcripts
        .get(&session.session_id)
        .unwrap_or(&EMPTY);
    let header = Line::from(vec![
        Span::styled(session.state.label(), Style::default().fg(state_color(session.state, theme))),
        Span::raw(format!(
            "  {}  ctx {}/{}  ${:.2}  pid {}  {}",
            session.title,
            session.context_used,
            session.context_limit,
            session.cost_usd,
            session.pid,
            session.last_tool
        )),
    ]);
    frame.render_widget(Paragraph::new(header), header_area);

    let mut lines: Vec<Line> = transcript
        .entries
        .iter()
        .map(|entry| {
            let style = if entry.dim {
                Style::default().fg(theme.dim)
            } else if entry.speaker == "user" {
                Style::default().fg(theme.accent)
            } else {
                Style::default()
            };
            Line::from(Span::styled(
                format!("{:<5} {}", entry.speaker, entry.text),
                style,
            ))
        })
        .collect();
    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            " (no transcript on disk yet)",
            Style::default().fg(theme.dim),
        )));
    }

    // Bottom-anchored window with wrap: show the newest entries, scrolling
    // up raises the start offset.
    let budget = body_area.height as usize;
    let wrapped = wrap_entries(&lines, body_area.width as usize);
    let start = wrapped
        .len()
        .saturating_sub(budget.saturating_add(app.scroll).min(wrapped.len()));
    frame.render_widget(Paragraph::new(wrapped[start..].to_vec()), body_area);

    let footer_text = match (&app.prompt, app.pending.last()) {
        (Some(text), _) => format!(" prompt ▸ {text}_"),
        (None, Some(pending)) => {
            format!(" {} sent, waiting for ack …  {NEWEST_HINT}", pending.what)
        }
        (None, None) => format!(" {NEWEST_HINT}"),
    };
    let footer = if app.notice.is_empty() {
        footer_text
    } else {
        format!(" {} │ {}", app.notice, NEWEST_HINT)
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            footer,
            Style::default().fg(theme.dim),
        ))),
        prompt_area,
    );
}


fn ack_label(result: maki_shmem::control::AckResult) -> &'static str {
    match result {
        maki_shmem::control::AckResult::Ok => "ok",
        maki_shmem::control::AckResult::NoSession => "no such session",
        maki_shmem::control::AckResult::Rejected => "rejected",
    }
}

const NEWEST_HINT: &str = "tab next · j/k scroll · c cancel · p prompt · q quit";

/// Splits entries into display lines wrapped to `width`, so the tail window
/// counts what the screen actually shows.
fn wrap_entries<'a>(lines: &[Line<'a>], width: usize) -> Vec<Line<'a>> {
    let mut out = Vec::with_capacity(lines.len());
    for line in lines {
        let text = line.spans.iter().map(|s| s.content.as_ref()).collect::<String>();
        let char_count = text.chars().count();
        if width > 0 && char_count > width {
            for chunk in text
                .chars()
                .collect::<Vec<_>>()
                .chunks(width)
                .map(|c| c.iter().collect::<String>())
            {
                let style = line.spans.first().map(|s| s.style).unwrap_or_default();
                out.push(Line::from(Span::styled(chunk, style)));
            }
        } else {
            out.push(line.clone());
        }
    }
    out
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_owned()
    } else {
        let cut: String = text.chars().take(max.saturating_sub(1)).collect();
        format!("{cut}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_each_block_as_an_entry() {
        let messages = vec![
            Message::user("hello there".to_owned()),
            Message {
                role: maki_providers::Role::Assistant,
                content: vec![
                    ContentBlock::Thinking {
                        thinking: "hidden".to_owned(),
                        signature: None,
                    },
                    ContentBlock::Text {
                        text: "hi".to_owned(),
                    },
                    ContentBlock::ToolUse {
                        id: "1".to_owned(),
                        name: "bash".to_owned(),
                        input: serde_json::Value::Null,
                        thought_signature: None,
                    },
                ],
                ..Default::default()
            },
        ];
        let entries = render_messages(&messages);
        let texts: Vec<&str> = entries.iter().map(|e| e.text.as_str()).collect();
        assert_eq!(texts, vec!["hello there", "hi", "→ bash"]);
        assert_eq!(entries[0].speaker, "user");
        assert!(entries[2].dim);
    }

    #[test]
    fn truncation_keeps_one_char_headroom() {
        assert_eq!(truncate("abcdef", 4), "abc…");
        assert_eq!(truncate("ab", 4), "ab");
    }

    #[test]
    fn wrapping_counts_screen_lines() {
        let line = Line::from(Span::raw("aaaabb"));
        let wrapped = wrap_entries(std::slice::from_ref(&line), 2);
        assert_eq!(wrapped.len(), 3);
    }
}
