use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};

use serde::Deserialize;
use serde_json::Value;
use thiserror::Error;

const HEADER_PREFIX: &str = r#"{"t":"header""#;
const MSG_PREFIX: &str = r#"{"t":"msg""#;
const SUB_MSG_PREFIX: &str = r#"{"t":"sub_msg""#;
const META_PREFIX: &str = r#"{"t":"meta""#;
const OUT_PREFIX: &str = r#"{"t":"out","id":""#;
const OUT_DATA_MARKER: &str = r#","d":{""#;
const DIFF_VARIANT: &str = "Diff";

#[derive(Debug, Error)]
pub enum LogError {
    #[error("no header record: not a maki session log")]
    MissingHeader,
    #[error("unreadable header record: {0}")]
    BadHeader(#[source] serde_json::Error),
}

#[derive(Debug, Deserialize)]
pub struct Header<'a> {
    pub v: u32,
    #[serde(borrow)]
    pub id: Cow<'a, str>,
    #[serde(borrow)]
    pub model: Cow<'a, str>,
    #[serde(borrow)]
    pub cwd: Cow<'a, str>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Block<'a> {
    Text {
        #[serde(borrow)]
        text: Cow<'a, str>,
    },
    Thinking {
        #[serde(borrow)]
        thinking: Cow<'a, str>,
    },
    ToolUse {
        #[serde(borrow)]
        id: Cow<'a, str>,
        #[serde(borrow)]
        name: Cow<'a, str>,
        input: Value,
    },
    ToolResult {
        #[serde(borrow)]
        tool_use_id: Cow<'a, str>,
        #[serde(borrow)]
        content: Cow<'a, str>,
        #[serde(default)]
        is_error: bool,
    },
    Image {},
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
pub struct Turn<'a> {
    pub role: Role,
    #[serde(borrow)]
    pub content: Vec<Block<'a>>,
}

#[derive(Debug, Deserialize)]
pub struct Diff {
    pub path: String,
    pub before: String,
    pub after: String,
}

#[derive(Debug, Default, Clone, Copy, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub input: u64,
    #[serde(default)]
    pub output: u64,
    #[serde(default)]
    pub cache_creation: u64,
    #[serde(default)]
    pub cache_read: u64,
    #[serde(default)]
    pub cost: Option<f64>,
}

#[derive(Debug, Deserialize)]
pub struct SubagentInfo<'a> {
    #[serde(borrow)]
    pub tool_use_id: Cow<'a, str>,
    #[serde(borrow)]
    pub name: Cow<'a, str>,
    #[serde(default)]
    pub model: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Meta<'a> {
    #[serde(borrow)]
    pub title: Cow<'a, str>,
    pub updated_at: i64,
    #[serde(default)]
    pub context_size: u32,
    #[serde(default, borrow)]
    pub subagents: Vec<SubagentInfo<'a>>,
    #[serde(default)]
    pub usage_by_model: BTreeMap<String, Usage>,
}

#[derive(Deserialize)]
struct MsgRecord<'a> {
    #[serde(borrow)]
    d: Turn<'a>,
}

#[derive(Deserialize)]
struct SubMsgRecord<'a> {
    #[serde(borrow)]
    sub: Cow<'a, str>,
    #[serde(borrow)]
    d: Turn<'a>,
}

#[derive(Deserialize)]
struct DiffRecord<'a> {
    #[serde(borrow)]
    id: Cow<'a, str>,
    d: DiffPayload,
}

#[derive(Deserialize)]
struct DiffPayload {
    #[serde(rename = "Diff")]
    diff: Diff,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ParseOpts {
    pub diffs: bool,
}

pub struct Session<'a> {
    pub header: Header<'a>,
    pub meta: Option<Meta<'a>>,
    pub turns: Vec<Turn<'a>>,
    pub subagent_turns: HashMap<Cow<'a, str>, Vec<Turn<'a>>>,
    pub subagent_order: Vec<Cow<'a, str>>,
    pub diffs: HashMap<Cow<'a, str>, Diff>,
    pub skipped_lines: usize,
}

