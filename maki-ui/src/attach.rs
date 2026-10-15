//! `maki attach`: a read-only TUI that follows a session another process
//! owns. It never claims the session, so the owner keeps running; the view
//! tails the session log at turn granularity and reads the owner's shared
//! memory feed for live state between saves. The only input that reaches the
//! owner is `c`, a cancel published on the control rings the TUI and headless
//! modes already poll.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use color_eyre::Result;
use color_eyre::eyre::eyre;
use crossterm::event::{
    self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
};
use maki_config::UiConfig;
use maki_lua::EventHandle;
use maki_shmem::auth::AuthKey;
use maki_shmem::broadcast::SessionState;
use maki_shmem::control::{AckResult, Command, command_record};
use maki_shmem::ring::{self, Consumer, Producer, Ring, RingVisitor, StartAt};
use maki_shmem::schema::blit_runtime;
use maki_shmem::text;
use maki_shmem::{MakiAck, MakiCommand, MakiStatus};
use maki_storage::StateDir;
use maki_storage::id::MakiId;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};

use crate::AppSession;
use crate::chat::{Chat, history_to_display};
use crate::repaint::Dirty;
use crate::theme;

/// How often the viewer polls the session log and the owner's feed.
const POLL: Duration = Duration::from_millis(250);
/// How often the ring directory is rescanned for a process that took over.
const RESCAN: Duration = Duration::from_secs(2);
/// A feed whose producer stopped beating is dropped after this long.
const FEED_STALE: Duration = Duration::from_secs(5);
/// How long an ack or error stays in the footer.
const NOTICE_TTL: Duration = Duration::from_secs(4);
const ATTACH_LABEL: &str = "attach";
const CANCEL_SENT: &str = "cancel sent";
const CANCEL_NO_OWNER: &str = "no live maki owns this session";
const CONTROL_KEY_MISSING: &str = "control rings unavailable";
const ACK_OK: &str = "cancelled";
const ACK_NO_SESSION: &str = "owner does not have this session";
const ACK_REJECTED: &str = "owner refused the cancel";

/// Everything `run` needs; the caller resolves the id, storage and config.
pub struct AttachParams {
    pub id: MakiId,
    pub storage: StateDir,
    pub ui_config: UiConfig,
    /// Folder pick or config theme, applied before the first frame.
    pub theme_name: Option<String>,
}

/// Watches `params.id` until the user quits. The session is only ever read,
/// so the process that owns it is untouched.
pub fn run(params: AttachParams) -> Result<()> {
    let session = AppSession::read_only(params.id, &params.storage)
        .map_err(|e| eyre!("cannot read session {}: {e}", params.id))?;
    if let Some(name) = &params.theme_name
        && let Ok(theme) = theme::load_by_name(name)
    {
        theme::set_current_name(name);
        theme::set(theme);
    }
    let (_guard, mut terminal) = crate::terminal::TerminalGuard::init()?;
    crate::color_compat::init();
    Viewer::new(session, params)?.run(&mut terminal)
}

/// One line of the owner's feed: what the header shows between saves.
struct Live {
    state: SessionState,
    context_used: u32,
    context_limit: u32,
    cost_usd: f64,
    last_tool: String,
}

/// The owner's ring, adopted once a status record for our session shows up.
struct StatusFeed {
    ring: Ring,
    consumer: Consumer,
}

/// This process's control ring, the one owners poll for commands.
struct ControlRing {
    ring: Ring,
    next_request: u64,
}

struct Notice {
    text: String,
    at: Instant,
}

struct Viewer {
    id: MakiId,
    id_str: String,
    title: String,
    storage: StateDir,
    ui_config: UiConfig,
    chat: Chat,
    log_path: Option<PathBuf>,
    file_sig: Option<(u64, u64)>,
    /// Message count and revision last rendered, to skip no-op reloads.
    seen: (usize, u64),
    feed: Option<StatusFeed>,
    rescan_at: Instant,
    live: Option<Live>,
    notice: Option<Notice>,
    key: Option<AuthKey>,
    control: Option<ControlRing>,
    chat_area: Rect,
}

impl Viewer {
    fn new(session: AppSession, params: AttachParams) -> Result<Self> {
        let mut viewer = Self {
            log_path: AppSession::log_path(params.id, &params.storage),
            file_sig: None,
            seen: (0, 0),
            feed: None,
            rescan_at: Instant::now(),
            live: None,
            notice: None,
            key: AuthKey::load_or_create(),
            title: String::new(),
            chat: Chat::new(
                params.id,
                None,
                "Main".into(),
                params.ui_config.clone(),
                EventHandle::disconnected(),
            ),
            id: params.id,
            id_str: params.id.to_string(),
            storage: params.storage,
            ui_config: params.ui_config,
            control: Self::start_control().ok(),
            chat_area: Rect::default(),
        };
        let _ = viewer.reload(session);
        Ok(viewer)
    }

