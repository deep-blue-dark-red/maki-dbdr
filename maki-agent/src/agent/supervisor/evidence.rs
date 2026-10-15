//! What the harness itself sees of a run, so the supervisor need not take the
//! worker's word: the change to the working tree, and the checks the user
//! configured.

use std::fmt::Write;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use futures_lite::{AsyncReadExt, future};
use smol::Timer;
use tempfile::TempDir;

use super::render::clip;
use crate::ChildGuard;

const DIFF_CAP: usize = 12_000;
const CHECK_TIMEOUT: Duration = Duration::from_secs(300);
const CHECK_TAIL_LINES: usize = 20;
const LINE_CAP: usize = 200;
const SCRATCH_INDEX: &str = "index";
const NO_REPOSITORY: &str = "<diff>unavailable: not a git repository</diff>\n";
const NO_CHANGES: &str = "<diff>no changes</diff>\n";

/// The working tree as a git tree, untracked files included. Written through
/// a scratch copy of the index, so the user's own staging is left alone.
pub(super) async fn snapshot(cwd: PathBuf) -> Option<String> {
    smol::unblock(move || snapshot_blocking(&cwd)).await
}

pub(super) struct Diff {
    pub text: String,
    /// `None` outside a git repository.
    pub changed: Option<bool>,
}

/// The change since `base`, or why there is none to show.
pub(super) async fn diff(cwd: PathBuf, base: Option<String>) -> Diff {
    smol::unblock(move || {
        let Some((base, now)) = base.zip(snapshot_blocking(&cwd)) else {
            return Diff {
                text: NO_REPOSITORY.to_owned(),
                changed: None,
            };
        };
        if base == now {
            return Diff {
                text: NO_CHANGES.to_owned(),
                changed: Some(false),
            };
        }
        let patch = git(
            &cwd,
            None,
            &[
                "diff",
                "--no-color",
                "--no-ext-diff",
                "--stat",
                "--patch",
                &base,
                &now,
            ],
        )
        .unwrap_or_default();
        Diff {
            text: format!("<diff>\n{}\n</diff>\n", clip(&patch, DIFF_CAP)),
            changed: Some(true),
        }
    })
    .await
}

fn snapshot_blocking(cwd: &Path) -> Option<String> {
    let index = git(cwd, None, &["rev-parse", "--git-path", "index"])?;
    let scratch = TempDir::new().ok()?;
    let scratch_index = scratch.path().join(SCRATCH_INDEX);
    // Seeding from the real index spares rehashing every unchanged file. A
    // repository with no commits has no index yet, and git starts one.
    let _ = fs::copy(cwd.join(index), &scratch_index);
    git(cwd, Some(&scratch_index), &["add", "--all"])?;
    git(cwd, Some(&scratch_index), &["write-tree"])
}

fn git(cwd: &Path, index: Option<&Path>, args: &[&str]) -> Option<String> {
    let mut command = Command::new("git");
    command
        .args(args)
        .current_dir(cwd)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null());
    if let Some(index) = index {
        command.env("GIT_INDEX_FILE", index);
    }
    let output = command.output().ok()?;
    output.status.success().then(|| {
        String::from_utf8_lossy(&output.stdout)
            .trim_end()
            .to_owned()
    })
}

/// Runs each command through `sh`, appends its exit code and the tail of its
/// output, and returns the exit codes.
pub(super) async fn checks(cwd: &Path, commands: &[String], out: &mut String) -> Vec<String> {
    if commands.is_empty() {
        return Vec::new();
    }
    out.push_str("<checks>\n");
    let mut exits = Vec::with_capacity(commands.len());
    for command in commands {
        let (exit, output) = check(cwd, command).await;
        let _ = writeln!(
            out,
            "<check exit=\"{exit}\">\n$ {command}\n{}</check>",
            tail(&output)
        );
        exits.push(exit);
    }
    out.push_str("</checks>\n");
    exits
}

