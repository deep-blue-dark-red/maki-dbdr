use std::cmp::Ordering;
use crossterm::event::{KeyCode, KeyEvent};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use serde::Serialize;
use maki_providers::format_tokens;
use nucleo_matcher::pattern::{Atom, AtomKind, CaseMatching, Normalization};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::components::{ModalScroll, hint_line};
use crate::components::keybindings::key;
use crate::components::modal::Modal;
use crate::components::scrollbar::render_vertical_scrollbar;
use crate::text_buffer::TextBuffer;
use crate::theme;

const TITLE: &str = " Turn stats ";
const PREFIX: &str = "  ";
const SEARCH_PREFIX: &str = "/ ";

/// Snapshot of one completed *internal round* — one real API request. A
/// single user-submitted message (what `/goto` calls a "turn") can involve
/// several of these: the agent auto-continues internally after each tool
/// call, so `id` (the agent's own round counter) resets to 1 at the start
/// of every user turn and is only unique *within* one. `user_turn` is the
/// other numbering — same one `/goto` and the `{n}` prompt-prefix use,
/// counting user-submitted messages — and is what actually stays unique
/// across the session; it's what ties several rounds back to the one turn
/// that triggered them.
#[derive(Debug, Clone)]
pub struct TurnSnapshot {
    /// Monotonic 0-based index of this event across the whole session
    /// (every `TurnComplete` increments it). This is the simple "turn"
    /// column the user sees, replacing the old `user_turn.round` label.
    pub event_id: usize,
    pub id: usize,
    /// Which user-submitted turn (1-based, matching `/goto`'s numbering)
    /// triggered this round.
    pub user_turn: usize,
    /// Whether this event is a real human turn (the first round of a
    /// user-submitted message), as opposed to an internal continuation
    /// round auto-triggered by a prior tool call.
    pub human_turn: bool,
    /// When the UI received this turn's `TurnComplete` event.
    pub received_at: Timestamp,
    /// Fresh (uncached) input tokens the API charged for.
    pub input: u32,
    pub cache_read: u32,
    pub cache_creation: u32,
    pub output: u32,
    pub cache_miss: bool,
    /// Upstream that served this turn (aggregators only). When consecutive
    /// turns report different upstreams, a `cache_miss` between them is a
    /// routing change rather than a changed prompt prefix.
    pub upstream: Option<maki_providers::Upstream>,
    pub cost: Option<f64>,
    /// Time from dispatching the request to the response arriving.
    pub api_duration_ms: Option<u64>,
    pub ttfb_ms: Option<u64>,
    /// Retried API errors before the request ultimately succeeded.
    pub api_error_count: u32,
    /// Filled in later by `TurnToolsDone`; 0/0/0 for turns with no tool
    /// calls, or before that event has arrived yet.
    pub tool_call_count: usize,
    pub tool_error_count: usize,
    pub tool_duration_ms: u64,
    /// Per-tool-call detail (name, args, duration, error) patched in by
    /// `TurnToolsDone`. Empty until that event arrives.
    pub tool_calls: Vec<maki_agent::agent::turn_state::ToolCallRecord>,
}

/// On-disk representation of a [`TurnSnapshot`]: one JSON object per line in
/// `logs_dir()/<session_id>/turn_stats.jsonl`. Carries the per-tool-call
/// records (with their args) so the log is fully queryable offline.
#[derive(Debug, Clone, Serialize)]
pub struct PersistedTurn {
    pub event_id: usize,
    pub id: usize,
    pub user_turn: usize,
    pub human_turn: bool,
    pub received_at: String,
    pub input: u32,
    pub cache_read: u32,
    pub cache_creation: u32,
    pub output: u32,
    pub cache_miss: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream: Option<maki_providers::Upstream>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<f64>,
    pub api_duration_ms: Option<u64>,
    pub ttfb_ms: Option<u64>,
    pub api_error_count: u32,
    pub tool_call_count: usize,
    pub tool_error_count: usize,
    pub tool_duration_ms: u64,
    pub tool_calls: Vec<maki_agent::agent::turn_state::ToolCallRecord>,
}

impl From<&TurnSnapshot> for PersistedTurn {
    fn from(t: &TurnSnapshot) -> Self {
        Self {
            event_id: t.event_id,
            id: t.id,
            user_turn: t.user_turn,
            human_turn: t.human_turn,
            received_at: t.received_at.to_string(),
            input: t.input,
            cache_read: t.cache_read,
            cache_creation: t.cache_creation,
            output: t.output,
            cache_miss: t.cache_miss,
            upstream: t.upstream.clone(),
            cost: t.cost,
            api_duration_ms: t.api_duration_ms,
            ttfb_ms: t.ttfb_ms,
            api_error_count: t.api_error_count,
            tool_call_count: t.tool_call_count,
            tool_error_count: t.tool_error_count,
            tool_duration_ms: t.tool_duration_ms,
            tool_calls: t.tool_calls.clone(),
        }
    }
}

