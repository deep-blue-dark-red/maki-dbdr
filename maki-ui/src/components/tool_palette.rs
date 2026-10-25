//! Completion for `$tool` input, mirroring the `/` command palette: the
//! input line is the query, `Tab` completes the selected row inline, `Enter`
//! submits through the normal path. Two phases: the tool name after the
//! sigil, then the tool's schema parameters (and enum values after `=`).

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use crossterm::event::{KeyCode, KeyEvent};
use maki_agent::tools::registry::RegistrySnapshot;
use maki_agent::tools::{DescriptionContext, ToolAudience, ToolFilter, ToolRegistry};
use maki_storage::paths::{project_dir, project_root};
use maki_storage::StateDir;
use nucleo::{Config, Matcher, Nucleo, Utf32String, pattern::{CaseMatching, Normalization}};
use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::{Clear, Paragraph},
};

use crate::app::user_tool::{SchemaProp, is_param_name, schema_props};
use crate::theme;

const TICK_TIMEOUT_MS: u64 = 2;
const DETAIL_MAX_CHARS: usize = 80;
/// Mirrors the custom_tools plugin's registry depth (`MAX_REL_DEPTH`).
const CUSTOM_MAX_DEPTH: usize = 32;
const CUSTOM_DIR: &str = "custom_tools";
const CUSTOM_META_SUFFIX: &str = ".yaml";

pub enum ToolAction {
    Consumed,
    /// Replace the whole input with this text (the completed form).
    Complete(String),
    Passthrough,
}

#[derive(Clone)]
struct ToolItem {
    name: String,
    detail: String,
    /// `Typed` renders the leading type token in `inline_code`, the rest
    /// dim; parameter rows use it.
    kind: Detail,
    /// Registry tools complete to `$name `; a custom_tools registration
    /// completes to the `$bash ...` line that runs it.
    insert: Option<String>,
}

#[derive(Clone)]
enum Detail {
    Text,
    Typed { type_name: String, rest: String },
}

struct Match {
    index: usize,
    indices: Vec<u32>,
}

enum Phase {
    Closed,
    /// Completing the tool name: `$rea` → `$read `.
    Name { hidden: bool },
    /// Completing a parameter name: `$read pa` → `$read path=`.
    ArgKey { prefix: String },
    /// Completing an enum parameter's value: `$mode m=bui` → `$mode m=build `.
    ArgValue { prefix: String },
}

pub struct ToolPalette {
    selected: usize,
    filtered: Vec<Match>,
    items: Vec<ToolItem>,
    candidate_names: Vec<String>,
    known_registry_names: Vec<Arc<str>>,
    pattern: String,
    phase: Phase,
    registry_items: Vec<ToolItem>,
    custom_items: Vec<ToolItem>,
    custom_dir: Option<PathBuf>,
    custom_mtime: Option<SystemTime>,
    nucleo: Nucleo<ToolItem>,
    matcher: Matcher,
}

impl ToolPalette {
    pub fn new() -> Self {
        Self {
            selected: 0,
            filtered: Vec::new(),
            items: Vec::new(),
            candidate_names: Vec::new(),
            known_registry_names: Vec::new(),
            pattern: String::new(),
            phase: Phase::Closed,
            registry_items: Vec::new(),
            custom_items: Vec::new(),
            custom_dir: None,
            custom_mtime: None,
            nucleo: Nucleo::new(Config::DEFAULT, Arc::new(|| {}), None, 1),
            matcher: Matcher::new(Config::DEFAULT),
        }
    }

    pub fn is_active(&self) -> bool {
        !self.filtered.is_empty()
    }

    pub fn sync(&mut self, input: &str, storage: &StateDir, cwd: &str) {
        let Some(rest) = input.strip_prefix('$') else {
            self.close();
            return;
        };
        let hidden = rest.starts_with('$');
        let body = &input[if hidden { 2 } else { 1 }..];
        if body.starts_with(' ') {
            self.close();
            return;
        }
        match body.split_once(char::is_whitespace) {
            None => self.sync_name(body, hidden, storage, cwd),
            Some((name, args)) => self.sync_args(input, name, args),
        }
    }

