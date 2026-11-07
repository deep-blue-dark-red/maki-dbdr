//! The model's own output kept as bytes it can point back into instead of
//! retyping. A fence labelled with a path is a draft: a `write` naming that
//! path and no content gets the draft as its content. Any tool argument may
//! quote the first and last line of something written earlier between ⟦ ⟧,
//! with … in between, and the lines they enclose are pasted in its place.
//!
//! Append-only: compaction and rewind change what the model is sent, never
//! what it wrote, so a quote keeps resolving for the whole session.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::{Arc, RwLock};

use maki_providers::{ContentBlock, EMPTY_RESPONSE_MARKER, Message, Role};
use regex::Regex;
use serde_json::Value;
use thiserror::Error;

pub const PASTE_OPEN: char = '⟦';
pub const PASTE_CLOSE: char = '⟧';
pub const DRAFT_PREFIX: &str = "draft:";
const ELLIPSIS: &str = "…";
const ELLIPSIS_LINE: &str = "...";
const SOURCE_SEP: char = '|';
const SOURCE_THINKING: &str = "thinking";
const SOURCE_TEXT: &str = "text";
const SOURCE_CODE: &str = "code";
const SOURCE_TABLE: &str = "table";
/// Assistant messages a quote may reach back through.
const SEARCH_DEPTH: usize = 8;
const MAX_LISTED: usize = 5;
const PREVIEW_CHARS: usize = 60;
const WRITE_TOOL: &str = "write";
const PATH_ARG: &str = "path";
const PATH_ALIAS: &str = "file_path";
const CONTENT_ARG: &str = "content";
const FENCE_MIN: usize = 3;
const FENCE_MAX_INDENT: usize = 3;
const NO_DRAFTS_HINT: &str = "fence the content as ```lang <path> first, or pass content";
/// `⟦@name⟧` pastes a clip saved by `copy`, or a draft by its path.
const NAME_SIGIL: char = '@';
const NO_NAMES_HINT: &str = "save one with transcript copy, or fence a draft with its path";

pub type SharedTranscript = Arc<RwLock<Transcript>>;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PasteError {
    #[error("paste: missing {PASTE_CLOSE} after {PASTE_OPEN}{0}")]
    Unclosed(String),
    #[error(
        "paste: separate the first and last line with {ELLIPSIS} or a line of {ELLIPSIS_LINE} in {PASTE_OPEN}{0}{PASTE_CLOSE}"
    )]
    MissingSeparator(String),
    #[error("paste: both sides of {ELLIPSIS} need text in {PASTE_OPEN}{0}{PASTE_CLOSE}")]
    EmptyAnchor(String),
    #[error("paste: nothing in {within} contains {start:?}")]
    NotFound { within: String, start: String },
    #[error(
        "paste: {start:?} occurs {count} times in the newest match; quote more lines to make it unique:\n{listed}"
    )]
    Ambiguous {
        start: String,
        count: usize,
        listed: String,
    },
    #[error("paste: found {start:?} but no {end:?} after it")]
    EndNotFound { start: String, end: String },
    #[error("no draft fenced as {path}; {hint}")]
    NoDraft { path: String, hint: String },
    #[error("draft {path} matches several: {matches}")]
    AmbiguousDraft { path: String, matches: String },
    #[error("paste: no clip or draft named {name}; {hint}")]
    NoName { name: String, hint: String },
}

/// One matching line of a transcript search.
pub struct Hit {
    /// Where the line is, like `m3 thinking code[src/h.rs]`.
    pub place: String,
    pub line_nr: usize,
    pub line: String,
}

pub struct DraftInfo {
    pub path: String,
    pub versions: usize,
    pub lines: usize,
}

pub struct ClipInfo {
    pub name: String,
    pub lines: usize,
}

/// A tool call whose input changed, or failed to, while its message was
/// appended. Dispatch runs the `Ok` input and answers the `Err` as the
/// call's result; history keeps the short form the model wrote.
pub struct ToolExpansion {
    pub id: String,
    pub input: Result<Value, PasteError>,
}

#[derive(Default)]
pub struct Transcript {
    text: String,
    /// Segment ranges of each assistant message, and of each draft edited
    /// through a tool, in order.
    messages: Vec<Range<usize>>,
    segments: Vec<Segment>,
    /// Fence path to its versions, oldest first.
    drafts: BTreeMap<Box<str>, Vec<usize>>,
    /// Clip name to the segment `copy` saved under it; a later copy replaces.
    clips: BTreeMap<Box<str>, usize>,
}

struct Segment {
    origin: Origin,
    shape: Shape,
    span: Range<usize>,
}

#[derive(Clone, PartialEq, Eq)]
enum Origin {
    Thinking,
    Text,
    Tool(Box<str>),
    Draft,
    Clip,
}

enum Shape {
    Prose,
    Table,
    Code { path: Option<Box<str>> },
}

/// Search order inside one message: a quote almost always means code, and
/// a brace in a fence must beat the same brace in the prose around it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Rank {
    Code,
    Table,
    Prose,
    Tool,
}

const RANKS: [Rank; 4] = [Rank::Code, Rank::Table, Rank::Prose, Rank::Tool];