impl TurnSnapshot {
    /// Fraction of this turn's input that touched the cache in any way —
    /// read from it (`cache_read`, a hit) or written to it (`cache_creation`,
    /// establishing the cache for a later turn to hit). A turn that first
    /// writes a large system prompt into the cache has `cache_read == 0`
    /// but should still read as ~100% cached, not 0% — it just hasn't been
    /// *read back* yet. `cache_read` alone answers "how much did this turn
    /// save"; this answers "how much of this turn was cache-eligible".
    pub fn cache_rate(&self) -> f64 {
        let total = self.input + self.cache_read + self.cache_creation;
        if total == 0 {
            0.0
        } else {
            (self.cache_read + self.cache_creation) as f64 / total as f64
        }
    }

    /// API wait plus tool execution: how long the turn took end to end.
    pub fn total_duration_ms(&self) -> u64 {
        self.api_duration_ms.unwrap_or(0) + self.tool_duration_ms
    }
}

/// Which column the table is currently sorted by, and the direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortColumn {
    Turn,
    Time,
    Input,
    Cache,
    Pct,
    Out,
    Total,
    Tool,
    Api,
    ToolErr,
    ApiErr,
    Cost,
    Upstream,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SortState {
    pub column: SortColumn,
    pub desc: bool,
    pub active: bool,
}

impl Default for SortState {
    fn default() -> Self {
        Self {
            column: SortColumn::Time,
            desc: true,
            active: false,
        }
    }
}

impl SortState {
    fn next_column(mut self) -> Self {
        self.column = match self.column {
            SortColumn::Turn => SortColumn::Time,
            SortColumn::Time => SortColumn::Input,
            SortColumn::Input => SortColumn::Cache,
            SortColumn::Cache => SortColumn::Pct,
            SortColumn::Pct => SortColumn::Out,
            SortColumn::Out => SortColumn::Total,
            SortColumn::Total => SortColumn::Tool,
            SortColumn::Tool => SortColumn::Api,
            SortColumn::Api => SortColumn::ToolErr,
            SortColumn::ToolErr => SortColumn::ApiErr,
            SortColumn::ApiErr => SortColumn::Cost,
            SortColumn::Cost => SortColumn::Upstream,
            SortColumn::Upstream => SortColumn::Turn,
        };
        self.active = true;
        self
    }

    fn prev_column(mut self) -> Self {
        self.column = match self.column {
            SortColumn::Turn => SortColumn::Upstream,
            SortColumn::Time => SortColumn::Turn,
            SortColumn::Input => SortColumn::Time,
            SortColumn::Cache => SortColumn::Input,
            SortColumn::Pct => SortColumn::Cache,
            SortColumn::Out => SortColumn::Pct,
            SortColumn::Total => SortColumn::Out,
            SortColumn::Tool => SortColumn::Total,
            SortColumn::Api => SortColumn::Tool,
            SortColumn::ToolErr => SortColumn::Api,
            SortColumn::ApiErr => SortColumn::ToolErr,
            SortColumn::Cost => SortColumn::ApiErr,
            SortColumn::Upstream => SortColumn::Cost,
        };
        self.active = true;
        self
    }

    fn toggle_desc(&mut self) {
        self.desc = !self.desc;
        self.active = true;
    }

    /// Ordering of two turns under the current column and direction. For the
    /// cost column, `None` (unpriced) always sorts last regardless of the
    /// direction; the sign only orders the priced values.
    fn compare(&self, a: &TurnSnapshot, b: &TurnSnapshot) -> Ordering {
        fn num(x: f64, y: f64, desc: bool) -> Ordering {
            if desc {
                y.total_cmp(&x)
            } else {
                x.total_cmp(&y)
            }
        }
        if self.desc {
            match self.column {
                SortColumn::Turn => (b.user_turn, b.id).cmp(&(a.user_turn, a.id)),
                SortColumn::Time => b.received_at.as_second().cmp(&a.received_at.as_second()),
                SortColumn::Input => b.input.cmp(&a.input),
                SortColumn::Cache => (b.cache_read + b.cache_creation).cmp(&(a.cache_read + a.cache_creation)),
                SortColumn::Pct => num(a.cache_rate(), b.cache_rate(), true),
                SortColumn::Out => b.output.cmp(&a.output),
                SortColumn::Total => b.total_duration_ms().cmp(&a.total_duration_ms()),
                SortColumn::Tool => b.tool_duration_ms.cmp(&a.tool_duration_ms),
                SortColumn::Api => b.api_duration_ms.unwrap_or(0).cmp(&a.api_duration_ms.unwrap_or(0)),
                SortColumn::ToolErr => b.tool_error_count.cmp(&a.tool_error_count),
                SortColumn::ApiErr => b.api_error_count.cmp(&a.api_error_count),
                SortColumn::Cost => match (a.cost, b.cost) {
                    (Some(x), Some(y)) => num(x, y, true),
                    (Some(_), None) => Ordering::Less,
                    (None, Some(_)) => Ordering::Greater,
                    (None, None) => Ordering::Equal,
                },
                SortColumn::Upstream => upstream_name(b).cmp(upstream_name(a)),
            }
        } else {
            match self.column {
                SortColumn::Turn => a.event_id.cmp(&b.event_id),
                SortColumn::Time => a.received_at.as_second().cmp(&b.received_at.as_second()),
                SortColumn::Input => a.input.cmp(&b.input),
                SortColumn::Cache => (a.cache_read + a.cache_creation).cmp(&(b.cache_read + b.cache_creation)),
                SortColumn::Pct => num(a.cache_rate(), b.cache_rate(), false),
                SortColumn::Out => a.output.cmp(&b.output),
                SortColumn::Total => a.total_duration_ms().cmp(&b.total_duration_ms()),
                SortColumn::Tool => a.tool_duration_ms.cmp(&b.tool_duration_ms),
                SortColumn::Api => a.api_duration_ms.unwrap_or(0).cmp(&b.api_duration_ms.unwrap_or(0)),
                SortColumn::ToolErr => a.tool_error_count.cmp(&b.tool_error_count),
                SortColumn::ApiErr => a.api_error_count.cmp(&b.api_error_count),
                SortColumn::Cost => match (a.cost, b.cost) {
                    (Some(x), Some(y)) => num(x, y, false),
                    (Some(_), None) => Ordering::Less,
                    (None, Some(_)) => Ordering::Greater,
                    (None, None) => Ordering::Equal,
                },
                SortColumn::Upstream => upstream_name(a).cmp(upstream_name(b)),
            }
        }
    }
}