    /// The command ring every maki process polls, created here so a cancel has
    /// somewhere to go. Its file is unlinked when the ring drops.
    fn start_control() -> Result<ControlRing> {
        let path = format!("{}/haunt-{}.brng", ring::ring_dir(), std::process::id());
        let ring = Ring::create(&path, ring::HAUNT_NODE_ID, 0)?;
        ring.update_status(ring::Status::Running);
        Ok(ControlRing {
            ring,
            next_request: ring::monotonic_nanos() as u64,
        })
    }

    fn run(&mut self, terminal: &mut ratatui::DefaultTerminal) -> Result<()> {
        self.draw(terminal)?;
        loop {
            let mut dirty = self.poll_changes();
            dirty |= self.chat.tick();
            let timeout = self.chat.cadence().frame().unwrap_or(POLL);
            if event::poll(timeout)? {
                if self.quit_on(event::read()?) {
                    return Ok(());
                }
                dirty = Dirty::YES;
            }
            if dirty.take() {
                self.draw(terminal)?;
            }
        }
    }

    /// True when the event ends the viewer.
    fn quit_on(&mut self, event: Event) -> bool {
        let ctrl = |key: &crossterm::event::KeyEvent| key.modifiers.contains(KeyModifiers::CONTROL);
        match event {
            Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                KeyCode::Char('q') | KeyCode::Esc => return true,
                KeyCode::Char('c') if ctrl(&key) => return true,
                KeyCode::Char('c') => self.send_cancel(),
                KeyCode::PageUp => self.chat.scroll(-self.chat.page()),
                KeyCode::PageDown => self.chat.scroll(self.chat.page()),
                KeyCode::Up | KeyCode::Char('k') => self.chat.scroll(-1),
                KeyCode::Down | KeyCode::Char('j') => self.chat.scroll(1),
                _ => {}
            },
            Event::Mouse(mouse) => match mouse.kind {
                MouseEventKind::ScrollUp => self.chat.scroll(-3),
                MouseEventKind::ScrollDown => self.chat.scroll(3),
                MouseEventKind::Down(MouseButton::Left) => {
                    self.chat.handle_click(mouse.row, self.chat_area)
                }
                _ => {}
            },
            _ => {}
        }
        false
    }

    /// The file watch and the owner's feed, both at poll cadence.
    fn poll_changes(&mut self) -> Dirty {
        let mut dirty = self.watch_log();
        dirty |= self.poll_feed();
        if let Some(control) = &self.control {
            control.ring.update_heartbeat(ring::monotonic_nanos());
        }
        if self
            .notice
            .as_ref()
            .is_some_and(|n| n.at.elapsed() > NOTICE_TTL)
        {
            self.notice = None;
            dirty = Dirty::YES;
        }
        dirty
    }

    /// Reloads the session when its file changed, at turn granularity.
    fn watch_log(&mut self) -> Dirty {
        let Some(path) = &self.log_path else {
            return Dirty::NO;
        };
        let Ok(meta) = std::fs::metadata(path) else {
            return Dirty::NO;
        };
        let sig = (
            meta.len(),
            meta.modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as u64)
                .unwrap_or_default(),
        );
        if Some(sig) == self.file_sig {
            return Dirty::NO;
        }
        self.file_sig = Some(sig);
        match AppSession::read_only(self.id, &self.storage) {
            Ok(session) => self.reload(session),
            Err(_) => Dirty::NO,
        }
    }

    fn reload(&mut self, session: AppSession) -> Dirty {
        let seen = (session.messages().len(), session.revision());
        if seen == self.seen {
            return Dirty::NO;
        }
        self.seen = seen;
        self.title = session.title.clone();
        let cost = session.token_usage.cost;
        let context_size = session.meta.context_size;
        let model = session.model.clone();
        let (display, _restores) = history_to_display(
            session.messages(),
            session.tool_outputs(),
            &self.ui_config.tool_output_lines,
        );
        self.chat.load_messages(display);
        self.chat.cost = cost;
        self.chat.context_size = context_size;
        self.chat.model_id = Some(model);
        Dirty::YES
    }

    /// Adopts the ring of whichever process owns this session, then follows
    /// its status records until the producer stops beating.
    fn poll_feed(&mut self) -> Dirty {
        let mut dirty = Dirty::NO;
        let mut visitor = FeedVisitor {
            id: &self.id_str,
            key: self.key.as_ref(),
            live: None,
            acks: Vec::new(),
        };
        if let Some(feed) = &mut self.feed {
            if feed_alive(&feed.ring) {
                feed.consumer.read_all(&feed.ring, &mut visitor);
            } else {
                self.feed = None;
                self.live = None;
                self.rescan_at = Instant::now();
                return Dirty::YES;
            }
        } else if self.rescan_at.elapsed() >= RESCAN {
            self.rescan_at = Instant::now();
            for path in ring::session_ring_paths() {
                let Ok(ring) = Ring::attach(&path, 0) else {
                    continue;
                };
                let mut consumer = Consumer::new(&ring, StartAt::Earliest);
                visitor.live = None;
                consumer.read_all(&ring, &mut visitor);
                if visitor.live.is_some() {
                    self.feed = Some(StatusFeed { ring, consumer });
                    break;
                }
            }
        }
        if visitor.live.is_some() {
            self.live = visitor.live;
            dirty = Dirty::YES;
        }
        for (_, result) in visitor.acks {
            self.flash(ack_text(result));
            dirty = Dirty::YES;
        }
        dirty
    }

    fn send_cancel(&mut self) {
        if self.feed.is_none() {
            self.flash(CANCEL_NO_OWNER);
            return;
        }
        let Some(key) = &self.key else {
            self.flash(CONTROL_KEY_MISSING);
            return;
        };
        let Some(control) = &mut self.control else {
            self.flash(CONTROL_KEY_MISSING);
            return;
        };
        let mut record = command_record(
            control.next_request,
            &self.id_str,
            &Command::Cancel,
            ring::monotonic_nanos() as u64,
        );
        key.sign_command(&mut record);
        let mut producer = Producer::new(&control.ring);
        producer.publish(MakiCommand::MSG_ID.get(), &record);
        producer.flush();
        control.ring.update_heartbeat(ring::monotonic_nanos());
        control.next_request += 1;
        self.flash(CANCEL_SENT);
    }

    fn flash(&mut self, text: &str) {
        self.notice = Some(Notice {
            text: text.to_owned(),
            at: Instant::now(),
        });
    }

    fn draw(&mut self, terminal: &mut ratatui::DefaultTerminal) -> Result<()> {
        let header = self.header();
        let footer = self.footer();
        terminal.draw(|frame| {
            let [head, chat_area, foot] = Layout::vertical([
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(1),
            ])
            .areas(frame.area());
            frame.render_widget(header, head);
            self.chat.view(frame, chat_area, false, true);
            frame.render_widget(footer, foot);
            self.chat_area = chat_area;
        })?;
        Ok(())
    }

    fn header(&self) -> Line<'static> {
        let t = theme::current();
        let mut spans = vec![
            Span::styled(ATTACH_LABEL, t.status_dim),
            Span::raw(" "),
            Span::styled(self.title.clone(), t.bold),
        ];
        match &self.live {
            Some(live) => {
                spans.extend(match live.state {
                    SessionState::Working => vec![Span::styled(" working", t.accent)],
                    SessionState::Waiting => vec![Span::styled(" needs input", t.error)],
                    SessionState::Idle => vec![Span::styled(" idle", t.status_dim)],
                });
                if live.state == SessionState::Working && !live.last_tool.is_empty() {
                    spans.push(Span::styled(format!(" {}", live.last_tool), t.tool_dim));
                }
                spans.push(Span::styled(
                    format!(
                        " {}k/{}k ${:.2}",
                        kilo(live.context_used),
                        kilo(live.context_limit),
                        live.cost_usd
                    ),
                    t.status_dim,
                ));
            }
            None => spans.push(Span::styled(" saved transcript", t.status_dim)),
        }
        Line::from(spans)
    }

    fn footer(&self) -> Line<'static> {
        let t = theme::current();
        let mut spans = vec![
            Span::styled("q", t.keybind_key),
            Span::styled(" quit  ", t.keybind_desc),
            Span::styled("c", t.keybind_key),
            Span::styled(" cancel  ", t.keybind_desc),
            Span::styled("PgUp/PgDn", t.keybind_key),
            Span::styled(" scroll", t.keybind_desc),
        ];
        if let Some(notice) = &self.notice {
            spans.push(Span::raw("  "));
            spans.push(Span::styled(notice.text.clone(), t.accent));
        }
        Line::from(spans)
    }
}