impl Segment {
    fn rank(&self) -> Rank {
        match (&self.origin, &self.shape) {
            (Origin::Tool(_), _) => Rank::Tool,
            (_, Shape::Code { .. }) => Rank::Code,
            (_, Shape::Table) => Rank::Table,
            (_, Shape::Prose) => Rank::Prose,
        }
    }
}

enum Source<'a> {
    Any,
    Thinking,
    Text,
    Code,
    Table,
    Tool(&'a str),
}

impl Source<'_> {
    fn matches(&self, seg: &Segment) -> bool {
        match self {
            Self::Any => true,
            Self::Thinking => seg.origin == Origin::Thinking,
            Self::Text => seg.origin == Origin::Text,
            Self::Code => matches!(seg.shape, Shape::Code { .. }),
            Self::Table => matches!(seg.shape, Shape::Table),
            Self::Tool(name) => matches!(&seg.origin, Origin::Tool(n) if &**n == *name),
        }
    }

    fn describe(&self) -> String {
        match self {
            Self::Any => "your earlier output".into(),
            Self::Thinking => "your thinking".into(),
            Self::Text => "your reply text".into(),
            Self::Code => "your code blocks".into(),
            Self::Table => "your tables".into(),
            Self::Tool(name) => format!("your earlier {name} calls"),
        }
    }
}

struct Fence {
    ch: u8,
    len: usize,
    path: Option<Box<str>>,
}

impl Transcript {
    /// Indexes an assistant message and resolves every paste and draft in
    /// its tool calls against what precedes them, this message included.
    /// Other roles are not the model's writing and leave nothing behind.
    pub fn append(&mut self, msg: &Message) -> Vec<ToolExpansion> {
        if !matches!(msg.role, Role::Assistant) {
            return Vec::new();
        }
        let first = self.segments.len();
        let mut expansions = Vec::new();
        for block in &msg.content {
            match block {
                ContentBlock::Thinking { thinking, .. } => {
                    self.push_markdown(thinking, Origin::Thinking);
                }
                ContentBlock::Text { text } if text != EMPTY_RESPONSE_MARKER => {
                    self.push_markdown(text, Origin::Text);
                }
                ContentBlock::ToolUse {
                    id, name, input, ..
                } => {
                    let expanded = self.expand_input(name, input, first..self.segments.len());
                    let indexed = match &expanded {
                        Ok(Some(value)) => value,
                        _ => input,
                    };
                    self.push_tool_args(name, indexed);
                    let input = match expanded {
                        Ok(Some(value)) => Ok(value),
                        Ok(None) => continue,
                        Err(e) => Err(e),
                    };
                    expansions.push(ToolExpansion {
                        id: id.clone(),
                        input,
                    });
                }
                _ => {}
            }
        }
        self.messages.push(first..self.segments.len());
        expansions
    }