pub struct StatsModal {
    open: bool,
    scroll: ModalScroll,
    search: TextBuffer,
    searching: bool,
    matcher: Matcher,
    sort: SortState,
    header_y: Option<u16>,
    col_x: Vec<(u16, u16)>,
    last_popup: Rect,
}

impl StatsModal {
    pub fn new() -> Self {
        Self {
            open: false,
            scroll: ModalScroll::new_top(),
            search: TextBuffer::new(String::new()),
            searching: false,
            matcher: Matcher::new(Config::DEFAULT),
            sort: SortState::default(),
            header_y: None,
            col_x: Vec::new(),
            last_popup: Rect::default(),
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn toggle(&mut self) {
        self.open = !self.open;
        self.scroll.reset();
        self.search.clear();
        self.searching = false;
        self.sort = SortState::default();
    }

    pub fn close(&mut self) {
        self.open = false;
        self.scroll.reset();
        self.search.clear();
        self.searching = false;
    }

    pub fn scroll(&mut self, delta: i32) {
        self.scroll.scroll(delta);
    }

    /// Hit-test a mouse click against the header row and (re)sort. Returns
    /// true if the click landed on a header cell.
    pub fn handle_mouse_click(&mut self, row: u16, col: u16) -> bool {
        let Some(header_y) = self.header_y else {
            return false;
        };
        if row != header_y {
            return false;
        }
        let x = col.saturating_sub(self.last_popup.x + 1);
        for (idx, (start, end)) in self.col_x.iter().enumerate() {
            if x >= *start && x < *end {
                let col = column_at_index(idx);
                if self.sort.column == col {
                    self.sort.toggle_desc();
                } else {
                    self.sort.column = col;
                    self.sort.desc = false;
                    self.sort.active = true;
                }
                self.scroll.reset();
                return true;
            }
        }
        false
    }

    pub fn handle_key(&mut self, key_event: KeyEvent) {
        if self.searching {
            self.handle_search_key(key_event);
            return;
        }
        match key_event.code {
            KeyCode::Esc => self.close(),
            KeyCode::Char('/') => {
                self.searching = true;
                self.search.clear();
            }
            KeyCode::Char('<') | KeyCode::Char('[') => {
                self.sort = self.sort.prev_column();
                self.scroll.reset();
            }
            KeyCode::Char('>') | KeyCode::Char(']') => {
                self.sort = self.sort.next_column();
                self.scroll.reset();
            }
            KeyCode::Enter => {
                self.sort.toggle_desc();
                self.scroll.reset();
            }
            _ => {
                if !self.scroll.handle_key(key_event) && key::QUIT.matches(key_event) {
                    self.close();
                }
            }
        }
    }

    fn handle_search_key(&mut self, key_event: KeyEvent) {
        match key_event.code {
            KeyCode::Esc => {
                self.searching = false;
                self.search.clear();
            }
            KeyCode::Enter => self.searching = false,
            _ => {
                self.search.handle_key(key_event);
            }
        }
    }

    fn ordered_indices(&mut self, turns: &[TurnSnapshot]) -> Vec<usize> {
        let mut idx: Vec<usize> = (0..turns.len()).collect();
        if self.sort.active {
            let sort = self.sort;
            idx.sort_by(|&a, &b| sort.compare(&turns[a], &turns[b]));
        }
        let query = self.search.value();
        if query.trim().is_empty() {
            return idx;
        }
        let haystacks: Vec<String> = turns.iter().map(row_search_text).collect();
        let atom = Atom::new(
            &query,
            CaseMatching::Smart,
            Normalization::Smart,
            AtomKind::Fuzzy,
            false,
        );
        let mut buf = Vec::new();
        idx.retain(|&i| {
            buf.clear();
            atom.score(Utf32Str::new(&haystacks[i], &mut buf), &mut self.matcher)
                .is_some()
        });
        idx
    }

    pub fn view(&mut self, frame: &mut Frame, area: Rect, turns: &[TurnSnapshot]) -> Rect {
        if !self.open {
            return Rect::default();
        }

        let theme = theme::current();
        let idx = self.ordered_indices(turns);
        let lines = self.build_lines(turns, &idx, &theme);

        let total = lines.len() as u16;
        let modal = Modal {
            title: TITLE,
            width_percent: 90,
            max_height_percent: 80,
        };
        let (popup, inner) = modal.render(frame, area, total);
        self.last_popup = popup;
        let viewport_h = inner.height;
        self.scroll.update_dimensions(total, viewport_h);
        let scroll = self.scroll.offset();

        let leading = if self.searching { 3 } else { 2 };
        self.header_y = Some(inner.y + scroll.saturating_sub(leading));
        self.col_x = column_ranges();

        frame.render_widget(Paragraph::new(lines).scroll((scroll, 0)), inner);

        if total > viewport_h {
            render_vertical_scrollbar(frame, inner, total, scroll);
        }

        popup
    }

    fn build_lines(
        &mut self,
        turns: &[TurnSnapshot],
        idx: &[usize],
        theme: &crate::theme::Theme,
    ) -> Vec<Line<'static>> {
        if turns.is_empty() {
            return vec![Line::from(Span::styled(
                format!("{PREFIX}no completed turns yet"),
                theme.tool_dim,
            ))];
        }

        let total_cost: f64 = turns.iter().filter_map(|t| t.cost).sum::<f64>() + 0.0;
        let user_turns: std::collections::HashSet<usize> = turns.iter().map(|t| t.user_turn).collect();
        let mut lines = Vec::new();
        if self.searching {
            lines.push(self.search_line(theme));
        }
        lines.push(Line::from(Span::styled(
            format!(
                "{PREFIX}{} turns · {} requests · ${total_cost:.4} total",
                user_turns.len(),
                turns.len(),
            ),
            theme.keybind_section,
        )));
        lines.push(Line::default());
        lines.push(self.header_line(theme));
        for &i in idx {
            lines.push(self.row_line(&turns[i], theme));
        }
        lines.push(Line::default());
        lines.push(hint_line(&[
            ("/", "search"),
            ("[", "sort col"),
            ("]", "sort col"),
            ("Enter", "toggle dir"),
            ("click", "header sorts"),
        ]));
        lines
    }

    fn search_line(&self, theme: &crate::theme::Theme) -> Line<'static> {
        Line::from(vec![
            Span::styled(SEARCH_PREFIX, theme.keybind_section),
            Span::styled(self.search.value(), theme.foreground),
        ])
    }