fn feed_alive(ring: &Ring) -> bool {
    let (status, beat) = ring.peer_health();
    status != ring::Status::Stopped
        && (beat == 0 || ring::monotonic_nanos() - beat < FEED_STALE.as_nanos() as i64)
}

fn kilo(n: u32) -> String {
    if n >= 1000 {
        format!("{:.1}", n as f64 / 1000.0)
    } else {
        n.to_string()
    }
}

fn ack_text(result: u8) -> &'static str {
    match result {
        x if x == AckResult::Ok as u8 => ACK_OK,
        x if x == AckResult::NoSession as u8 => ACK_NO_SESSION,
        _ => ACK_REJECTED,
    }
}

/// Collects this session's status and ack records off one ring.
struct FeedVisitor<'a> {
    id: &'a str,
    key: Option<&'a AuthKey>,
    live: Option<Live>,
    acks: Vec<(u64, u8)>,
}

impl RingVisitor for FeedVisitor<'_> {
    fn on_record(
        &mut self,
        _sequence: u64,
        msg_id: u16,
        body: &[u8; ring::BODY_BYTES],
        _last: bool,
    ) {
        if msg_id == MakiStatus::MSG_ID.get() {
            let Some(record) = (unsafe { blit_runtime::from_bytes::<MakiStatus>(body) }) else {
                return;
            };
            if self.key.is_some_and(|key| !key.verify_status(record)) {
                return;
            }
            if text::decode(&record.session_id) != self.id {
                return;
            }
            self.live = Some(Live {
                // Wire contract: Working=1, Idle=2, Waiting=3; anything else
                // reads as working so an unknown value never hides progress.
                state: match record.state {
                    2 => SessionState::Idle,
                    3 => SessionState::Waiting,
                    _ => SessionState::Working,
                },
                context_used: record.context_used,
                context_limit: record.context_limit,
                cost_usd: record.cost_usd,
                last_tool: text::decode(&record.last_tool).into_owned(),
            });
        } else if msg_id == MakiAck::MSG_ID.get() {
            let Some(record) = (unsafe { blit_runtime::from_bytes::<MakiAck>(body) }) else {
                return;
            };
            if self.key.is_some_and(|key| !key.verify_ack(record)) {
                return;
            }
            if text::decode(&record.session_id) == self.id {
                self.acks.push((record.request_id, record.result));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maki_shmem::broadcast::{Broadcaster, SessionSnapshot};

    use test_case::test_case;

    /// Polls `check` until it passes or the deadline does, so the test waits
    /// for the broadcaster's 500ms period without sleeping a fixed time.
    fn soon(check: impl FnMut() -> bool) -> bool {
        let mut check = check;
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if check() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        false
    }

    #[test]
    fn follows_a_live_owner_and_cancels_over_the_control_ring() {
        let broadcaster = Broadcaster::start().expect("session ring");
        let mut session = AppSession::new("test/model", "cwd");
        session.id = MakiId::generate();
        let id = session.id;
        let viewer = Viewer::new(
            session,
            AttachParams {
                id,
                storage: StateDir::from_path(tempfile::tempdir().expect("temp state dir").keep()),
                ui_config: UiConfig::default(),
                theme_name: None,
            },
        );
        let mut viewer = viewer.expect("viewer");
        viewer.rescan_at = Instant::now() - RESCAN;
        broadcaster.update(SessionSnapshot {
            state: SessionState::Working,
            context_used: 1_234,
            context_limit: 200_000,
            cost_usd: 0.5,
            session_id: viewer.id_str.clone(),
            title: "live".into(),
            last_tool: "bash".into(),
        });

        assert!(
            soon(|| {
                let _ = viewer.poll_feed();
                viewer.live.is_some()
            }),
            "owner ring adopted"
        );
        let live = viewer.live.as_ref().expect("live status");
        assert_eq!(live.state, SessionState::Working);
        assert_eq!(live.last_tool, "bash");
        assert_eq!(live.context_used, 1_234);

        viewer.send_cancel();
        let mut pending = Vec::new();
        assert!(
            soon(|| {
                pending = broadcaster.drain_commands();
                !pending.is_empty()
            }),
            "cancel reached the owner"
        );
        assert_eq!(pending[0].command, Command::Cancel);
        assert_eq!(pending[0].session_id, viewer.id_str);
        broadcaster.ack(&viewer.id_str, pending[0].request_id, AckResult::Ok);

        assert!(
            soon(|| {
                let _ = viewer.poll_feed();
                viewer.notice.as_ref().is_some_and(|n| n.text == ACK_OK)
            }),
            "ack shown in the footer"
        );
    }

    #[test_case(0 => "0"; "kilo_zero")]
    #[test_case(999 => "999"; "kilo_below_thousand")]
    #[test_case(1234 => "1.2"; "kilo_thousands")]
    #[test_case(200_000 => "200.0"; "kilo_context_limit")]
    fn kilo_formats(n: u32) -> String {
        kilo(n)
    }

    #[test]
    fn ack_results_map_to_notices() {
        assert_eq!(ack_text(AckResult::Ok as u8), ACK_OK);
        assert_eq!(ack_text(AckResult::NoSession as u8), ACK_NO_SESSION);
        assert_eq!(ack_text(AckResult::Rejected as u8), ACK_REJECTED);
        assert_eq!(ack_text(0), ACK_REJECTED);
    }
}