async fn check(cwd: &Path, command: &str) -> (String, String) {
    let mut std_command = Command::new("sh");
    std_command
        .arg("-c")
        .arg(format!("exec 2>&1\n{command}"))
        .current_dir(cwd);
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut std_command, 0);
    // The conversion keeps the process group but not the stdio settings.
    let mut command: async_process::Command = std_command.into();
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => return ("error".to_owned(), error.to_string()),
    };
    let Some(mut stdout) = child.stdout.take() else {
        return ("error".to_owned(), String::new());
    };
    // Dropping the guard on a timeout kills the whole process group.
    let mut guard = ChildGuard::new(child);
    let ran = future::or(
        async {
            let mut output = Vec::new();
            let _ = stdout.read_to_end(&mut output).await;
            let status = guard.status().await.ok();
            Some((status, output))
        },
        async {
            Timer::after(CHECK_TIMEOUT).await;
            None
        },
    )
    .await;
    match ran {
        Some((status, output)) => (
            status
                .and_then(|status| status.code())
                .map_or_else(|| "signal".to_owned(), |code| code.to_string()),
            String::from_utf8_lossy(&output).into_owned(),
        ),
        None => (
            "timeout".to_owned(),
            format!("no exit after {}s", CHECK_TIMEOUT.as_secs()),
        ),
    }
}

fn tail(output: &str) -> String {
    let lines: Vec<&str> = output.lines().collect();
    let skipped = lines.len().saturating_sub(CHECK_TAIL_LINES);
    let mut out = String::new();
    if skipped > 0 {
        let _ = writeln!(out, "[{skipped} lines]");
    }
    for line in &lines[skipped..] {
        let _ = writeln!(out, "{}", clip(line, LINE_CAP));
    }
    out
}

#[cfg(test)]
mod tests {
    use test_case::test_case;

    use super::*;

    fn repo() -> TempDir {
        let dir = TempDir::new().unwrap();
        assert!(git(dir.path(), None, &["init", "--quiet"]).is_some());
        dir
    }

    fn run_checks(commands: &[&str]) -> (String, Vec<String>) {
        let dir = TempDir::new().unwrap();
        let commands: Vec<String> = commands.iter().map(|c| (*c).to_owned()).collect();
        let mut out = String::new();
        let exits = smol::block_on(checks(dir.path(), &commands, &mut out));
        (out, exits)
    }

    #[test]
    fn diff_shows_edits_and_new_files_and_leaves_the_index_alone() {
        let dir = repo();
        let root = dir.path().to_path_buf();
        fs::write(root.join("kept.txt"), "one\n").unwrap();
        let base = smol::block_on(snapshot(root.clone()));
        assert!(base.is_some());
        fs::write(root.join("kept.txt"), "two\n").unwrap();
        fs::write(root.join("added.txt"), "new\n").unwrap();

        let out = smol::block_on(diff(root.clone(), base));

        assert_eq!(out.changed, Some(true));
        assert!(out.text.contains("-one\n+two"));
        assert!(out.text.contains("added.txt"));
        assert_eq!(
            git(&root, None, &["diff", "--cached", "--name-only"]),
            Some(String::new())
        );
    }

    #[test]
    fn untouched_tree_has_no_changes() {
        let dir = repo();
        let root = dir.path().to_path_buf();
        fs::write(root.join("a.txt"), "a\n").unwrap();
        let base = smol::block_on(snapshot(root.clone()));
        let out = smol::block_on(diff(root, base));
        assert_eq!((out.text.as_str(), out.changed), (NO_CHANGES, Some(false)));
    }

    #[test]
    fn outside_a_repository_there_is_no_diff() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        assert_eq!(smol::block_on(snapshot(root.clone())), None);
        let out = smol::block_on(diff(root, None));
        assert_eq!((out.text.as_str(), out.changed), (NO_REPOSITORY, None));
    }

    #[test_case("echo checked; exit 3", "3", "checked" ; "exit_code_and_output")]
    #[test_case("echo oops >&2", "0", "oops" ; "stderr_is_kept")]
    fn check_reports_exit_and_output(command: &str, exit: &str, output: &str) {
        let (out, exits) = run_checks(&[command]);
        assert_eq!(exits, [exit]);
        assert!(out.contains(&format!("<check exit=\"{exit}\">\n$ {command}\n{output}\n")));
    }

    #[test]
    fn no_checks_add_nothing() {
        let (out, exits) = run_checks(&[]);
        assert!(out.is_empty() && exits.is_empty());
    }

    #[test]
    fn long_output_keeps_its_tail() {
        let output: String = (1..=CHECK_TAIL_LINES + 5)
            .map(|n| format!("line{n}\n"))
            .collect();
        let out = tail(&output);
        assert!(out.starts_with("[5 lines]\nline6\n"));
        assert!(out.ends_with(&format!("line{}\n", CHECK_TAIL_LINES + 5)));
    }
}