    fn sync_name(&mut self, word: &str, hidden: bool, storage: &StateDir, cwd: &str) {
        self.phase = Phase::Name { hidden };
        let registry = ToolRegistry::global();
        let names = registry.names();
        let registry_changed = names != self.known_registry_names;
        if registry_changed {
            self.known_registry_names = names;
            self.registry_items = registry_descriptions(registry.iter());
        }
        self.refresh_custom(storage, cwd);
        // Always re-set: the args phase leaves the parameter items in place,
        // and `set_candidates` itself skips the matcher rebuild when the
        // names have not changed.
        {
            let registry_names: HashSet<&str> = self
                .registry_items
                .iter()
                .map(|item| item.name.as_str())
                .collect();
            let items = self
                .registry_items
                .clone()
                .into_iter()
                .chain(
                    self.custom_items
                        .iter()
                        .filter(|item| !registry_names.contains(item.name.as_str()))
                        .cloned(),
                )
                .collect::<Vec<_>>();
            self.set_candidates(items);
        }
        self.run_pattern(word);
    }

    /// Rescans the custom_tools registrations when their directory changed
    /// since the last look; one `stat` per keystroke otherwise.
    fn refresh_custom(&mut self, storage: &StateDir, cwd: &str) -> bool {
        let dir = project_dir(storage, Path::new(cwd)).join(CUSTOM_DIR);
        let mtime = fs::metadata(&dir)
            .and_then(|meta| meta.modified())
            .ok();
        if self.custom_dir.as_deref() == Some(dir.as_path()) && mtime == self.custom_mtime {
            return false;
        }
        let root = project_root(Path::new(cwd));
        self.custom_items = scan_custom_tools(&dir, &root, Path::new(cwd));
        self.custom_dir = Some(dir);
        self.custom_mtime = mtime;
        true
    }

    /// The name is settled; complete the trailing word of the arguments, or
    /// an enum value when it already carries a `key=`.
    fn sync_args(&mut self, input: &str, name: &str, args: &str) {
        let Some(entry) = ToolRegistry::global().get(name) else {
            self.close();
            return;
        };
        let schema = entry.tool.schema();
        let (prior, word) = match args.rsplit_once(char::is_whitespace) {
            Some((prior, word)) => (prior, word),
            None => ("", args),
        };
        let word_start = input.len() - word.len();
        let used: Vec<&str> = prior
            .split_whitespace()
            .filter_map(|token| token.split_once('='))
            .filter(|(key, _)| is_param_name(key))
            .map(|(key, _)| key)
            .collect();

        match word.split_once('=') {
            Some((key, value)) => {
                let Some(prop) = schema_props(&schema)
                    .into_iter()
                    .find(|prop| prop.name == key)
                else {
                    self.close();
                    return;
                };
                if prop.variants.is_empty() {
                    self.close();
                    return;
                }
                self.phase = Phase::ArgValue {
                    prefix: input[..word_start + key.len() + 1].to_owned(),
                };
                let detail = prop_detail(&prop);
                let candidates = prop
                    .variants
                    .clone()
                    .into_iter()
                    .map(|v| ToolItem {
                        detail: detail.clone(),
                        kind: Detail::Text,
                        insert: None,
                        name: v,
                    })
                    .collect::<Vec<_>>();
                self.set_candidates(candidates);
                self.run_pattern(value);
            }
            None => {
                self.phase = Phase::ArgKey {
                    prefix: input[..word_start].to_owned(),
                };
                let candidates = schema_props(&schema)
                    .into_iter()
                    .filter(|prop| !used.contains(&prop.name.as_str()))
                    .map(|prop| {
                        let detail = prop_detail(&prop);
                        let type_name = prop.type_name.clone();
                        ToolItem {
                            kind: Detail::Typed {
                                type_name,
                                rest: detail
                                    .strip_prefix(&prop.type_name)
                                    .unwrap_or(&detail)
                                    .to_owned(),
                            },
                            insert: None,
                            detail,
                            name: prop.name,
                        }
                    })
                    .collect::<Vec<_>>();
                self.set_candidates(candidates);
                self.run_pattern(word);
            }
        }
    }