impl<'a> Session<'a> {
    pub fn parse(text: &'a str, opts: ParseOpts) -> Result<Self, LogError> {
        let mut header = None;
        let mut last_meta = None;
        let mut turns = Vec::new();
        let mut subagent_turns: HashMap<Cow<str>, Vec<Turn>> = HashMap::new();
        let mut subagent_order = Vec::new();
        let mut diffs = HashMap::new();
        let mut skipped_lines = 0;

        for line in text.lines() {
            if line.starts_with(MSG_PREFIX) {
                match serde_json::from_str::<MsgRecord>(line) {
                    Ok(record) => turns.push(record.d),
                    Err(_) => skipped_lines += 1,
                }
            } else if line.starts_with(SUB_MSG_PREFIX) {
                match serde_json::from_str::<SubMsgRecord>(line) {
                    Ok(record) => {
                        if !subagent_turns.contains_key(&record.sub) {
                            subagent_order.push(record.sub.clone());
                        }
                        subagent_turns.entry(record.sub).or_default().push(record.d);
                    }
                    Err(_) => skipped_lines += 1,
                }
            } else if line.starts_with(META_PREFIX) {
                last_meta = Some(line);
            } else if line.starts_with(HEADER_PREFIX) {
                if header.is_none() {
                    header = Some(serde_json::from_str(line).map_err(LogError::BadHeader)?);
                }
            } else if opts.diffs && is_diff_out(line) {
                match serde_json::from_str::<DiffRecord>(line) {
                    Ok(record) => {
                        diffs.insert(record.id, record.d.diff);
                    }
                    Err(_) => skipped_lines += 1,
                }
            }
        }

        Ok(Self {
            header: header.ok_or(LogError::MissingHeader)?,
            meta: last_meta.and_then(|line| serde_json::from_str(line).ok()),
            turns,
            subagent_turns,
            subagent_order,
            diffs,
            skipped_lines,
        })
    }

    pub fn title(&self) -> &str {
        self.meta.as_ref().map_or("", |meta| &meta.title)
    }
}

fn is_diff_out(line: &str) -> bool {
    line.strip_prefix(OUT_PREFIX)
        .and_then(|rest| rest.split_once(OUT_DATA_MARKER))
        .is_some_and(|(_, variant)| variant.starts_with(DIFF_VARIANT))
}

#[cfg(test)]
mod tests {
    use test_case::test_case;

    use super::*;

    const HEADER: &str =
        r#"{"t":"header","v":2,"id":"abc","model":"m","cwd":"/work","created_at":1790625106}"#;
    const USER_MSG: &str =
        r#"{"t":"msg","d":{"role":"user","content":[{"type":"text","text":"hi"}]}}"#;
    const META: &str = r#"{"t":"meta","title":"t","token_usage":null,"updated_at":5,"context_size":9,"usage_by_model":{"m":{"input":7,"output":3}}}"#;
    const DIFF_OUT: &str = r#"{"t":"out","id":"c1","d":{"Diff":{"path":"a.rs","before":"x\n","after":"y\n","summary":"edited a.rs"}}}"#;
    const PLAIN_OUT: &str = r#"{"t":"out","id":"c2","d":{"Plain":{"text":"ok"}}}"#;

    fn log(lines: &[&str]) -> String {
        lines.join("\n")
    }

    #[test]
    fn parses_header_turns_and_last_meta() {
        let first_meta = META.replace(r#""title":"t""#, r#""title":"old""#);
        let text = log(&[HEADER, USER_MSG, &first_meta, USER_MSG, META]);
        let session = Session::parse(&text, ParseOpts::default()).unwrap();
        assert_eq!(session.header.id, "abc");
        assert_eq!(session.turns.len(), 2);
        assert_eq!(session.title(), "t");
        assert_eq!(session.meta.unwrap().usage_by_model["m"].output, 3);
    }

    #[test]
    fn unknown_records_and_corrupt_lines_are_tolerated() {
        let text = log(&[
            HEADER,
            r#"{"t":"frame","d":{}}"#,
            r#"{"t":"msg","d":{"role":"user""#,
            USER_MSG,
        ]);
        let session = Session::parse(&text, ParseOpts::default()).unwrap();
        assert_eq!(session.turns.len(), 1);
        assert_eq!(session.skipped_lines, 1);
    }

    #[test]
    fn missing_header_is_an_error() {
        let result = Session::parse(USER_MSG, ParseOpts::default());
        assert!(matches!(result, Err(LogError::MissingHeader)));
    }

    #[test_case(false, 0; "off_by_default")]
    #[test_case(true, 1; "on_request")]
    fn diff_outputs_are_only_parsed_on_request(diffs: bool, expected: usize) {
        let text = log(&[HEADER, DIFF_OUT, PLAIN_OUT]);
        let session = Session::parse(&text, ParseOpts { diffs }).unwrap();
        assert_eq!(session.diffs.len(), expected);
    }

    #[test]
    fn unknown_block_types_do_not_fail_the_message() {
        let msg = r#"{"t":"msg","d":{"role":"assistant","content":[{"type":"future","x":1},{"type":"image","source":{}}]}}"#;
        let text = log(&[HEADER, msg]);
        let session = Session::parse(&text, ParseOpts::default()).unwrap();
        assert!(matches!(session.turns[0].content[0], Block::Other));
        assert!(matches!(session.turns[0].content[1], Block::Image {}));
    }

    #[test]
    fn subagent_turns_group_by_parent_call() {
        let sub = r#"{"t":"sub_msg","sub":"call_1","d":{"role":"user","content":[]}}"#;
        let text = log(&[HEADER, sub, sub]);
        let session = Session::parse(&text, ParseOpts::default()).unwrap();
        assert_eq!(session.subagent_turns["call_1"].len(), 2);
    }
}
