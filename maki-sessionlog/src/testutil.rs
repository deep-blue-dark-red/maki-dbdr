const HEADER: &str = r#"{"t":"header","v":2,"id":"s","model":"m","cwd":"/w","created_at":0}"#;

pub fn call(id: &str, name: &str) -> String {
    call_with_input(id, name, "{}")
}

pub fn call_with_input(id: &str, name: &str, input: &str) -> String {
    format!(
        r#"{{"t":"msg","d":{{"role":"assistant","content":[{{"type":"tool_use","id":"{id}","name":"{name}","input":{input}}}]}}}}"#
    )
}

pub fn reply(text: &str) -> String {
    format!(
        r#"{{"t":"msg","d":{{"role":"assistant","content":[{{"type":"text","text":"{text}"}}]}}}}"#
    )
}

pub fn thought(text: &str) -> String {
    format!(
        r#"{{"t":"msg","d":{{"role":"assistant","content":[{{"type":"thinking","thinking":"{text}"}}]}}}}"#
    )
}

pub fn result(id: &str, content: &str, is_error: bool) -> String {
    format!(
        r#"{{"t":"msg","d":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"{id}","content":"{content}","is_error":{is_error}}}]}}}}"#
    )
}

/// `before` and `after` are spliced into JSON, so newlines are written as `\\n`.
pub fn diff_out(id: &str, path: &str, before: &str, after: &str) -> String {
    format!(
        r#"{{"t":"out","id":"{id}","d":{{"Diff":{{"path":"{path}","before":"{before}","after":"{after}","summary":"s"}}}}}}"#
    )
}

pub fn log(lines: &[String]) -> String {
    std::iter::once(HEADER.to_owned())
        .chain(lines.iter().cloned())
        .collect::<Vec<_>>()
        .join("\n")
}
