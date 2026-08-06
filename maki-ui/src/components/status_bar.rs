use std::borrow::Cow;
use std::env;
use std::path::Path;
use std::time::{Duration, Instant};

use super::{RetryInfo, Status};


use crate::theme;

use maki_providers::{ModelPricing, TokenUsage, format_tokens};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

const FAST_LABEL: &str = " [fast]";
const WORKFLOW_LABEL: &str = " [workflow]";

pub struct UsageStats<'a> {
    pub global_usage: &'a TokenUsage,
    pub context_size: u32,
    pub cost: Option<f64>,
    pub pricing: &'a ModelPricing,
    pub context_window: u32,
    pub show_global: bool,
}

pub struct StreamingInfo {
    pub duration: Duration,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub active_tools: Vec<String>,
}

pub struct TurnStats {
    pub pp_tps: f64,
    pub tg_tps: f64,
    pub cache_rate: f64,
}

pub struct StatusBarContext<'a> {
    pub status: &'a Status,
    pub mode_label: Cow<'static, str>,
    pub mode_style: Style,
    pub model_id: &'a str,
    pub stats: UsageStats<'a>,
    pub auto_scroll: bool,
    pub chat_name: Option<&'a str>,
    pub session_name: Option<&'a str>,
    pub retry_info: Option<&'a RetryInfo>,
    pub thinking_label: Option<Cow<'static, str>>,
    pub fast: bool,
    pub workflow: bool,
    pub restoring: bool,
    pub streaming_info: Option<StreamingInfo>,
    pub streaming_active: bool,
    pub verbose: bool,
    pub last_turn_stats: Option<&'a TurnStats>,
    pub show_token_stats: bool,
}

pub struct StatusBar {
    flash: Option<(String, Instant)>,
    cwd_branch: String,
    pub flash_duration: Duration,
    branch_update_rx: Option<flume::Receiver<()>>,
}

impl StatusBar {
    pub fn new(flash_duration: Duration) -> Self {
        Self {
            flash: None,
            cwd_branch: cwd_branch_label(),
            flash_duration,
            branch_update_rx: spawn_branch_watcher(),
        }
    }

    pub fn flash(&mut self, msg: String) {
        self.flash = Some((msg, Instant::now()));
    }

    #[cfg(test)]
    pub fn flash_text(&self) -> Option<&str> {
        self.flash.as_ref().map(|(s, _)| s.as_str())
    }

    pub fn refresh_cwd(&mut self) {
        self.cwd_branch = cwd_branch_label();
    }

    pub fn poll_branch_update(&mut self) {
        let Some(rx) = &self.branch_update_rx else {
            return;
        };
        if rx.try_iter().next().is_some() {
            self.cwd_branch = cwd_branch_label();
        }
    }

    pub fn clear_flash(&mut self) {
        self.flash = None;
    }

    pub fn clear_expired_hint(&mut self) {
        if self
            .flash
            .as_ref()
            .is_some_and(|(_, t)| t.elapsed() >= self.flash_duration)
        {
            self.flash = None;
        }
    }