    /// Resolves ⟦…⟧ quotes in `s` against everything appended so far.
    pub fn expand<'a>(&self, s: &'a str) -> Result<Cow<'a, str>, PasteError> {
        let end = self.segments.len();
        self.expand_in(s, end..end)
    }

    /// The newest version of the draft fenced as `path`, with or without the
    /// `draft:` prefix. A bare file name finds `src/<name>` and the reverse.
    pub fn draft(&self, path: &str) -> Result<&str, PasteError> {
        let (_, versions) = self.draft_entry(path)?;
        let seg = &self.segments[*versions.last().expect("a draft has a version")];
        Ok(&self.text[seg.span.clone()])
    }

    /// A new version under the draft's own name, so a later lookup by either
    /// spelling sees the edit.
    pub fn set_draft(&mut self, path: &str, text: &str) {
        let query = path.strip_prefix(DRAFT_PREFIX).unwrap_or(path);
        let key: Box<str> = match self.draft_entry(query) {
            Ok((key, _)) => key.into(),
            Err(_) => query.into(),
        };
        let idx = self.segments.len();
        self.push_segment(
            text,
            Origin::Draft,
            Shape::Code {
                path: Some(key.clone()),
            },
        );
        self.drafts.entry(key).or_default().push(idx);
        self.messages.push(idx..idx + 1);
    }

    pub fn draft_paths(&self) -> impl Iterator<Item = &str> {
        self.drafts.keys().map(|k| &**k)
    }

    pub fn drafts(&self) -> Vec<DraftInfo> {
        self.drafts
            .iter()
            .map(|(path, versions)| DraftInfo {
                path: path.to_string(),
                versions: versions.len(),
                lines: self.line_count(*versions.last().expect("a draft has a version")),
            })
            .collect()
    }

    pub fn clips(&self) -> Vec<ClipInfo> {
        self.clips
            .iter()
            .map(|(name, &idx)| ClipInfo {
                name: name.to_string(),
                lines: self.line_count(idx),
            })
            .collect()
    }

    /// What a quote would paste, shown rather than used. With a name, the
    /// text is also kept as a clip so `⟦@name⟧` pastes it from then on.
    pub fn copy(&mut self, quote: &str, name: Option<&str>) -> Result<String, PasteError> {
        let text = self.expand(quote)?.into_owned();
        if let Some(name) = name {
            let idx = self.segments.len();
            self.push_segment(&text, Origin::Clip, Shape::Prose);
            self.clips.insert(name.into(), idx);
            self.messages.push(idx..idx + 1);
        }
        Ok(text)
    }

    /// Lines matching `pattern`, newest first, each with where it was
    /// written, so the model can quote it or copy it by name.
    pub fn search(&self, pattern: &Regex, limit: usize) -> Vec<Hit> {
        let mut hits = Vec::new();
        for (msg_nr, range) in self.messages.iter().enumerate().rev() {
            for idx in range.clone().rev() {
                let text = &self.text[self.segments[idx].span.clone()];
                for (line_idx, line) in text.lines().enumerate() {
                    if !pattern.is_match(line) {
                        continue;
                    }
                    hits.push(Hit {
                        place: self.place(msg_nr, idx),
                        line_nr: line_idx + 1,
                        line: line.to_owned(),
                    });
                    if hits.len() == limit {
                        return hits;
                    }
                }
            }
        }
        hits
    }

    fn place(&self, msg_nr: usize, idx: usize) -> String {
        let seg = &self.segments[idx];
        let origin = match &seg.origin {
            Origin::Thinking => SOURCE_THINKING.into(),
            Origin::Text => SOURCE_TEXT.into(),
            Origin::Tool(name) => format!("{name}()"),
            Origin::Draft => "draft".into(),
            Origin::Clip => {
                let name = self.clips.iter().find(|(_, i)| **i == idx);
                name.map_or_else(|| "clip".into(), |(name, _)| format!("clip[{name}]"))
            }
        };
        let shape = match &seg.shape {
            Shape::Prose => String::new(),
            Shape::Table => format!(" {SOURCE_TABLE}"),
            Shape::Code { path: Some(path) } => format!(" {SOURCE_CODE}[{path}]"),
            Shape::Code { path: None } => format!(" {SOURCE_CODE}"),
        };
        format!("m{} {origin}{shape}", msg_nr + 1)
    }

    fn line_count(&self, idx: usize) -> usize {
        self.text[self.segments[idx].span.clone()].lines().count()
    }

    fn named(&self, name: &str) -> Result<&str, PasteError> {
        if let Some(&idx) = self.clips.get(name) {
            return Ok(&self.text[self.segments[idx].span.clone()]);
        }
        self.draft(name).map_err(|e| match e {
            PasteError::NoDraft { .. } => PasteError::NoName {
                name: name.into(),
                hint: self.name_hint(),
            },
            other => other,
        })
    }

    fn name_hint(&self) -> String {
        let names: Vec<&str> = self
            .clips
            .keys()
            .map(|k| &**k)
            .chain(self.draft_paths())
            .collect();
        if names.is_empty() {
            return NO_NAMES_HINT.into();
        }
        format!("names: {}", names.join(", "))
    }

    fn draft_entry(&self, path: &str) -> Result<(&str, &Vec<usize>), PasteError> {
        let query = path.strip_prefix(DRAFT_PREFIX).unwrap_or(path);
        if let Some((key, versions)) = self.drafts.get_key_value(query) {
            return Ok((key, versions));
        }
        let mut hits = self
            .drafts
            .iter()
            .filter(|(key, _)| suffix_match(key, query));
        let Some((key, versions)) = hits.next() else {
            return Err(PasteError::NoDraft {
                path: query.into(),
                hint: self.draft_hint(),
            });
        };
        if hits.next().is_some() {
            let matches = self
                .drafts
                .keys()
                .filter(|key| suffix_match(key, query))
                .map(|key| &**key)
                .collect::<Vec<_>>()
                .join(", ");
            return Err(PasteError::AmbiguousDraft {
                path: query.into(),
                matches,
            });
        }
        Ok((key, versions))
    }

    fn draft_hint(&self) -> String {
        if self.drafts.is_empty() {
            return NO_DRAFTS_HINT.into();
        }
        format!(
            "drafts: {}",
            self.draft_paths().collect::<Vec<_>>().join(", ")
        )
    }

    fn push_markdown(&mut self, text: &str, origin: Origin) {
        for (shape, range) in segment_markdown(text) {
            let idx = self.segments.len();
            if let Shape::Code { path: Some(path) } = &shape {
                self.drafts.entry(path.clone()).or_default().push(idx);
            }
            self.push_segment(&text[range], origin.clone(), shape);
        }
    }

    fn push_tool_args(&mut self, tool: &str, input: &Value) {
        for_each_string(input, &mut |s| {
            self.push_segment(s, Origin::Tool(tool.into()), Shape::Prose);
        });
    }

    fn push_segment(&mut self, text: &str, origin: Origin, shape: Shape) {
        let start = self.text.len();
        self.text.push_str(text);
        self.segments.push(Segment {
            origin,
            shape,
            span: start..self.text.len(),
        });
    }

    /// `Ok(None)` when the call needs nothing from the transcript.
    fn expand_input(
        &self,
        tool: &str,
        input: &Value,
        current: Range<usize>,
    ) -> Result<Option<Value>, PasteError> {
        let mut out = None;
        if contains_marker(input) {
            let mut value = input.clone();
            map_strings(&mut value, &mut |s| {
                if let Cow::Owned(expanded) = self.expand_in(s, current.clone())? {
                    *s = expanded;
                }
                Ok(())
            })?;
            out = Some(value);
        }
        if tool == WRITE_TOOL {
            let call = out.as_ref().unwrap_or(input);
            let path = call
                .get(PATH_ARG)
                .or_else(|| call.get(PATH_ALIAS))
                .and_then(Value::as_str);
            if call.get(CONTENT_ARG).is_none_or(Value::is_null)
                && let Some(path) = path
            {
                let content = self.draft(path)?.to_owned();
                let mut value = out.take().unwrap_or_else(|| input.clone());
                value[CONTENT_ARG] = Value::String(content);
                out = Some(value);
            }
        }
        Ok(out)
    }

    fn expand_in<'a>(&self, s: &'a str, current: Range<usize>) -> Result<Cow<'a, str>, PasteError> {
        let Some(first) = s.find(PASTE_OPEN) else {
            return Ok(Cow::Borrowed(s));
        };
        let mut out = String::with_capacity(s.len());
        out.push_str(&s[..first]);
        let mut rest = &s[first..];
        loop {
            let inner_start = PASTE_OPEN.len_utf8();
            let Some(close) = rest[inner_start..].find(PASTE_CLOSE) else {
                return Err(PasteError::Unclosed(preview(&rest[inner_start..])));
            };
            let inner = &rest[inner_start..inner_start + close];
            out.push_str(self.resolve(inner, current.clone())?);
            rest = &rest[inner_start + close + PASTE_CLOSE.len_utf8()..];
            match rest.find(PASTE_OPEN) {
                Some(next) => {
                    out.push_str(&rest[..next]);
                    rest = &rest[next..];
                }
                None => {
                    out.push_str(rest);
                    return Ok(Cow::Owned(out));
                }
            }
        }
    }

    /// The whole lines from the newest unique occurrence of the start quote
    /// to the first occurrence of the end quote after it.
    fn resolve(&self, inner: &str, current: Range<usize>) -> Result<&str, PasteError> {
        if let Some(name) = inner.strip_prefix(NAME_SIGIL)
            && !name.contains(ELLIPSIS)
            && !name.contains('\n')
        {
            return self.named(name.trim());
        }
        let (source, body) = split_source(inner);
        let (start, end) = split_anchors(body)?;
        for idx in self.search_order(current) {
            let seg = &self.segments[idx];
            if !source.matches(seg) {
                continue;
            }
            let hay = &self.text[seg.span.clone()];
            let Some(at) = hay.find(start) else {
                continue;
            };
            let count = hay.matches(start).count();
            if count > 1 {
                return Err(PasteError::Ambiguous {
                    start: start.into(),
                    count,
                    listed: list_matches(hay, start),
                });
            }
            let after = at + start.len();
            let Some(end_at) = hay[after..].find(end) else {
                return Err(PasteError::EndNotFound {
                    start: start.into(),
                    end: end.into(),
                });
            };
            let end_abs = after + end_at + end.len();
            let line_start = hay[..at].rfind('\n').map_or(0, |i| i + 1);
            let line_end = hay[end_abs..].find('\n').map_or(hay.len(), |i| end_abs + i);
            return Ok(&hay[line_start..line_end]);
        }
        Err(PasteError::NotFound {
            within: source.describe(),
            start: start.into(),
        })
    }

    /// The message being appended first, then earlier ones newest first;
    /// within each, code before tables before prose before tool arguments,
    /// and later segments before earlier ones.
    fn search_order(&self, current: Range<usize>) -> impl Iterator<Item = usize> + '_ {
        let earlier = self.messages.iter().rev().take(SEARCH_DEPTH).cloned();
        std::iter::once(current)
            .chain(earlier)
            .flat_map(move |range| {
                RANKS.into_iter().flat_map(move |rank| {
                    range
                        .clone()
                        .rev()
                        .filter(move |&i| self.segments[i].rank() == rank)
                })
            })
    }
}

