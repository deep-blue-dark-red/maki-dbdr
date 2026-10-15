pub mod blame;
pub mod effects;
pub mod events;
pub mod filter;
pub mod fs;
pub mod history;
pub mod model;
pub mod render;
pub mod shell;
pub mod stats;
#[cfg(test)]
mod testutil;

use crate::blame::BlameOpts;
use crate::effects::EffectsOpts;
use crate::events::build;
use crate::filter::Filter;
use crate::history::{HistoryOpts, collect};
use crate::model::{LogError, ParseOpts, Session};
use crate::render::{RenderOpts, render};

/// `None` when the filter leaves nothing to show.
pub fn render_log(
    text: &str,
    filter: &Filter,
    opts: &RenderOpts,
) -> Result<Option<String>, LogError> {
    let session = Session::parse(text, ParseOpts { diffs: opts.diffs })?;
    let events = filter.apply(build(&session));
    Ok((!events.is_empty()).then(|| render(&session, &events, opts)))
}

/// `None` when no file in the session was edited (or none matches `files`).
pub fn history_log(
    text: &str,
    files: &[String],
    opts: HistoryOpts,
) -> Result<Option<String>, LogError> {
    let session = Session::parse(text, ParseOpts { diffs: true })?;
    let events = build(&session);
    let histories = collect(&events, files);
    Ok((!histories.is_empty()).then(|| history::render(&session, &histories, opts)))
}

/// `None` when no file in the session was edited (or none matches `files`).
pub fn blame_log(
    text: &str,
    files: &[String],
    opts: &BlameOpts,
) -> Result<Option<String>, LogError> {
    let session = Session::parse(text, ParseOpts { diffs: true })?;
    let events = build(&session);
    let histories = collect(&events, files);
    Ok((!histories.is_empty()).then(|| blame::render(&session, &histories, opts)))
}

/// `None` when the session made no tool calls.
pub fn effects_log(text: &str, opts: EffectsOpts) -> Result<Option<String>, LogError> {
    let session = Session::parse(text, ParseOpts { diffs: true })?;
    let events = build(&session);
    let report = effects::collect(&events);
    Ok((report.call_count() > 0).then(|| effects::render(&session, &report, opts)))
}