    pub fn view(&self, frame: &mut Frame, area: Rect, ctx: &StatusBarContext) {
        let mut left_spans = Vec::new();

        if let Some(info) = &ctx.streaming_info {
            let info_style = if ctx.streaming_active {
                theme::current().spinner
            } else {
                theme::current().status_dim
            };
            if !info.active_tools.is_empty() {
                let tool_list = info.active_tools.join(", ");
                let duration_secs = info.duration.as_secs();
                left_spans.push(Span::styled(
                    format!(" ✻ running {tool_list} ({duration_secs}s)"),
                    info_style,
                ));
            } else {
                let duration_secs = info.duration.as_secs();
                let mut stats = Vec::new();
                let is_working = info.output_tokens > 0;
                let status_label = if !ctx.streaming_active {
                    "Done   "
                } else if is_working {
                    "Working"
                } else {
                    "Waiting"
                };

                if !is_working && info.input_tokens > 0 {
                    stats.push(format!("↑ {} t", format_tokens(info.input_tokens)));
                } else if is_working && info.output_tokens > 0 {
                    stats.push(format!("↓ {} t", format_tokens(info.output_tokens)));
                }

                let stats_str = if stats.is_empty() {
                    format!("({duration_secs}s)")
                } else {
                    format!("({duration_secs}s · {})", stats.join(" · "))
                };
                left_spans.push(Span::styled(
                    format!(" ✻ {status_label} {stats_str}"),
                    info_style,
                ));
            }
        } else if *ctx.status == Status::Streaming {
            left_spans.push(Span::styled(" ✻", theme::current().spinner));
        }

        if let Some(name) = ctx.session_name {
            left_spans.push(Span::styled(
                format!(" [{name}]"),
                theme::current().status_dim,
            ));
        }

        let mut token_stats_idx: Option<usize> = None;
        let mut token_stats_abbrev: Option<String> = None;
        if ctx.show_token_stats
            && let Some(stats) = ctx.last_turn_stats
        {
            let abbrev = format!(
                " (PP {:.0}|TG {:.0}|CR {:.0})",
                stats.pp_tps,
                stats.tg_tps,
                stats.cache_rate * 100.0,
            );
            token_stats_idx = Some(left_spans.len());
            token_stats_abbrev = Some(abbrev);
            left_spans.push(Span::styled(
                format!(
                    " PP {:.1} t/s | TG {:.1} t/s | CR {:.1}%",
                    stats.pp_tps,
                    stats.tg_tps,
                    stats.cache_rate * 100.0,
                ),
                theme::current().status_dim,
            ));
        }

        if ctx.restoring {
            left_spans.push(Span::styled(
                " ✻",
                theme::current().status_notice,
            ));
        }

        if let Some(name) = ctx.chat_name {
            left_spans.push(Span::styled(
                format!(" [{name}]"),
                theme::current().status_dim,
            ));
        }

        if !ctx.auto_scroll {
            left_spans.push(Span::styled(
                " auto-scroll paused",
                theme::current().status_dim,
            ));
        }

        if let Some(retry) = ctx.retry_info {
            let secs = retry
                .deadline
                .saturating_duration_since(Instant::now())
                .as_secs();
            left_spans.push(Span::styled(
                format!(" {}", retry.message),
                theme::current().status_retry_error,
            ));
            left_spans.push(Span::styled(
                format!(" · retrying in {secs}s (#{})", retry.attempt),
                theme::current().status_retry_info,
            ));
        }

        if let Status::Error { message: e, .. } = ctx.status {
            left_spans.push(Span::styled(format!(" {e}"), theme::current().error));
        }

        if let Some((ref msg, _)) = self.flash {
            left_spans.push(Span::styled(
                format!(" {msg}"),
                theme::current().status_notice,
            ));
        }

        let mut right_spans = Vec::new();

        if !matches!(ctx.status, Status::Error { .. }) {
            let pct = if ctx.stats.context_window > 0 {
                (ctx.stats.context_size as f64 / ctx.stats.context_window as f64 * 100.0) as u32
            } else {
                0
            };

            // Order: [verbose] [thinking] [fast] mode  ctx/window (pct%) [$cost] [global]  model  cwd
            if ctx.verbose {
                right_spans.push(Span::styled("[verbose] ", theme::current().status_dim));
            }

            if let Some(ref label) = ctx.thinking_label {
                right_spans.push(Span::styled(
                    format!("[{label}] "),
                    theme::current().status_dim,
                ));
            }

            if ctx.fast {
                right_spans.push(Span::styled(
                    format!("{} ", FAST_LABEL.trim()),
                    theme::current().status_dim,
                ));
            }

            right_spans.push(Span::styled(format!("{}  ", ctx.mode_label), ctx.mode_style));

            let context_style = Style::new().fg(theme::current().foreground);
            let context_text = format!(
                "{}/{} ({}%)",
                format_tokens(ctx.stats.context_size),
                format_tokens(ctx.stats.context_window),
                pct,
            );
            let rest_text = match ctx.stats.cost {
                Some(cost) => format!("{context_text} ${cost:.3}  "),
                None => format!("{context_text}  "),
            };
            right_spans.push(Span::styled(rest_text, context_style));

            if ctx.stats.show_global && !ctx.stats.pricing.is_zero() {
                let global_text = format!(
                    "\u{03a3}${:.3}  ",
                    ctx.stats.global_usage.cost(ctx.stats.pricing, ctx.fast),
                );
                right_spans.push(Span::styled(global_text, context_style));
            }

            // Model ID: always strip middle path components
            let model_short = shorten_model_id(ctx.model_id);
            let model_idx = right_spans.len();
            right_spans.push(Span::styled(model_short.clone(), theme::current().status_dim));

            // CWD at far right, same color as context usage
            right_spans.push(Span::raw("  "));
            let cwd_idx = right_spans.len();
            right_spans.push(Span::styled(self.cwd_branch.clone(), context_style));
            right_spans.push(Span::raw(" "));

            // Adaptive shortening when total exceeds terminal width.
            // Pass 1: abbreviate cwd path components.
            // Pass 2: truncate model with "...".
            // Pass 3: abbreviate left-side token stats to compact form.
            let span_width = |spans: &[Span]| -> u16 {
                spans.iter().map(|s| s.width() as u16).sum()
            };
            let left_w = || -> u16 {
                left_spans.iter().map(|s| s.width() as u16).sum::<u16>().max(1)
            };

            if left_w() + span_width(&right_spans) > area.width {
                right_spans[cwd_idx] =
                    Span::styled(abbreviate_path_components(&self.cwd_branch), context_style);

                if left_w() + span_width(&right_spans) > area.width {
                    let overflow =
                        (left_w() + span_width(&right_spans)).saturating_sub(area.width) as usize;
                    let model_width = right_spans[model_idx].width();
                    if overflow + 3 < model_width {
                        let keep = model_width - overflow - 3;
                        let end = model_short
                            .char_indices()
                            .nth(keep)
                            .map(|(i, _)| i)
                            .unwrap_or(model_short.len());
                        right_spans[model_idx] = Span::styled(
                            format!("{}...", &model_short[..end]),
                            theme::current().status_dim,
                        );
                    } else {
                        right_spans[model_idx] =
                            Span::styled("...", theme::current().status_dim);
                    }

                    if left_w() + span_width(&right_spans) > area.width
                        && let (Some(idx), Some(abbrev)) = (token_stats_idx, token_stats_abbrev)
                    {
                        left_spans[idx] =
                            Span::styled(abbrev, theme::current().status_dim);
                    }
                }
            }
        }

        let left_width = left_spans.iter().map(|s| s.width() as u16).sum::<u16>().max(1);

        let [left_area, right_area] = Layout::horizontal([
            Constraint::Length(left_width),
            Constraint::Fill(1),
        ])
        .areas(area);

        frame.render_widget(Paragraph::new(Line::from(left_spans)), left_area);
        frame.render_widget(
            Paragraph::new(Line::from(right_spans)).alignment(Alignment::Right),
            right_area,
        );
    }
}

