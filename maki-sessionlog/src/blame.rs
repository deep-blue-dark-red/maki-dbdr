use std::collections::BTreeMap;
use std::fmt::Write;
use std::ops::RangeInclusive;

use clap::ValueEnum;
use similar::{ChangeTag, TextDiff};

use crate::history::{Change, FileHistory};
use crate::model::Session;
use crate::render::session_header;

const BASE_LABEL: &str = "base";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlamedLine<'a> {
    pub text: &'a str,
    /// Index into the history's steps; `None` for lines that were already in
    /// the file before the first logged edit.
    pub origin: Option<usize>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
pub enum BlameView {
    #[default]
    Lines,
    Ranges,
    Summary,
}

#[derive(Debug, Clone, Default)]
pub struct BlameOpts {
    pub view: BlameView,
    pub lines: Option<RangeInclusive<usize>>,
}

/// Every step's `before` equals the previous step's `after` (history inserts
/// an unattributed step wherever that would not hold), so the origins carried
/// out of one diff always line up with the next diff's old side.
pub fn blame<'a>(history: &FileHistory<'a>) -> Vec<BlamedLine<'a>> {
    let (Some(first), Some(last)) = (history.steps.first(), history.steps.last()) else {
        return Vec::new();
    };
    let mut origins: Vec<Option<usize>> = vec![None; line_count(first.change.sides().0)];
    for (index, step) in history.steps.iter().enumerate() {
        let (before, after) = step.change.sides();
        let mut carried = Vec::with_capacity(origins.len());
        let mut old_line = 0;
        for change in TextDiff::from_lines(before, after).iter_all_changes() {
            match change.tag() {
                ChangeTag::Equal => {
                    carried.push(origins.get(old_line).copied().flatten());
                    old_line += 1;
                }
                ChangeTag::Delete => old_line += 1,
                ChangeTag::Insert => carried.push(Some(index)),
            }
        }
        origins = carried;
    }
    last.change
        .sides()
        .1
        .split_inclusive('\n')
        .zip(origins)
        .map(|(text, origin)| BlamedLine {
            text: text.trim_end_matches('\n'),
            origin,
        })
        .collect()
}

fn line_count(text: &str) -> usize {
    text.split_inclusive('\n').count()
}

fn origin_label(history: &FileHistory, origin: Option<usize>) -> String {
    let Some(step) = origin.map(|index| &history.steps[index]) else {
        return BASE_LABEL.to_owned();
    };
    let via = step
        .via
        .map_or(String::new(), |name| format!(" via {name}"));
    match step.change {
        Change::Edit(_) | Change::Write { .. } => format!("msg {} {}{via}", step.at, step.tool),
        Change::Unattributed { .. } => format!("{} before msg {}", step.tool, step.at),
    }
}

pub fn render(session: &Session, histories: &[FileHistory], opts: &BlameOpts) -> String {
    let mut out = session_header(session, "# ");
    for history in histories {
        let lines = blame(history);
        let _ = writeln!(out, "\n## {} ({} lines)", history.path, lines.len());
        if opts.view == BlameView::Summary {
            write_summary(&mut out, history, &lines, opts);
            continue;
        }
        let mut start = 0;
        while start < lines.len() {
            let origin = lines[start].origin;
            let run = lines[start..]
                .iter()
                .take_while(|line| line.origin == origin)
                .count();
            write_run(
                &mut out,
                history,
                &lines[start..start + run],
                origin,
                start + 1,
                opts,
            );
            start += run;
        }
    }
    out
}

fn write_summary(out: &mut String, history: &FileHistory, lines: &[BlamedLine], opts: &BlameOpts) {
    let mut counts: BTreeMap<Option<usize>, usize> = BTreeMap::new();
    for (index, line) in lines.iter().enumerate() {
        if opts
            .lines
            .as_ref()
            .is_none_or(|wanted| wanted.contains(&(index + 1)))
        {
            *counts.entry(line.origin).or_default() += 1;
        }
    }
    for (origin, count) in counts {
        let _ = writeln!(out, "{count:>6} {}", origin_label(history, origin));
    }
}