fn suffix_match(key: &str, query: &str) -> bool {
    let under = |whole: &str, tail: &str| {
        whole.len() > tail.len()
            && whole.ends_with(tail)
            && whole[..whole.len() - tail.len()].ends_with('/')
    };
    under(key, query) || under(query, key)
}

/// A bare identifier before the first `|` names a source. A quote that
/// starts with `|` itself (a closure, a table row) has no such prefix.
fn split_source(inner: &str) -> (Source<'_>, &str) {
    let Some(sep) = inner.find(SOURCE_SEP) else {
        return (Source::Any, inner);
    };
    let prefix = &inner[..sep];
    let is_ident = !prefix.is_empty()
        && prefix
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if !is_ident {
        return (Source::Any, inner);
    }
    let source = match prefix {
        SOURCE_THINKING => Source::Thinking,
        SOURCE_TEXT => Source::Text,
        SOURCE_CODE => Source::Code,
        SOURCE_TABLE => Source::Table,
        tool => Source::Tool(tool),
    };
    (source, &inner[sep + SOURCE_SEP.len_utf8()..])
}

fn split_anchors(body: &str) -> Result<(&str, &str), PasteError> {
    let (head, tail) = match body.find(ELLIPSIS) {
        Some(i) => (&body[..i], &body[i + ELLIPSIS.len()..]),
        None => split_at_ellipsis_line(body)
            .ok_or_else(|| PasteError::MissingSeparator(preview(body)))?,
    };
    let newline = |c: char| c == '\n' || c == '\r';
    let start = head.trim_matches(newline);
    let end = tail.trim_matches(newline);
    if start.is_empty() || end.is_empty() {
        return Err(PasteError::EmptyAnchor(preview(body)));
    }
    Ok((start, end))
}