fn shorten_model_id(model_id: &str) -> String {
    let first = model_id.find('/');
    let last = model_id.rfind('/');
    match (first, last) {
        (Some(f), Some(l)) if f < l => format!("{}/{}", &model_id[..f], &model_id[l + 1..]),
        _ => model_id.to_string(),
    }
}

fn abbreviate_path_components(path: &str) -> String {
    // Separate optional ":branch" suffix (Unix paths never contain ':')
    let (path_part, branch_suffix) = path
        .find(':')
        .map(|i| (&path[..i], &path[i..]))
        .unwrap_or((path, ""));

    let parts: Vec<&str> = path_part.split('/').collect();
    if parts.len() <= 2 {
        return path.to_string();
    }
    let last = parts.len() - 1;
    let abbreviated: Vec<String> = parts
        .iter()
        .enumerate()
        .map(|(i, &p)| {
            if i == last || p.is_empty() || p == "~" {
                p.to_string()
            } else {
                p.chars().next().map(|c| c.to_string()).unwrap_or_default()
            }
        })
        .collect();
    format!("{}{}", abbreviated.join("/"), branch_suffix)
}

fn collapse_home(path: &str) -> String {
    let Some(home) = maki_storage::paths::home() else {
        return path.to_string();
    };
    collapse_home_with(path, &home.to_string_lossy())
}

fn collapse_home_with(path: &str, home: &str) -> String {
    path.strip_prefix(home)
        .map(|rest| format!("~{rest}"))
        .unwrap_or_else(|| path.to_string())
}

fn cwd_branch_label() -> String {
    let cwd = env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| ".".into());
    let label = collapse_home(&cwd);
    match detect_branch(&cwd) {
        Some(branch) => format!("{label}:{branch}"),
        None => label,
    }
}

fn detect_branch(cwd: &str) -> Option<String> {
    let head = std::fs::read_to_string(find_git_dir(Path::new(cwd))?.join("HEAD")).ok()?;
    let head = head.trim();
    head.strip_prefix("ref: refs/heads/")
        .map(str::to_string)
        .or_else(|| Some(head.get(..7)?.to_string()))
}