fn write_run(
    out: &mut String,
    history: &FileHistory,
    run: &[BlamedLine],
    origin: Option<usize>,
    first_line: usize,
    opts: &BlameOpts,
) {
    let last_line = first_line + run.len() - 1;
    let shown_start = opts
        .lines
        .as_ref()
        .map_or(first_line, |lines| first_line.max(*lines.start()));
    let shown_end = opts
        .lines
        .as_ref()
        .map_or(last_line, |lines| last_line.min(*lines.end()));
    if shown_start > shown_end {
        return;
    }
    let _ = writeln!(
        out,
        "@ {} {shown_start}-{shown_end}",
        origin_label(history, origin)
    );
    if opts.view == BlameView::Lines {
        for line in &run[shown_start - first_line..=shown_end - first_line] {
            let _ = writeln!(out, "{}", line.text);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::build;
    use crate::history::collect;
    use crate::model::ParseOpts;
    use crate::testutil::{call, diff_out, log};

    fn blamed(text: &str) -> Vec<(String, Option<usize>)> {
        let session = Session::parse(text, ParseOpts { diffs: true }).unwrap();
        let events = build(&session);
        let histories = collect(&events, &[]);
        blame(&histories[0])
            .into_iter()
            .map(|line| (line.text.to_owned(), line.origin))
            .collect()
    }

    fn two_edits() -> String {
        log(&[
            call("c1", "edit"),
            diff_out("c1", "a.rs", "a\\nb\\nc\\n", "a\\nB\\nc\\n"),
            call("c2", "edit"),
            diff_out("c2", "a.rs", "a\\nB\\nc\\n", "a\\nB\\nc\\nd\\n"),
        ])
    }

    #[test]
    fn lines_keep_the_step_that_introduced_them() {
        let lines = blamed(&two_edits());
        let expected = [("a", None), ("B", Some(0)), ("c", None), ("d", Some(1))];
        assert_eq!(
            lines,
            expected.map(|(text, origin)| (text.to_owned(), origin))
        );
    }

    #[test]
    fn deleted_lines_disappear_and_replaced_lines_are_reattributed() {
        let text = log(&[
            call("c1", "edit"),
            diff_out("c1", "a.rs", "a\\nb\\nc\\n", "a\\nc\\n"),
            call("c2", "edit"),
            diff_out("c2", "a.rs", "a\\nc\\n", "z\\nc\\n"),
        ]);
        let lines = blamed(&text);
        let expected = [("z", Some(1)), ("c", None)];
        assert_eq!(
            lines,
            expected.map(|(text, origin)| (text.to_owned(), origin))
        );
    }

    #[test]
    fn changes_outside_the_log_are_blamed_on_the_unattributed_step() {
        let text = log(&[
            call("c1", "edit"),
            diff_out("c1", "a.rs", "a\\n", "b\\n"),
            call("c2", "edit"),
            diff_out("c2", "a.rs", "b\\nx\\n", "b\\nx\\ny\\n"),
        ]);
        let session = Session::parse(&text, ParseOpts { diffs: true }).unwrap();
        let events = build(&session);
        let histories = collect(&events, &[]);
        let lines = blame(&histories[0]);

        let steps: Vec<_> = lines
            .iter()
            .map(|line| line.origin.map(|index| histories[0].steps[index].tool))
            .collect();
        assert_eq!(steps, [Some("edit"), Some("unattributed"), Some("edit")]);
    }

    #[test]
    fn render_groups_runs_under_their_origin() {
        let text = two_edits();
        let session = Session::parse(&text, ParseOpts { diffs: true }).unwrap();
        let events = build(&session);
        let histories = collect(&events, &[]);
        let out = render(&session, &histories, &BlameOpts::default());

        assert!(out.contains("## a.rs (4 lines)"), "{out}");
        assert!(
            out.contains(
                "@ base 1-1\na\n@ msg 0 edit 2-2\nB\n@ base 3-3\nc\n@ msg 1 edit 4-4\nd\n"
            ),
            "{out}"
        );
    }

    #[test]
    fn ranges_only_omits_text_and_line_filter_clips_runs() {
        let text = two_edits();
        let session = Session::parse(&text, ParseOpts { diffs: true }).unwrap();
        let events = build(&session);
        let histories = collect(&events, &[]);
        let opts = BlameOpts {
            view: BlameView::Ranges,
            lines: Some(2..=3),
        };
        let out = render(&session, &histories, &opts);

        assert!(out.ends_with("@ msg 0 edit 2-2\n@ base 3-3\n"), "{out}");
    }

    #[test]
    fn summary_counts_lines_per_origin_in_step_order() {
        let text = two_edits();
        let session = Session::parse(&text, ParseOpts { diffs: true }).unwrap();
        let events = build(&session);
        let histories = collect(&events, &[]);
        let opts = BlameOpts {
            view: BlameView::Summary,
            lines: None,
        };
        let out = render(&session, &histories, &opts);

        assert!(
            out.ends_with("     2 base\n     1 msg 0 edit\n     1 msg 1 edit\n"),
            "{out}"
        );
    }
}