fn split_at_ellipsis_line(body: &str) -> Option<(&str, &str)> {
    let mut offset = 0;
    for line in body.split_inclusive('\n') {
        if line.trim() == ELLIPSIS_LINE {
            return Some((&body[..offset], &body[offset + line.len()..]));
        }
        offset += line.len();
    }
    None
}

fn list_matches(hay: &str, needle: &str) -> String {
    hay.match_indices(needle)
        .take(MAX_LISTED)
        .map(|(at, _)| {
            let line_nr = hay[..at].matches('\n').count() + 1;
            let line_start = hay[..at].rfind('\n').map_or(0, |i| i + 1);
            let line = hay[line_start..].lines().next().unwrap_or_default();
            format!("  {line_nr}: {}", preview(line.trim_end()))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn preview(s: &str) -> String {
    let line = s.lines().next().unwrap_or_default();
    let mut out: String = line.chars().take(PREVIEW_CHARS).collect();
    if out.len() < line.len() || s.len() > line.len() {
        out.push_str(ELLIPSIS);
    }
    out
}

fn contains_marker(value: &Value) -> bool {
    let mut found = false;
    for_each_string(value, &mut |s| found |= s.contains(PASTE_OPEN));
    found
}

fn for_each_string<'a>(value: &'a Value, f: &mut impl FnMut(&'a str)) {
    match value {
        Value::String(s) => f(s),
        Value::Array(items) => items.iter().for_each(|item| for_each_string(item, f)),
        Value::Object(map) => map.values().for_each(|item| for_each_string(item, f)),
        _ => {}
    }
}

fn map_strings(
    value: &mut Value,
    f: &mut impl FnMut(&mut String) -> Result<(), PasteError>,
) -> Result<(), PasteError> {
    match value {
        Value::String(s) => f(s),
        Value::Array(items) => items.iter_mut().try_for_each(|item| map_strings(item, f)),
        Value::Object(map) => map.values_mut().try_for_each(|item| map_strings(item, f)),
        _ => Ok(()),
    }
}

/// Fenced code (body only, fence lines excluded), runs of table rows, and
/// the prose between them, as byte ranges of `text`. An unclosed fence runs
/// to the end, which is what a cut-off reply leaves behind.
fn segment_markdown(text: &str) -> Vec<(Shape, Range<usize>)> {
    let mut lines = Vec::new();
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        lines.push((offset, line));
        offset += line.len();
    }

    let mut out = Vec::new();
    let mut prose_start = 0;
    let mut i = 0;
    while i < lines.len() {
        let (start, line) = lines[i];
        if let Some(fence) = fence_open(line) {
            push_prose(&mut out, text, prose_start..start);
            let body_start = start + line.len();
            let closing = (i + 1..lines.len()).find(|&j| fence_close(lines[j].1, &fence));
            let body_end = closing.map_or(text.len(), |j| lines[j].0);
            out.push((Shape::Code { path: fence.path }, body_start..body_end));
            i = closing.map_or(lines.len(), |j| j + 1);
            prose_start = lines.get(i).map_or(text.len(), |l| l.0);
        } else if is_table_row(line) && lines.get(i + 1).is_some_and(|l| is_table_row(l.1)) {
            push_prose(&mut out, text, prose_start..start);
            let past = (i + 1..lines.len())
                .find(|&j| !is_table_row(lines[j].1))
                .unwrap_or(lines.len());
            let end = lines[past - 1].0 + lines[past - 1].1.len();
            out.push((Shape::Table, start..end));
            prose_start = end;
            i = past;
        } else {
            i += 1;
        }
    }
    push_prose(&mut out, text, prose_start..text.len());
    out
}

fn push_prose(out: &mut Vec<(Shape, Range<usize>)>, text: &str, range: Range<usize>) {
    if !text[range.clone()].trim().is_empty() {
        out.push((Shape::Prose, range));
    }
}

fn fence_open(line: &str) -> Option<Fence> {
    let trimmed = line.trim_start_matches(' ');
    if line.len() - trimmed.len() > FENCE_MAX_INDENT {
        return None;
    }
    let ch = *trimmed.as_bytes().first()?;
    if ch != b'`' && ch != b'~' {
        return None;
    }
    let len = trimmed.bytes().take_while(|&b| b == ch).count();
    if len < FENCE_MIN {
        return None;
    }
    let info = trimmed[len..].trim();
    if ch == b'`' && info.contains('`') {
        return None;
    }
    Some(Fence {
        ch,
        len,
        path: fence_path(info),
    })
}

fn fence_close(line: &str, fence: &Fence) -> bool {
    let trimmed = line.trim_start_matches(' ');
    if line.len() - trimmed.len() > FENCE_MAX_INDENT {
        return false;
    }
    let len = trimmed.bytes().take_while(|&b| b == fence.ch).count();
    len >= fence.len && trimmed[len..].trim().is_empty()
}

/// The first path-looking token of a fence info string, with any `key=`
/// prefix and quotes stripped: ```` ```rust src/h.rs ````, ```` ```src/h.rs ````
/// and ```` ```rust title="src/h.rs" ```` all name `src/h.rs`.
fn fence_path(info: &str) -> Option<Box<str>> {
    info.split_whitespace()
        .map(|token| {
            token
                .rsplit_once('=')
                .map_or(token, |(_, value)| value)
                .trim_matches(['"', '\''])
        })
        .find(|token| {
            token.contains('/') || (token.contains('.') && token.len() > 1 && !token.ends_with('.'))
        })
        .map(Box::from)
}

fn is_table_row(line: &str) -> bool {
    line.trim().starts_with('|')
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use test_case::test_case;

    use super::*;

    const DRAFT: &str = "fn handle(req: Request) -> Response {\n    Response::ok()\n}\n";
    const DRAFT_PATH: &str = "src/h.rs";

    fn assistant(blocks: Vec<ContentBlock>) -> Message {
        Message {
            role: Role::Assistant,
            content: blocks,
            ..Default::default()
        }
    }

    fn thinking(text: &str) -> ContentBlock {
        ContentBlock::Thinking {
            thinking: text.into(),
            signature: None,
        }
    }

    fn text(text: &str) -> ContentBlock {
        ContentBlock::Text { text: text.into() }
    }

    fn call(id: &str, name: &str, input: Value) -> ContentBlock {
        ContentBlock::tool_use(id, name, input)
    }

    fn fenced(path: &str, body: &str) -> String {
        format!("Plan:\n```rust {path}\n{body}```\nDone.\n")
    }

    fn with_draft() -> Transcript {
        let mut t = Transcript::default();
        t.append(&assistant(vec![thinking(&fenced(DRAFT_PATH, DRAFT))]));
        t
    }

    fn expanded(t: &Transcript, s: &str) -> String {
        t.expand(s).unwrap().into_owned()
    }

    #[test_case("rust src/h.rs", Some("src/h.rs"); "lang then path")]
    #[test_case("src/h.rs", Some("src/h.rs"); "path alone")]
    #[test_case("rust title=\"src/h.rs\"", Some("src/h.rs"); "quoted key value")]
    #[test_case("toml Cargo.toml", Some("Cargo.toml"); "dotted name")]
    #[test_case("rust", None; "lang alone")]
    #[test_case("", None; "empty")]
    fn fence_path_detection(info: &str, expected: Option<&str>) {
        assert_eq!(fence_path(info).as_deref(), expected);
    }

    #[test_case("```rust\nx\n```\n", 1, 0; "backtick fence")]
    #[test_case("~~~\nx\n~~~\n", 1, 0; "tilde fence")]
    #[test_case("   ```\nx\n```\n", 1, 0; "indented fence")]
    #[test_case("```rust\nx\n", 1, 0; "unclosed fence runs to end")]
    #[test_case("```a`b\nx\n", 0, 1; "backtick in info is not a fence")]
    #[test_case("| a | b |\n|---|---|\n| 1 | 2 |\n", 0, 0; "table alone")]
    #[test_case("intro\n```\nx\n```\nouter\n", 1, 2; "prose around a fence")]
    fn segmenting(text: &str, code: usize, prose: usize) {
        let shapes = segment_markdown(text);
        let count = |pred: fn(&Shape) -> bool| shapes.iter().filter(|(s, _)| pred(s)).count();
        assert_eq!(count(|s| matches!(s, Shape::Code { .. })), code);
        assert_eq!(count(|s| matches!(s, Shape::Prose)), prose);
    }

    #[test]
    fn fence_body_excludes_fence_lines() {
        let text = "```rust\nfn a() {}\n```\n";
        let (shape, range) = segment_markdown(text).into_iter().next().unwrap();
        assert!(matches!(shape, Shape::Code { .. }));
        assert_eq!(&text[range], "fn a() {}\n");
    }

    #[test]
    fn table_rows_form_one_segment() {
        let text = "see:\n| a |\n| b |\nend\n";
        let table = segment_markdown(text)
            .into_iter()
            .find(|(s, _)| matches!(s, Shape::Table))
            .unwrap();
        assert_eq!(&text[table.1], "| a |\n| b |\n");
    }

    #[test_case("src/h.rs"; "exact")]
    #[test_case("draft:src/h.rs"; "prefixed")]
    #[test_case("h.rs"; "bare name")]
    #[test_case("/abs/proj/src/h.rs"; "absolute over relative")]
    fn draft_lookup(path: &str) {
        assert_eq!(with_draft().draft(path).unwrap(), DRAFT);
    }

    #[test]
    fn draft_missing_lists_known() {
        let err = with_draft().draft("src/other.rs").unwrap_err();
        assert_eq!(
            err,
            PasteError::NoDraft {
                path: "src/other.rs".into(),
                hint: "drafts: src/h.rs".into(),
            }
        );
    }

    #[test]
    fn draft_missing_without_any_explains_fences() {
        let err = Transcript::default().draft("x.rs").unwrap_err();
        assert!(matches!(err, PasteError::NoDraft { hint, .. } if hint == NO_DRAFTS_HINT));
    }

    #[test]
    fn draft_suffix_ambiguity_is_an_error() {
        let mut t = Transcript::default();
        t.append(&assistant(vec![thinking(&format!(
            "{}{}",
            fenced("a/h.rs", "a\n"),
            fenced("b/h.rs", "b\n")
        ))]));
        assert_eq!(
            t.draft("h.rs").unwrap_err(),
            PasteError::AmbiguousDraft {
                path: "h.rs".into(),
                matches: "a/h.rs, b/h.rs".into(),
            }
        );
    }

    #[test]
    fn latest_draft_version_wins() {
        let mut t = with_draft();
        t.append(&assistant(vec![text(&fenced(DRAFT_PATH, "v2\n"))]));
        assert_eq!(t.draft(DRAFT_PATH).unwrap(), "v2\n");
    }

    #[test]
    fn set_draft_by_short_name_updates_the_long_one() {
        let mut t = with_draft();
        t.set_draft("draft:h.rs", "edited\n");
        assert_eq!(t.draft(DRAFT_PATH).unwrap(), "edited\n");
        assert_eq!(t.draft_paths().collect::<Vec<_>>(), vec![DRAFT_PATH]);
    }

    #[test]
    fn write_without_content_gets_the_draft() {
        let mut t = Transcript::default();
        let msg = assistant(vec![
            thinking(&fenced(DRAFT_PATH, DRAFT)),
            call("t1", "write", json!({"path": DRAFT_PATH})),
        ]);
        let expansions = t.append(&msg);
        assert_eq!(expansions.len(), 1);
        assert_eq!(expansions[0].id, "t1");
        assert_eq!(
            expansions[0].input.as_ref().unwrap(),
            &json!({"path": DRAFT_PATH, "content": DRAFT})
        );
    }

    #[test]
    fn write_with_content_is_untouched() {
        let mut t = with_draft();
        let msg = assistant(vec![call(
            "t1",
            "write",
            json!({"path": DRAFT_PATH, "content": "mine"}),
        )]);
        assert!(t.append(&msg).is_empty());
    }

    #[test]
    fn write_without_draft_surfaces_the_error() {
        let mut t = Transcript::default();
        let msg = assistant(vec![call("t1", "write", json!({"file_path": "x.rs"}))]);
        let expansions = t.append(&msg);
        assert!(matches!(
            expansions[0].input.as_ref().unwrap_err(),
            PasteError::NoDraft { path, .. } if path == "x.rs"
        ));
    }

    #[test]
    fn user_messages_leave_nothing() {
        let mut t = Transcript::default();
        let msg = Message {
            role: Role::User,
            content: vec![text(&fenced(DRAFT_PATH, DRAFT))],
            ..Default::default()
        };
        assert!(t.append(&msg).is_empty());
        assert!(t.draft(DRAFT_PATH).is_err());
    }

    #[test]
    fn paste_whole_lines_between_quotes() {
        let t = with_draft();
        let out = expanded(&t, "⟦handle(req…ok()⟧");
        assert_eq!(
            out,
            "fn handle(req: Request) -> Response {\n    Response::ok()"
        );
    }

    #[test]
    fn paste_with_ellipsis_line_and_newlines() {
        let t = with_draft();
        let out = expanded(&t, "⟦\nfn handle(\n...\n}\n⟧");
        assert_eq!(out, DRAFT.trim_end());
    }

    #[test]
    fn paste_splices_literal_text() {
        let t = with_draft();
        let out = expanded(
            &t,
            "// top\n⟦fn handle…Response {⟧\n    log();\n⟦Response::ok…}⟧\n",
        );
        assert_eq!(
            out,
            "// top\nfn handle(req: Request) -> Response {\n    log();\n    Response::ok()\n}\n"
        );
    }

    #[test]
    fn paste_without_marker_borrows() {
        let t = with_draft();
        assert!(matches!(t.expand("plain").unwrap(), Cow::Borrowed(_)));
    }

    #[test_case("⟦fn handle…}", PasteError::Unclosed("fn handle…}".into()); "unclosed")]
    #[test_case("⟦fn handle }⟧", PasteError::MissingSeparator("fn handle }".into()); "no separator")]
    #[test_case("⟦…}⟧", PasteError::EmptyAnchor("…}".into()); "empty start")]
    #[test_case("⟦fn nope(…}⟧", PasteError::NotFound { within: "your earlier output".into(), start: "fn nope(".into() }; "start missing")]
    #[test_case("⟦fn handle(…zzz⟧", PasteError::EndNotFound { start: "fn handle(".into(), end: "zzz".into() }; "end missing")]
    #[test_case("⟦text|fn handle(…}⟧", PasteError::NotFound { within: "your reply text".into(), start: "fn handle(".into() }; "source excludes thinking")]
    fn paste_errors(input: &str, expected: PasteError) {
        assert_eq!(with_draft().expand(input).unwrap_err(), expected);
    }

    #[test]
    fn paste_ambiguous_start_lists_lines() {
        let mut t = Transcript::default();
        t.append(&assistant(vec![thinking(
            "```\nlet a = 1;\nlet b = 2;\nlet c = 3;\n```\n",
        )]));
        let err = t.expand("⟦let…3;⟧").unwrap_err();
        assert_eq!(
            err,
            PasteError::Ambiguous {
                start: "let".into(),
                count: 3,
                listed: "  1: let a = 1;\n  2: let b = 2;\n  3: let c = 3;".into(),
            }
        );
    }

    #[test]
    fn paste_prefers_code_over_prose_in_the_same_message() {
        let mut t = Transcript::default();
        t.append(&assistant(vec![thinking(
            "fn a is below\n```\nfn a() {\n}\n```\n",
        )]));
        assert_eq!(expanded(&t, "⟦fn a…}⟧"), "fn a() {\n}");
    }

    #[test]
    fn paste_reaches_earlier_messages_newest_first() {
        let mut t = Transcript::default();
        t.append(&assistant(vec![text("```\nfn a() {\n    old\n}\n```\n")]));
        t.append(&assistant(vec![text("```\nfn a() {\n    new\n}\n```\n")]));
        t.append(&assistant(vec![text("unrelated")]));
        assert_eq!(expanded(&t, "⟦fn a…}⟧"), "fn a() {\n    new\n}");
    }

    #[test]
    fn paste_from_an_earlier_tool_argument() {
        let mut t = Transcript::default();
        t.append(&assistant(vec![call(
            "t1",
            "edit",
            json!({"path": "x", "old_string": "a", "new_string": "line one\nline two\n"}),
        )]));
        assert_eq!(expanded(&t, "⟦edit|line one…two⟧"), "line one\nline two");
    }

    #[test]
    fn paste_in_the_same_message_as_the_draft() {
        let mut t = Transcript::default();
        let msg = assistant(vec![
            thinking(&fenced(DRAFT_PATH, DRAFT)),
            call(
                "t1",
                "edit",
                json!({"path": "x", "old_string": "a", "new_string": "⟦fn handle…}⟧"}),
            ),
        ]);
        let expansions = t.append(&msg);
        assert_eq!(
            expansions[0].input.as_ref().unwrap()["new_string"],
            Value::String(DRAFT.trim_end().into())
        );
    }

    #[test]
    fn nested_strings_are_expanded_and_indexed_expanded() {
        let mut t = with_draft();
        let batch = assistant(vec![call(
            "b1",
            "batch",
            json!({"calls": [{"tool": "write", "input": {"path": "y", "content": "⟦fn handle…}⟧"}}]}),
        )]);
        let expansions = t.append(&batch);
        let content = &expansions[0].input.as_ref().unwrap()["calls"][0]["input"]["content"];
        assert_eq!(content, &Value::String(DRAFT.trim_end().into()));
        assert_eq!(expanded(&t, "⟦batch|fn handle…}⟧"), DRAFT.trim_end());
    }

    #[test]
    fn copy_shows_the_paste_and_saves_a_clip() {
        let mut t = with_draft();
        let shown = t.copy("⟦fn handle…}⟧", Some("handler")).unwrap();
        assert_eq!(shown, DRAFT.trim_end());
        assert_eq!(expanded(&t, "⟦@handler⟧"), DRAFT.trim_end());
        assert_eq!(t.clips().len(), 1);
        assert_eq!(t.clips()[0].lines, 3);
    }

    #[test]
    fn copy_without_name_keeps_no_clip() {
        let mut t = with_draft();
        t.copy("⟦fn handle…}⟧", None).unwrap();
        assert!(t.clips().is_empty());
    }

    #[test]
    fn named_paste_falls_back_to_a_draft() {
        let t = with_draft();
        assert_eq!(expanded(&t, "⟦@src/h.rs⟧"), DRAFT);
    }

    #[test]
    fn named_paste_unknown_lists_names() {
        let mut t = with_draft();
        t.copy("⟦fn handle…}⟧", Some("h")).unwrap();
        assert_eq!(
            t.expand("⟦@nope⟧").unwrap_err(),
            PasteError::NoName {
                name: "nope".into(),
                hint: "names: h, src/h.rs".into(),
            }
        );
    }

    #[test]
    fn search_reports_place_and_line_newest_first() {
        let mut t = Transcript::default();
        t.append(&assistant(vec![thinking(
            "```rust a.rs\nfn old() {}\n```\n",
        )]));
        t.append(&assistant(vec![
            text("fn in prose\n"),
            call(
                "t1",
                "edit",
                json!({"path": "x", "old_string": "fn arg", "new_string": "y"}),
            ),
        ]));
        let hits = t.search(&Regex::new("^fn ").unwrap(), 10);
        let places: Vec<(&str, usize, &str)> = hits
            .iter()
            .map(|h| (h.place.as_str(), h.line_nr, h.line.as_str()))
            .collect();
        assert_eq!(
            places,
            vec![
                ("m2 edit()", 1, "fn arg"),
                ("m2 text", 1, "fn in prose"),
                ("m1 thinking code[a.rs]", 1, "fn old() {}"),
            ]
        );
    }

    #[test]
    fn search_stops_at_the_limit() {
        let mut t = Transcript::default();
        t.append(&assistant(vec![text("a\na\na\n")]));
        assert_eq!(t.search(&Regex::new("a").unwrap(), 2).len(), 2);
    }

    #[test]
    fn drafts_report_versions_and_lines() {
        let mut t = with_draft();
        t.set_draft(DRAFT_PATH, "one\ntwo\n");
        let drafts = t.drafts();
        assert_eq!(drafts.len(), 1);
        assert_eq!(drafts[0].path, DRAFT_PATH);
        assert_eq!(drafts[0].versions, 2);
        assert_eq!(drafts[0].lines, 2);
    }
}