    fn set_candidates(&mut self, items: Vec<ToolItem>) {
        let names: Vec<String> = items.iter().map(|i| i.name.clone()).collect();
        if names == self.candidate_names {
            self.items = items;
            return;
        }
        self.candidate_names = names;
        self.items = items;
        let nucleo = Nucleo::new(Config::DEFAULT, Arc::new(|| {}), None, 1);
        let injector = nucleo.injector();
        for item in &self.items {
            injector.push(
                ToolItem {
                    name: item.name.clone(),
                    detail: String::new(),
                    kind: Detail::Text,
                    insert: None,
                },
                |item, cols| cols[0] = Utf32String::from(item.name.as_str()),
            );
        }
        self.nucleo = nucleo;
    }

    fn run_pattern(&mut self, pattern: &str) {
        self.pattern = pattern.to_owned();
        self.nucleo.pattern.reparse(
            0,
            pattern,
            CaseMatching::Ignore,
            Normalization::Smart,
            false,
        );
        self.tick();
        self.refresh_matches();
        // A full name beats any fuzzy neighbor, so `$read` cannot complete to
        // `read_file` while both match, and `path=` cannot complete to
        // `path_suffix`.
        if let Some(index) = self
            .filtered
            .iter()
            .position(|m| self.items[m.index].name.eq_ignore_ascii_case(pattern))
        {
            let exact = self.filtered.remove(index);
            self.filtered.insert(0, exact);
            self.selected = 0;
        }
    }

    fn tick(&mut self) {
        loop {
            let status = self.nucleo.tick(TICK_TIMEOUT_MS);
            if status.changed {
                self.refresh_matches();
            }
            if !status.running {
                break;
            }
        }
    }

    fn refresh_matches(&mut self) {
        let snapshot = self.nucleo.snapshot();
        let pattern = snapshot.pattern();
        let has_pattern = !pattern.column_pattern(0).atoms.is_empty();

        self.filtered.clear();
        let count = snapshot.matched_item_count();
        let items = &self.items;
        for item in snapshot.matched_items(0..count) {
            let name = &item.data.name;
            let Some(index) = items.iter().position(|i| i.name == *name) else {
                continue;
            };
            let indices = if has_pattern {
                let mut buf = Vec::new();
                pattern.column_pattern(0).indices(
                    item.matcher_columns[0].slice(..),
                    &mut self.matcher,
                    &mut buf,
                );
                buf
            } else {
                Vec::new()
            };
            self.filtered.push(Match { index, indices });
        }
        self.selected = self.selected.min(self.filtered.len().saturating_sub(1));
    }

    pub fn close(&mut self) {
        self.filtered.clear();
        self.selected = 0;
        self.phase = Phase::Closed;
    }

    fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    fn move_down(&mut self) {
        if self.selected + 1 < self.filtered.len() {
            self.selected += 1;
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> ToolAction {
        if !self.is_active() {
            return ToolAction::Passthrough;
        }
        match key.code {
            KeyCode::Up => {
                self.move_up();
                ToolAction::Consumed
            }
            KeyCode::Down => {
                self.move_down();
                ToolAction::Consumed
            }
            KeyCode::Esc => {
                self.close();
                ToolAction::Consumed
            }
            KeyCode::Enter => {
                // Enter accepts the row like Tab, except mid-arguments with
                // nothing being typed: `$tool ` then Enter submits the call
                // as typed rather than forcing a parameter in.
                if matches!(self.phase, Phase::ArgKey { .. }) && self.pattern.is_empty() {
                    return ToolAction::Passthrough;
                }
                self.accept_selected()
            }
            KeyCode::Tab => self.accept_selected(),
            _ => ToolAction::Passthrough,
        }
    }

    fn accept_selected(&mut self) -> ToolAction {
        match self.filtered.get(self.selected) {
            Some(m) => {
                let item = &self.items[m.index];
                let text = match (&self.phase, &item.insert) {
                    (_, Some(target)) => format!("$bash {target} "),
                    (Phase::Name { hidden }, None) => {
                        let sigil = if *hidden { "$$" } else { "$" };
                        format!("{sigil}{} ", item.name)
                    }
                    (Phase::ArgKey { prefix }, None) => format!("{prefix}{}=", item.name),
                    (Phase::ArgValue { prefix }, None) => format!("{prefix}{} ", item.name),
                    (Phase::Closed, None) => return ToolAction::Consumed,
                };
                ToolAction::Complete(text)
            }
            None => ToolAction::Consumed,
        }
    }

    pub fn view(&self, frame: &mut Frame, input_area: Rect) {
        if self.filtered.is_empty() {
            return;
        }
        let popup_height = (self.filtered.len() as u16).min(input_area.y);
        if popup_height == 0 {
            return;
        }

        const GAP: usize = 2;
        let max_name = self
            .filtered
            .iter()
            .map(|m| self.items[m.index].name.len())
            .max()
            .unwrap_or(0);
        let max_detail = self
            .filtered
            .iter()
            .map(|m| self.items[m.index].detail.chars().count())
            .max()
            .unwrap_or(0);
        const PAD: usize = 1;
        let popup_width = (PAD + max_name + GAP + max_detail + PAD) as u16;

        let popup = Rect {
            x: input_area.x,
            y: input_area.y.saturating_sub(popup_height),
            width: popup_width.min(input_area.width),
            height: popup_height,
        };

        let t = theme::current();
        let lines: Vec<Line> = self
            .filtered
            .iter()
            .enumerate()
            .map(|(i, m)| {
                let item = &self.items[m.index];
                let selected = i == self.selected;
                let name_pad =
                    max_name - item.name.chars().count() + GAP;

                if selected {
                    let s = t.item_selected;
                    let mut spans = vec![Span::styled(" ".repeat(PAD), s)];
                    spans.extend(highlighted_spans(
                        &item.name,
                        &m.indices,
                        s,
                        t.item_match_selected,
                    ));
                    spans.push(Span::styled(" ".repeat(name_pad), s));
                    spans.push(Span::styled(item.detail.clone(), s));
                    spans.push(Span::styled(" ".repeat(PAD), s));
                    Line::from(spans)
                } else {
                    let (name_style, detail_spans) = match (&self.phase, &item.kind) {
                        // A tool name wears the color tool names wear in the
                        // transcript; a parameter wears the parameter scope
                        // the user's syntax palette already defines. A
                        // custom_tools registration is a script, so it wears
                        // the code accent.
                        (Phase::Name { .. }, _) if item.insert.is_some() => (
                            t.inline_code,
                            vec![Span::styled(item.detail.clone(), t.item_desc)],
                        ),
                        (Phase::Name { .. }, _) => (
                            t.tool,
                            vec![Span::styled(item.detail.clone(), t.item_desc)],
                        ),
                        (Phase::ArgKey { .. }, Detail::Typed { type_name, rest }) => (
                            t.code_gutter,
                            vec![
                                Span::styled(type_name.clone(), t.inline_code),
                                Span::styled(rest.clone(), t.item_desc),
                            ],
                        ),
                        _ => (t.item, vec![Span::styled(item.detail.clone(), t.item_desc)]),
                    };
                    let mut spans = vec![Span::raw(" ".repeat(PAD))];
                    spans.extend(highlighted_spans(
                        &item.name,
                        &m.indices,
                        name_style,
                        t.item_match,
                    ));
                    spans.push(Span::raw(" ".repeat(name_pad)));
                    spans.extend(detail_spans);
                    spans.push(Span::raw(" ".repeat(PAD)));
                    Line::from(spans)
                }
            })
            .collect();

        frame.render_widget(Clear, popup);
        frame.render_widget(
            Paragraph::new(lines).style(Style::new().bg(t.background)),
            popup,
        );
    }
}

/// The one-line popup detail for a schema property: `type — description`.
fn prop_detail(prop: &SchemaProp) -> String {
    let mut detail = prop.type_name.clone();
    if !prop.description.is_empty() {
        detail.push_str(" — ");
        let desc: String = prop
            .description
            .lines()
            .next()
            .unwrap_or_default()
            .to_owned();
        detail.push_str(&desc);
    }
    let mut detail: String = detail.chars().take(DETAIL_MAX_CHARS).collect();
    if detail.chars().count() == DETAIL_MAX_CHARS {
        detail.push('…');
    }
    detail
}

fn registry_descriptions(snapshot: RegistrySnapshot) -> Vec<ToolItem> {
    let desc_ctx = DescriptionContext {
        filter: &ToolFilter::All,
        audience: ToolAudience::MAIN,
        workflow: false,
        mcp: true,
    };
    snapshot
        .iter()
        .map(|entry| {
            let first_line = entry
                .tool
                .description(&desc_ctx)
                .lines()
                .next()
                .unwrap_or_default()
                .to_owned();
            let mut detail: String = first_line.chars().take(DETAIL_MAX_CHARS).collect();
            if detail.chars().count() == DETAIL_MAX_CHARS {
                detail.push('…');
            }
            ToolItem {
                name: entry.name().to_owned(),
                detail,
                kind: Detail::Text,
                insert: None,
            }
        })
        .collect()
}

/// Reads the custom_tools plugin's registrations from
/// `<project_dir>/custom_tools/**/*.yaml` — the same flat
/// `{tags, description, body}` files its own list/read actions answer from.
/// A registration with a `body` is a command tool and completes to its body;
/// otherwise it names a project file, which completes to that file's path.
fn scan_custom_tools(dir: &Path, root: &Path, cwd: &Path) -> Vec<ToolItem> {
    let mut files = Vec::new();
    collect_yaml_files(dir, dir, 0, &mut files);
    let mut items = Vec::new();
    for rel in files {
        let rel_name = rel.to_string_lossy().into_owned();
        let Some(name) = rel_name.strip_suffix(CUSTOM_META_SUFFIX) else {
            continue;
        };
        // Legacy registrations lived under a `.meta/` prefix.
        let name = name.trim_start_matches(".meta/").to_owned();
        if name.is_empty() {
            continue;
        }
        let Ok(content) = fs::read_to_string(dir.join(&rel)) else {
            continue;
        };
        let Ok(meta) = serde_yaml::from_str::<serde_yaml::Value>(&content) else {
            continue;
        };
        let description = meta
            .get("description")
            .and_then(serde_yaml::Value::as_str)
            .unwrap_or_default();
        let body = meta
            .get("body")
            .and_then(serde_yaml::Value::as_str)
            .filter(|body| !body.is_empty());
        let target = match body {
            Some(body) => body.to_owned(),
            None => root
                .join(&name)
                .strip_prefix(cwd)
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|_| root.join(&name).to_string_lossy().into_owned()),
        };
        let fallback = if body.is_some() { "command" } else { name.as_str() };
        let source = if description.is_empty() {
            fallback.to_owned()
        } else {
            description.to_owned()
        };
        let mut detail = format!("custom — {source}");
        if detail.chars().count() > DETAIL_MAX_CHARS {
            detail = detail.chars().take(DETAIL_MAX_CHARS).collect();
            detail.push('…');
        }
        items.push(ToolItem {
            name,
            detail,
            kind: Detail::Text,
            insert: Some(target),
        });
    }
    items
}

fn collect_yaml_files(base: &Path, dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > CUSTOM_MAX_DEPTH {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_yaml_files(base, &path, depth + 1, out);
        } else if path.extension().is_some_and(|ext| ext == "yaml")
            && let Ok(rel) = path.strip_prefix(base)
        {
            out.push(rel.to_path_buf());
        }
    }
}