fn find_git_dir(cwd: &Path) -> Option<std::path::PathBuf> {
    let mut dir = cwd;
    loop {
        let git = dir.join(".git");
        if git.is_dir() {
            return Some(git);
        }
        dir = dir.parent()?;
    }
}

fn spawn_branch_watcher() -> Option<flume::Receiver<()>> {
    use notify::{RecursiveMode, Watcher};

    let cwd = env::current_dir().ok()?;
    let git_dir = find_git_dir(&cwd)?;
    let (tx, rx) = flume::bounded(1);

    std::thread::spawn(move || {
        let Ok(mut watcher) = notify::recommended_watcher(move |res: Result<notify::Event, _>| {
            if res.is_ok_and(|e| e.paths.iter().any(|p| p.ends_with("HEAD"))) {
                let _ = tx.try_send(());
            }
        }) else {
            return;
        };
        if watcher.watch(&git_dir, RecursiveMode::NonRecursive).is_ok() {
            std::thread::park();
        }
    });

    Some(rx)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use tempfile::TempDir;
    use test_case::test_case;

    #[test_case("/home/user/projects/app", "/home/user", "~/projects/app" ; "inside_home")]
    #[test_case("/tmp/other", "/home/user", "/tmp/other"                  ; "outside_home")]
    #[test_case("/home/user", "/home/user", "~"                           ; "exact_home")]
    fn collapse_home_cases(path: &str, home: &str, expected: &str) {
        assert_eq!(collapse_home_with(path, home), expected);
    }

    fn tmp_with_head(content: Option<&str>) -> (TempDir, String) {
        let dir = TempDir::new().unwrap();
        if let Some(head) = content {
            let git = dir.path().join(".git");
            fs::create_dir(&git).unwrap();
            fs::write(git.join("HEAD"), head).unwrap();
        }
        let path = dir.path().to_string_lossy().into_owned();
        (dir, path)
    }

    #[test_case(Some("ref: refs/heads/feature/foo\n"), Some("feature/foo") ; "regular_ref")]
    #[test_case(Some("abc1234deadbeef\n"),            Some("abc1234")      ; "detached_head")]
    #[test_case(None,                                 None                 ; "no_git_dir")]
    fn detect_branch_cases(head: Option<&str>, expected: Option<&str>) {
        let (_dir, path) = tmp_with_head(head);
        assert_eq!(detect_branch(&path), expected.map(String::from));
    }

    #[test]
    fn detect_branch_from_subdirectory() {
        let (_dir, path) = tmp_with_head(Some("ref: refs/heads/main\n"));
        let sub = Path::new(&path).join("sub");
        fs::create_dir(&sub).unwrap();
        assert_eq!(
            detect_branch(&sub.to_string_lossy()),
            Some("main".to_string())
        );
    }

    #[test]
    fn clear_expired_hint_removes_stale_flash() {
        let mut bar = StatusBar::new(Duration::ZERO);
        bar.flash("Copied".into());
        bar.clear_expired_hint();
        assert!(bar.flash.is_none());
    }

    #[test_case("llama-cpp//mnt/commons/models/Qwen3.gguf", "llama-cpp/Qwen3.gguf" ; "strips_middle")]
    #[test_case("llama-cpp/Qwen3.gguf", "llama-cpp/Qwen3.gguf"                    ; "single_slash_unchanged")]
    #[test_case("gpt-4", "gpt-4"                                                   ; "no_slash_unchanged")]
    #[test_case("a/b/c/d", "a/d"                                                   ; "many_segments")]
    fn shorten_model_id_cases(input: &str, expected: &str) {
        assert_eq!(shorten_model_id(input), expected);
    }

    #[test_case("~/git/maki/target/release:main", "~/g/m/t/release:main" ; "typical_path_with_branch")]
    #[test_case("~/git/maki:feature/foo", "~/g/maki:feature/foo"         ; "branch_with_slash")]
    #[test_case("~/git/maki", "~/g/maki"                                 ; "no_branch")]
    #[test_case("~/maki", "~/maki"                                        ; "short_path_unchanged")]
    #[test_case("/home/user/proj/src:main", "/h/u/p/src:main"            ; "absolute_path")]
    fn abbreviate_path_cases(input: &str, expected: &str) {
        assert_eq!(abbreviate_path_components(input), expected);
    }

    #[test]
    fn clear_flash_removes_flash() {
        let mut bar = StatusBar::new(Duration::from_secs(999));
        bar.flash("Copied".into());
        bar.clear_flash();
        assert!(bar.flash.is_none());
    }
}