    fn header_line(&self, theme: &crate::theme::Theme) -> Line<'static> {
        if !self.sort.active {
            return header_row(theme);
        }
        let cols = column_labels();
        let sort_idx = column_idx(self.sort.column);
        let mut spans: Vec<Span> = Vec::with_capacity(cols.len());
        for (i, (label, width)) in cols.iter().enumerate() {
            if i == sort_idx {
                let arrow = if self.sort.desc { "↓" } else { "↑" };
                spans.push(Span::styled(
                    format!("{label:<width$} {arrow}", width = *width as usize),
                    theme.keybind_section.add_modifier(Modifier::BOLD),
                ));
            } else {
                spans.push(Span::styled(
                    format!("{label:<width$}", width = *width as usize),
                    theme.status_dim,
                ));
            }
        }
        let mut line = Line::from(spans);
        line.spans.insert(0, Span::raw(PREFIX));
        line
    }

    fn row_line(&mut self, t: &TurnSnapshot, theme: &crate::theme::Theme) -> Line<'static> {
        let mut line = turn_row(t, theme);
        let query = self.search.value();
        if !query.trim().is_empty() {
            highlight_query(&mut line, &query, &mut self.matcher);
        }
        line
    }
}

impl crate::components::Overlay for StatsModal {
    fn is_open(&self) -> bool {
        self.is_open()
    }

    fn close(&mut self) {
        self.close()
    }
}

// Column widths shared by `header_row` and `turn_row` so the two can never
// drift apart the way they used to (the mark glyph had no header column of
// its own, and the "ms" duration suffix was appended outside its column's
// width — both silently broke alignment). Every cell — header or data,
// including any unit suffix — is rendered to exactly its named width
// before a single space is added as the column separator.
const COL_TURN: usize = 6;
const COL_TIME: usize = 19;
const COL_TOKENS: usize = 6;
const COL_PCT: usize = 4;
const COL_MARK: usize = 1;
const COL_DURATION: usize = 7;
const COL_ERR: usize = 4;
const COL_COST: usize = 8;
/// Upstream provider name for aggregators (OpenRouter). Blank for direct
/// providers, which serve every request from the same place.
const COL_UPSTREAM: usize = 12;

/// Seconds with 2 decimal places, e.g. "15.10s" — pre-rendered (suffix
/// included) so the caller right-aligns the *whole* string to
/// `COL_DURATION`, not just the number with the suffix tacked on after.
fn duration_secs(ms: u64) -> String {
    format!("{:.2}s", ms as f64 / 1000.0)
}

/// Upstream provider name, or `""` for direct providers. Sorting on it groups
/// every turn served by the same upstream together, which is what makes an
/// aggregator's routing churn visible.
fn upstream_name(t: &TurnSnapshot) -> &str {
    t.upstream
        .as_ref()
        .and_then(|u| u.name.as_deref())
        .unwrap_or("")
}