fn highlighted_spans(
    text: &str,
    indices: &[u32],
    base: Style,
    matched: Style,
) -> Vec<Span<'static>> {
    if indices.is_empty() {
        return vec![Span::styled(text.to_owned(), base)];
    }
    let mut spans = Vec::with_capacity(indices.len() + 1);
    let mut start = 0usize;
    let mut indices = indices.iter().copied().peekable();
    while let Some(i) = indices.next() {
        let i = i as usize;
        if i > start {
            spans.push(Span::styled(text[start..i].to_owned(), base));
        }
        let mut end = i + 1;
        while indices.peek().is_some_and(|&next| next as usize == end) {
            indices.next();
            end += 1;
        }
        spans.push(Span::styled(text[i..end].to_owned(), matched));
        start = end;
    }
    if start < text.len() {
        spans.push(Span::styled(text[start..].to_owned(), base));
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;
    use maki_agent::tools::registry::ToolSource;
    use maki_agent::tools::test_support::{mock_tool, mock_tool_with_schema};
    use maki_agent::tools::ToolAudience;
    use serde_json::{Value, json};
    use tempfile::TempDir;
    use test_case::test_case;

    const PROBE_A: &str = "tp_alpha";
    const PROBE_B: &str = "tp_beta";
    fn probe_schema() -> Value {
        json!({
            "type": "object",
            "required": ["path"],
            "properties": {
                "limit": {"type": "integer", "description": "max lines"},
                "mode": {"type": "string", "enum": ["build", "plan"]},
                "path": {"type": "string", "description": "file to read"}
            }
        })
    }

    fn register_probes() {
        let reg = ToolRegistry::global();
        let _ = reg.register(
            mock_tool_with_schema(PROBE_A, ToolAudience::MAIN, probe_schema()),
            ToolSource::Lua { plugin: "tp".into() },
        );
        let _ = reg.register(
            mock_tool(PROBE_B, ToolAudience::MAIN),
            ToolSource::Lua { plugin: "tp".into() },
        );
    }

    struct TestCtx {
        storage: StateDir,
        cwd: PathBuf,
        _tmp: TempDir,
    }

    impl TestCtx {
        fn new() -> Self {
            let tmp = TempDir::new().unwrap();
            Self {
                storage: StateDir::from_path(tmp.path().join("state")),
                cwd: tmp.path().to_path_buf(),
                _tmp: tmp,
            }
        }

        fn sync(&self, palette: &mut ToolPalette, input: &str) {
            palette.sync(input, &self.storage, &self.cwd.to_string_lossy());
        }

        fn register_custom(&self, name: &str, yaml: &str) {
            let dir = project_dir(&self.storage, &self.cwd).join(CUSTOM_DIR);
            fs::create_dir_all(dir.join(Path::new(name).parent().unwrap())).unwrap();
            fs::write(dir.join(format!("{name}{CUSTOM_META_SUFFIX}")), yaml).unwrap();
        }

        fn palette(&self) -> ToolPalette {
            register_probes();
            let mut p = ToolPalette::new();
            self.sync(&mut p, "$");
            p
        }
    }

    fn names(p: &ToolPalette) -> Vec<String> {
        p.filtered
            .iter()
            .map(|m| p.items[m.index].name.clone())
            .collect()
    }

    fn tab(p: &mut ToolPalette) -> String {
        match p.handle_key(key(KeyCode::Tab)) {
            ToolAction::Complete(text) => text,
            _ => panic!("tab must complete"),
        }
    }

    #[test_case("" ; "non_tool_input")]
    #[test_case("read a file" ; "plain_message")]
    #[test_case("!ls" ; "shell_prefix")]
    #[test_case("$ read src/main.rs" ; "space_after_sigil")]
    #[test_case("$unknown_tool x" ; "unknown_tool")]
    #[test_case("$tp_alpha limit=5 limit=6 li" ; "key_set_twice_not_offered_again")]
    #[test_case("$tp_alpha path=x" ; "non_enum_value")]
    fn sync_closes(input: &str) {
        let ctx = TestCtx::new();
        let mut p = ctx.palette();
        ctx.sync(&mut p, input);
        assert!(!p.is_active(), "popup must be down for {input:?}");
    }

    #[test]
    fn bare_sigil_lists_tools() {
        let ctx = TestCtx::new();
        let p = ctx.palette();
        let listed = names(&p);
        assert!(listed.iter().any(|n| n == PROBE_A));
        assert!(listed.iter().any(|n| n == PROBE_B));
    }

    #[test]
    fn prefix_narrows() {
        let ctx = TestCtx::new();
        let mut p = ctx.palette();
        ctx.sync(&mut p, "$tp_al");
        let listed = names(&p);
        assert!(listed.contains(&PROBE_A.to_owned()));
        assert!(!listed.contains(&PROBE_B.to_owned()));
    }

    #[test]
    fn exact_match_pins_first() {
        let ctx = TestCtx::new();
        let mut p = ctx.palette();
        ctx.sync(&mut p, &format!("${PROBE_B}"));
        assert_eq!(names(&p).first(), Some(&PROBE_B.to_owned()));
        assert_eq!(p.selected, 0);
    }

    #[test]
    fn args_space_lists_unset_params() {
        let ctx = TestCtx::new();
        let mut p = ctx.palette();
        ctx.sync(&mut p, &format!("${PROBE_A} "));
        // `path` is required, so it leads the list.
        assert_eq!(names(&p), vec!["path", "limit", "mode"]);
    }

    #[test]
    fn args_used_keys_excluded() {
        let ctx = TestCtx::new();
        let mut p = ctx.palette();
        ctx.sync(&mut p, &format!("${PROBE_A} path=a.rs "));
        assert_eq!(names(&p), vec!["limit", "mode"]);
    }

    #[test]
    fn args_key_fragment_narrows() {
        let ctx = TestCtx::new();
        let mut p = ctx.palette();
        ctx.sync(&mut p, &format!("${PROBE_A} pa"));
        assert_eq!(names(&p), vec!["path"]);
    }

    #[test]
    fn args_tab_completes_key_with_prefix() {
        let ctx = TestCtx::new();
        let mut p = ctx.palette();
        ctx.sync(&mut p, &format!("${PROBE_A} limit=5 pa"));
        assert_eq!(tab(&mut p), format!("${PROBE_A} limit=5 path="));
    }

    #[test]
    fn args_enum_value_offered_and_completed() {
        let ctx = TestCtx::new();
        let mut p = ctx.palette();
        ctx.sync(&mut p, &format!("${PROBE_A} mode="));
        assert_eq!(names(&p), vec!["build", "plan"]);
        ctx.sync(&mut p, &format!("${PROBE_A} mode=pla"));
        assert_eq!(tab(&mut p), format!("${PROBE_A} mode=plan "));
    }

    #[test]
    fn args_hidden_sigil_preserved_through_key_completion() {
        let ctx = TestCtx::new();
        let mut p = ctx.palette();
        ctx.sync(&mut p, &format!("$${PROBE_A} pa"));
        assert_eq!(tab(&mut p), format!("$${PROBE_A} path="));
    }

    #[test_case("$" ; "visible")]
    #[test_case("$$" ; "hidden")]
    fn tab_completes_name_with_sigil(sigil: &str) {
        let ctx = TestCtx::new();
        let mut p = ctx.palette();
        ctx.sync(&mut p, &format!("{sigil}{PROBE_A}"));
        assert_eq!(tab(&mut p), format!("{sigil}{PROBE_A} "));
    }

    #[test]
    fn navigation_and_escape() {
        let ctx = TestCtx::new();
        let mut p = ctx.palette();
        assert!(matches!(
            p.handle_key(key(KeyCode::Down)),
            ToolAction::Consumed
        ));
        assert!(matches!(p.handle_key(key(KeyCode::Up)), ToolAction::Consumed));
        assert!(matches!(p.handle_key(key(KeyCode::Esc)), ToolAction::Consumed));
        assert!(!p.is_active());
    }

    #[test]
    fn inactive_palette_passes_everything() {
        let mut p = ToolPalette::new();
        for code in [KeyCode::Tab, KeyCode::Enter] {
            assert!(
                matches!(p.handle_key(key(code)), ToolAction::Passthrough),
                "{code:?} must pass through while the popup is down"
            );
        }
    }

    #[test]
    fn enter_in_name_phase_accepts_the_row() {
        let ctx = TestCtx::new();
        let mut p = ctx.palette();
        ctx.sync(&mut p, "$tp_al");
        assert!(matches!(
            p.handle_key(key(KeyCode::Enter)),
            ToolAction::Complete(text) if text == "$tp_alpha "
        ));
    }

    #[test]
    fn enter_on_argument_fragment_accepts_the_row() {
        let ctx = TestCtx::new();
        let mut p = ctx.palette();
        ctx.sync(&mut p, "$tp_alpha li");
        assert!(matches!(
            p.handle_key(key(KeyCode::Enter)),
            ToolAction::Complete(text) if text == "$tp_alpha limit="
        ));
    }

    #[test]
    fn enter_after_tool_space_submits_as_typed() {
        let ctx = TestCtx::new();
        let mut p = ctx.palette();
        ctx.sync(&mut p, "$tp_alpha ");
        assert!(matches!(
            p.handle_key(key(KeyCode::Enter)),
            ToolAction::Passthrough
        ));
    }

    #[test]
    fn name_phase_recovers_after_argument_visit() {
        let ctx = TestCtx::new();
        let mut p = ctx.palette();
        ctx.sync(&mut p, "$tp_alpha ");
        assert_eq!(names(&p), vec!["path", "limit", "mode"]);

        ctx.sync(&mut p, "$");
        let listed = names(&p);
        assert!(
            listed.contains(&PROBE_A.to_owned()) && listed.contains(&PROBE_B.to_owned()),
            "the tool list must come back, not the parameter list: {listed:?}"
        );
    }

    const PUSH_YAML: &str = "tags: [pub]\ndescription: pushes to github\n";
    const DEPLOY_YAML: &str =
        "tags: [pub]\ndescription: deploys\nbody: git push --force-with-lease\n";

    #[test]
    fn custom_tools_listed_with_detail() {
        let ctx = TestCtx::new();
        ctx.register_custom("scripts/push.sh", PUSH_YAML);
        let p = ctx.palette();
        let listed = names(&p);
        assert!(listed.contains(&"scripts/push.sh".to_owned()));
        let row = p
            .items
            .iter()
            .find(|i| i.name == "scripts/push.sh")
            .unwrap();
        assert_eq!(row.detail, "custom — pushes to github");
    }

    #[test]
    fn custom_file_tool_completes_to_bash_line() {
        let ctx = TestCtx::new();
        ctx.register_custom("scripts/push.sh", PUSH_YAML);
        let mut p = ctx.palette();
        ctx.sync(&mut p, "$push");
        assert_eq!(
            tab(&mut p),
            // cwd is the project root here, so the file resolves relative.
            "$bash scripts/push.sh "
        );
    }

    #[test]
    fn custom_command_tool_completes_to_its_body() {
        let ctx = TestCtx::new();
        ctx.register_custom("deploy", DEPLOY_YAML);
        let mut p = ctx.palette();
        ctx.sync(&mut p, "$deploy");
        assert_eq!(tab(&mut p), "$bash git push --force-with-lease ");
    }

    #[test]
    fn registry_name_wins_over_custom_registration() {
        let ctx = TestCtx::new();
        ctx.register_custom(PROBE_A, PUSH_YAML);
        let mut p = ctx.palette();
        ctx.sync(&mut p, &format!("${PROBE_A}"));
        // The registry tool completes as itself, not as a $bash line.
        assert_eq!(tab(&mut p), format!("${PROBE_A} "));
    }

    #[test]
    fn custom_dir_changes_rescan() {
        let ctx = TestCtx::new();
        let mut p = ctx.palette();
        assert!(!names(&p).contains(&"scripts/push.sh".to_owned()));

        ctx.register_custom("scripts/push.sh", PUSH_YAML);
        ctx.sync(&mut p, "$");
        assert!(names(&p).contains(&"scripts/push.sh".to_owned()));
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, crossterm::event::KeyModifiers::NONE)
    }
}