/// Clip to `width` on a char boundary so a long upstream name can't push the
/// row past its column and break the alignment every other cell relies on.
fn truncate_cell(s: &str, width: usize) -> String {
    if s.chars().count() <= width {
        return s.to_string();
    }
    s.chars().take(width.saturating_sub(1)).chain(['…']).collect()
}

fn header_row(theme: &crate::theme::Theme) -> Line<'static> {
    Line::from(Span::styled(
        format!(
            "{PREFIX}{:<COL_TURN$} {:<COL_TIME$} {:>COL_TOKENS$} {:>COL_TOKENS$} {:>COL_PCT$}% {:>COL_MARK$} {:>COL_TOKENS$} {:>COL_DURATION$} {:>COL_DURATION$} {:>COL_DURATION$} {:>COL_ERR$} {:>COL_ERR$} {:>COL_COST$} {:<COL_UPSTREAM$}",
            "turn", "time", "in", "cache", "cch", "", "out", "total", "tool", "api", "tE", "aE", "cost", "upstream",
        ),
        theme.status_dim,
    ))
}

fn turn_row(t: &TurnSnapshot, theme: &crate::theme::Theme) -> Line<'static> {
    let fg = Style::new().fg(theme.foreground);    let time = t
        .received_at
        .to_zoned(TimeZone::system())
        .strftime("%Y-%m-%d %H:%M:%S")
        .to_string();
    let mark_style = if t.cache_miss {
        theme.tool_error
    } else {
        fg
    };
    let mark = if t.cache_miss { "𐄂" } else { "✓" };
    let cost = match t.cost {
        Some(c) => format!("{c:>COL_COST$.4}"),
        None => format!("{:>COL_COST$}", "—"),
    };
    let total = duration_secs(t.total_duration_ms());
    let tool = duration_secs(t.tool_duration_ms);
    let api = duration_secs(t.api_duration_ms.unwrap_or(0));
    // Simple 0,1,2,3... index for every completed turn, replacing the old
    // `user_turn.round` label. `human_turn` marks real user turns (the rest
    // are internal continuation rounds auto-triggered by tool calls).
    let turn_label = format!("{}", t.event_id);
    let upstream = t
        .upstream
        .as_ref()
        .and_then(|u| u.name.as_deref())
        .map_or_else(String::new, |n| truncate_cell(n, COL_UPSTREAM));
    Line::from(vec![
        Span::raw(PREFIX),
        Span::styled(format!("{turn_label:<COL_TURN$} "), fg),
        Span::styled(format!("{time:<COL_TIME$} "), fg),
        Span::styled(format!("{:>COL_TOKENS$} ", format_tokens(t.input)), fg),
        Span::styled(
            format!("{:>COL_TOKENS$} ", format_tokens(t.cache_read + t.cache_creation)),
            fg,
        ),
        Span::styled(format!("{:>COL_PCT$.0}% ", t.cache_rate() * 100.0), fg),
        Span::styled(format!("{mark:>COL_MARK$} "), mark_style),
        Span::styled(format!("{:>COL_TOKENS$} ", format_tokens(t.output)), fg),
        Span::styled(format!("{total:>COL_DURATION$} "), fg),
        Span::styled(format!("{tool:>COL_DURATION$} "), fg),
        Span::styled(format!("{api:>COL_DURATION$} "), fg),
        Span::styled(format!("{:>COL_ERR$} ", t.tool_error_count), fg),
        Span::styled(format!("{:>COL_ERR$} ", t.api_error_count), fg),
        Span::styled(format!("{cost} "), fg),
        Span::styled(format!("{upstream:<COL_UPSTREAM$}"), fg),
    ])
}

/// Detail line under a turn row for one tool call: name, duration, and an
/// error marker. Indented under its turn so the per-call breakdown reads as
/// a child list of the aggregate row above.
fn tool_call_line(rec: &maki_agent::agent::turn_state::ToolCallRecord, theme: &crate::theme::Theme) -> Line<'static> {
    const PAD: &str = "    ";
    let fg = Style::new().fg(theme.foreground);
    let mark_style = if rec.is_error {
        theme.tool_error
    } else {
        fg
    };
    let mark = if rec.is_error { "✗" } else { "✓" };
    Line::from(vec![
        Span::raw(PAD),
        Span::raw("└ "),
        Span::styled(format!("{:<16}", rec.tool), fg),
        Span::styled(duration_secs(rec.duration_ms), theme.tool_dim),
        Span::styled(format!("  {mark}"), mark_style),
    ])
}

fn build_lines(turns: &[TurnSnapshot], theme: &crate::theme::Theme) -> Vec<Line<'static>> {
    if turns.is_empty() {
        return vec![Line::from(Span::styled(
            format!("{PREFIX}no completed turns yet"),
            theme.tool_dim,
        ))];
    }

    // `Sum for f64` folds from `-0.0` (the true IEEE-754 additive identity),
    // so summing zero costs (e.g. every turn used an unpriced/local model)
    // yields `-0.0`, which formats as "-0.0000" instead of "0.0000". `+
    // 0.0` normalizes the sign back to positive.
    let total_cost: f64 = turns.iter().filter_map(|t| t.cost).sum::<f64>() + 0.0;
    let user_turns: std::collections::HashSet<usize> = turns.iter().map(|t| t.user_turn).collect();
    let mut lines = vec![
        Line::from(Span::styled(
            format!(
                "{PREFIX}{} turns · {} requests · ${total_cost:.4} total",
                user_turns.len(),
                turns.len(),
            ),
            theme.keybind_section,
        )),
        Line::default(),
        header_row(theme),
    ];
    lines.extend(turns.iter().map(|t| turn_row(t, theme)));
    lines
}

/// Display column metadata: optional sort column, header label, width. The
/// mark (cache-miss ✓/𐄂) column has no label and is not sortable. Widths are
/// kept in lockstep with the `COL_*` consts so the header, rows, and mouse
/// hit-testing can never drift apart.
fn column_defs() -> [(Option<SortColumn>, &'static str, usize); 14] {
    [
        (Some(SortColumn::Turn), "turn", COL_TURN),
        (Some(SortColumn::Time), "time", COL_TIME),
        (Some(SortColumn::Input), "in", COL_TOKENS),
        (Some(SortColumn::Cache), "cache", COL_TOKENS),
        (Some(SortColumn::Pct), "cch", COL_PCT),
        (None, "", COL_MARK),
        (Some(SortColumn::Out), "out", COL_TOKENS),
        (Some(SortColumn::Total), "total", COL_DURATION),
        (Some(SortColumn::Tool), "tool", COL_DURATION),
        (Some(SortColumn::Api), "api", COL_DURATION),
        (Some(SortColumn::ToolErr), "tE", COL_ERR),
        (Some(SortColumn::ApiErr), "aE", COL_ERR),
        (Some(SortColumn::Cost), "cost", COL_COST),
        (Some(SortColumn::Upstream), "upstream", COL_UPSTREAM),
    ]
}

/// Per-column header labels/widths for rendering the sorted header line.
fn column_labels() -> Vec<(&'static str, u16)> {
    column_defs()
        .iter()
        .map(|(_, label, width)| (*label, *width as u16))
        .collect()
}

/// Absolute x-range of each display column, derived the same way rows are
/// laid out: `PREFIX`, then each cell at its width plus one separator space.
/// The pct column renders a trailing `%` so its on-screen width is one wider.
fn column_ranges() -> Vec<(u16, u16)> {
    let mut x = PREFIX.len() as u16;
    let mut ranges = Vec::with_capacity(column_defs().len());
    for (i, (_, _, width)) in column_defs().iter().enumerate() {
        let start = x;
        let extra = if i == pct_index() { 1 } else { 0 }; // the trailing '%'
        let w = (*width as u16) + extra;
        ranges.push((start, start + w));
        x = start + w + 1;
    }
    ranges
}

fn pct_index() -> usize {
    column_defs().iter().position(|(c, _, _)| *c == Some(SortColumn::Pct)).unwrap()
}

fn column_idx(col: SortColumn) -> usize {
    column_defs().iter().position(|(c, _, _)| *c == Some(col)).unwrap()
}

fn column_at_index(idx: usize) -> SortColumn {
    column_defs()[idx].0.unwrap()
}

/// Plain searchable text of a row: every field as the user would type it,
/// including token numbers, costs and durations.
fn row_search_text(t: &TurnSnapshot) -> String {
    let time = t
        .received_at
        .to_zoned(TimeZone::system())
        .strftime("%Y-%m-%d %H:%M:%S")
        .to_string();
    format!(
        "{} {time} {} {} {} {} {:.2} {} {} {} {} {:.4}",
        t.event_id,
        t.input,
        t.cache_read + t.cache_creation,
        t.cache_rate(),
        t.output,
        t.total_duration_ms() as f64 / 1000.0,
        t.tool_duration_ms as f64 / 1000.0,
        t.api_duration_ms.unwrap_or(0) as f64 / 1000.0,
        t.tool_error_count,
        t.api_error_count,
        t.cost.unwrap_or(0.0),
    )
}

/// Bold the fuzzy-matched characters within a row's spans (a row is the
/// concatenation of its spans). Rebuilds the spans so matched runs get a
/// BOLD modifier layered on their existing styling.
fn highlight_query(line: &mut Line<'static>, query: &str, matcher: &mut Matcher) {
    let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
    let atom = Atom::new(query, CaseMatching::Smart, Normalization::Smart, AtomKind::Fuzzy, false);
    let mut buf = Vec::new();
    let mut indices = Vec::new();
    if atom.indices(Utf32Str::new(&text, &mut buf), matcher, &mut indices).is_none() {
        return;
    }
    // Flatten the row into (char, style) pairs so the global match indices
    // line up with the concatenated text.
    let chars: Vec<(char, Style)> = line
        .spans
        .iter()
        .flat_map(|s| s.content.chars().map(|c| (c, s.style)))
        .collect();
    let span_count = chars.len();
    let matched: std::collections::HashSet<u32> = indices.into_iter().collect();
    let mut new_spans: Vec<Span> = Vec::new();
    let flush = |start: usize, end: usize, is_match: bool, out: &mut Vec<Span>| {
        for (c, style) in &chars[start..end] {
            let mut st = *style;
            if is_match {
                st = st.add_modifier(Modifier::BOLD);
            }
            match out.last_mut() {
                Some(last) if last.style == st => {
                    last.content.to_mut().push(*c);
                }
                _ => out.push(Span::styled(c.to_string(), st)),
            }
        }
    };
    let mut i = 0usize;
    while i < span_count {
        let is_match = matched.contains(&(i as u32));
        let mut j = i;
        while j < span_count && matched.contains(&(j as u32)) == is_match {
            j += 1;
        }
        flush(i, j, is_match, &mut new_spans);
        i = j;
    }
    line.spans = new_spans;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    fn key_event(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn sample_turn(id: usize) -> TurnSnapshot {
        TurnSnapshot {
            event_id: id,
            id,
            user_turn: id,
            human_turn: id == 1,
            received_at: Timestamp::from_second(1_700_000_000).unwrap(),
            input: 1200,
            cache_read: 800,
            cache_creation: 0,
            output: 340,
            cache_miss: false,
            upstream: None,
            cost: Some(0.0123),
            api_duration_ms: Some(1400),
            ttfb_ms: Some(300),
            api_error_count: 0,
            tool_call_count: 2,
            tool_error_count: 1,
            tool_duration_ms: 500,
            tool_calls: Vec::new(),
        }
    }

    #[test]
    fn toggle_open_close() {
        let mut modal = StatsModal::new();
        assert!(!modal.is_open());
        modal.toggle();
        assert!(modal.is_open());
        modal.toggle();
        assert!(!modal.is_open());
    }

    #[test]
    fn handle_key_esc_closes() {
        let mut modal = StatsModal::new();
        modal.toggle();
        modal.handle_key(key_event(KeyCode::Esc));
        assert!(!modal.is_open());
    }

    #[test]
    fn cache_rate_and_total_duration_are_computed() {
        let t = sample_turn(1);
        assert!((t.cache_rate() - 0.4).abs() < 1e-9);
        assert_eq!(t.total_duration_ms(), 1900);
    }

    /// Durations render as seconds (2dp) with the *whole* string —
    /// including the "s" suffix — right-aligned to match the header.
    /// Regression: the old `"{:>6}ms"` format aligned only the number,
    /// leaving "ms" to blow past the header's column width.
    #[test]
    fn duration_columns_are_seconds_and_right_aligned_with_header() {
        // Rust's `{:>N}` pads by *character* count, not byte length — a
        // multi-byte glyph like the cache-miss mark (✓/𐄂) earlier in the
        // row would throw off a raw byte-offset comparison even when the
        // rendered columns line up. Compare char counts instead.
        fn char_count_to(s: &str, needle: &str) -> usize {
            let byte_idx = s.find(needle).unwrap();
            s[..byte_idx].chars().count() + needle.chars().count()
        }

        let theme = theme::current();
        let header_text: String = header_row(&theme)
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        let row_text: String = turn_row(&sample_turn(1), &theme)
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();

        // api_duration_ms=1400, tool_duration_ms=500, total=1900.
        for (label, value) in [("total", "1.90s"), ("tool", "0.50s"), ("api", "1.40s")] {
            assert!(row_text.contains(value), "got: {row_text:?}");
            let header_col_end = char_count_to(&header_text, label);
            let row_col_end = char_count_to(&row_text, value);
            assert_eq!(
                header_col_end, row_col_end,
                "{label:?} column misaligned\nheader: {header_text:?}\nrow:    {row_text:?}"
            );
        }
    }

    /// A turn that *establishes* the cache (writes the system prompt/tools
    /// in for the first time) has `cache_read == 0` — nothing existed to
    /// read yet — but should still read as ~100% cached, not 0%. Regression
    /// for conflating "cache hit rate" with "cache activity rate": the
    /// former is legitimately 0 here, but the latter (what's actually
    /// shown) must count the write.
    #[test]
    fn cache_rate_counts_cache_creation_not_just_reads() {
        let mut t = sample_turn(1);
        t.input = 0;
        t.cache_read = 0;
        t.cache_creation = 10_000;
        assert!((t.cache_rate() - 1.0).abs() < 1e-9, "got: {}", t.cache_rate());
    }

    /// A later turn mostly re-reading an already-established cache, with a
    /// small new increment of fresh input, should read as ~95% cached.
    #[test]
    fn cache_rate_counts_reads_of_an_established_cache() {
        let mut t = sample_turn(1);
        t.input = 500;
        t.cache_read = 9_500;
        t.cache_creation = 0;
        assert!((t.cache_rate() - 0.95).abs() < 1e-9, "got: {}", t.cache_rate());
    }

    #[test]
    fn cache_rate_is_zero_when_no_input() {
        let mut t = sample_turn(1);
        t.input = 0;
        t.cache_read = 0;
        t.cache_creation = 0;
        assert_eq!(t.cache_rate(), 0.0);
    }

    #[test]
    fn empty_history_shows_placeholder_line() {
        let theme = theme::current();
        let lines = build_lines(&[], &theme);
        assert_eq!(lines.len(), 1);
        assert!(
            lines[0]
                .spans
                .iter()
                .any(|s| s.content.contains("no completed turns yet"))
        );
    }

    /// Regression: summing zero costs (e.g. every turn used an unpriced
    /// local model, so every `cost` is `None`) must show "$0.0000", not
    /// "$-0.0000" — `Sum for f64` folds from `-0.0`, so a naive
    /// `filter_map(..).sum()` over an all-`None` set produces negative
    /// zero.
    #[test]
    fn summary_line_shows_positive_zero_when_no_turn_has_a_cost() {
        let theme = theme::current();
        let mut t1 = sample_turn(1);
        t1.cost = None;
        let mut t2 = sample_turn(2);
        t2.cost = None;
        let lines = build_lines(&[t1, t2], &theme);
        let summary: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(summary.contains("$0.0000"), "got: {summary:?}");
        assert!(!summary.contains('-'), "got: {summary:?}");
    }

    #[test]
    fn build_lines_includes_header_and_one_row_per_turn() {
        let theme = theme::current();
        let turns = vec![sample_turn(1), sample_turn(2)];
        let lines = build_lines(&turns, &theme);
        // summary + blank + header + 2 rows
        assert_eq!(lines.len(), 5);
    }

    /// Regression: a single user turn that triggers tool calls produces
    /// several rounds (agent id resets to 1 each user turn, so multiple
    /// rows can share the same `id`). The summary's turn count must count
    /// distinct `user_turn` values, not rows — otherwise "5 rounds across
    /// 2 real turns" gets mislabeled as "5 turns".
    #[test]
    fn summary_counts_distinct_user_turns_not_rounds() {
        let theme = theme::current();
        let mut r1 = sample_turn(1);
        r1.user_turn = 1;
        let mut r2 = sample_turn(2);
        r2.user_turn = 1;
        let mut r3 = sample_turn(1);
        r3.user_turn = 2;
        let lines = build_lines(&[r1, r2, r3], &theme);
        let summary: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(summary.contains("2 turns"), "got: {summary:?}");
        assert!(summary.contains("3 requests"), "got: {summary:?}");
    }

    #[test]
    fn turn_row_shows_cache_miss_mark() {
        let theme = theme::current();
        let mut t = sample_turn(1);
        t.cache_miss = true;
        let line = turn_row(&t, &theme);
        assert!(line.spans.iter().any(|s| s.content.contains('𐄂')));
    }

    #[test]
    fn turn_label_uses_event_id() {
        let theme = theme::current();
        let first_turn = sample_turn(1);
        let line = turn_row(&first_turn, &theme);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text.split_whitespace().next(), Some("1"));

        let second_turn = sample_turn(2);
        let line = turn_row(&second_turn, &theme);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text.split_whitespace().next(), Some("2"));
    }

    #[test]
    fn sorting_by_cost_ascending_and_descending() {
        let mut t1 = sample_turn(1);
        t1.cost = Some(0.05);
        let mut t2 = sample_turn(2);
        t2.cost = Some(0.01);
        let mut t3 = sample_turn(3);
        t3.cost = None;
        let turns = vec![t1, t2, t3];

        let mut m = StatsModal::new();
        m.sort.column = SortColumn::Cost;
        m.sort.desc = false;
        m.sort.active = true;
        let idx = m.ordered_indices(&turns);
        assert_eq!(idx, vec![1, 0, 2], "ascending: 0.01, 0.05, None");

        m.sort.desc = true;
        let idx = m.ordered_indices(&turns);
        assert_eq!(idx, vec![0, 1, 2], "descending: 0.05, 0.01, None");
    }

    #[test]
    fn search_filters_rows_fuzzy() {
        let turns = vec![sample_turn(1), sample_turn(2)];
        let mut m = StatsModal::new();

        // Empty search returns every row.
        m.searching = true;
        let idx = m.ordered_indices(&turns);
        assert_eq!(idx.len(), turns.len());

        // A query absent from every row matches nothing.
        for c in "zzzzzz".chars() {
            m.search.push_char(c);
        }
        let idx = m.ordered_indices(&turns);
        assert!(idx.is_empty(), "got: {idx:?}");
    }

    #[test]
    fn mouse_click_on_header_sorts_and_toggles_direction() {
        let mut m = StatsModal::new();
        m.last_popup = Rect::new(0, 0, 100, 100);
        m.header_y = Some(3);
        m.col_x = column_ranges();
        let (start, _end) = column_ranges()[column_idx(SortColumn::Cost)];

        assert!(m.handle_mouse_click(3, m.last_popup.x + 1 + start));
        assert_eq!(m.sort.column, SortColumn::Cost);
        assert!(!m.sort.desc);
        assert!(m.sort.active);

        // Second click on the same column toggles direction.
        assert!(m.handle_mouse_click(3, m.last_popup.x + 1 + start));
        assert!(m.sort.desc);

        // A click on a data row (not the header) is not consumed.
        assert!(!m.handle_mouse_click(5, m.last_popup.x + 1 + start));
    }

    #[test]
    fn sort_cycling_returns_to_start_after_all_columns() {
        // Derived from `column_defs` so adding a column can't silently leave
        // the cycle short — the two used to drift independently. Only the
        // sortable columns are in the cycle; the mark column has no key.
        let n = column_defs().iter().filter(|(c, _, _)| c.is_some()).count();
        let mut col = SortColumn::Cost;
        for _ in 0..n {
            let st = SortState {
                column: col,
                desc: false,
                active: true,
            };
            col = st.next_column().column;
        }
        assert_eq!(col, SortColumn::Cost, "cycling {n} columns returns to start");
    }
}
