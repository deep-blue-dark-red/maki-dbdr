use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::fmt::Write;
use strum::EnumIter;
use unicode_width::UnicodeWidthStr;

macro_rules! mod_key {
    ($suffix:expr) => {
        concat!("Ctrl+", $suffix)
    };
}

macro_rules! upper {
    ('a') => {
        "A"
    };
    ('b') => {
        "B"
    };
    ('c') => {
        "C"
    };
    ('d') => {
        "D"
    };
    ('e') => {
        "E"
    };
    ('f') => {
        "F"
    };
    ('g') => {
        "G"
    };
    ('h') => {
        "H"
    };
    ('i') => {
        "I"
    };
    ('j') => {
        "J"
    };
    ('k') => {
        "K"
    };
    ('l') => {
        "L"
    };
    ('m') => {
        "M"
    };
    ('n') => {
        "N"
    };
    ('o') => {
        "O"
    };
    ('p') => {
        "P"
    };
    ('q') => {
        "Q"
    };
    ('r') => {
        "R"
    };
    ('s') => {
        "S"
    };
    ('t') => {
        "T"
    };
    ('u') => {
        "U"
    };
    ('v') => {
        "V"
    };
    ('w') => {
        "W"
    };
    ('x') => {
        "X"
    };
    ('y') => {
        "Y"
    };
    ('z') => {
        "Z"
    };
}

macro_rules! ctrl_bind {
    ($char:tt) => {
        Bind {
            code: KeyCode::Char($char),
            modifiers: KeyModifiers::CONTROL,
            label: mod_key!(upper!($char)),
            name: None,
        }
    };
    ($char:tt, $name:expr) => {
        Bind {
            code: KeyCode::Char($char),
            modifiers: KeyModifiers::CONTROL,
            label: mod_key!(upper!($char)),
            name: Some($name),
        }
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bind {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
    pub label: &'static str,
    pub name: Option<&'static str>,
}

impl Bind {
    pub fn matches(&self, key: KeyEvent) -> bool {
        if let Some(name) = self.name
            && let Some(dyn_bind) = get_configured_bind(name)
        {
            return dyn_bind.matches_raw(key);
        }
        self.matches_raw(key)
    }

    fn matches_raw(&self, key: KeyEvent) -> bool {
        let code_match = match (self.code, key.code) {
            (KeyCode::Char(c1), KeyCode::Char(c2)) => c1.eq_ignore_ascii_case(&c2),
            (a, b) => a == b,
        };
        code_match && key.modifiers == self.modifiers
    }

    pub fn label(&self) -> &str {
        if let Some(name) = self.name
            && let Some(dyn_bind) = get_configured_bind(name)
        {
            return dyn_bind.label;
        }
        self.label
    }

    pub const fn to_key_event(self) -> KeyEvent {
        KeyEvent {
            code: self.code,
            modifiers: self.modifiers,
            kind: crossterm::event::KeyEventKind::Press,
            state: crossterm::event::KeyEventState::NONE,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfiguredKeybindings {
    pub quit: Bind,
    pub help: Bind,
    pub prev_chat: Bind,
    pub next_chat: Bind,
    pub scroll_half_up: Bind,
    pub scroll_half_down: Bind,
    pub scroll_line_up: Bind,
    pub scroll_line_down: Bind,
    pub scroll_top: Bind,
    pub scroll_bottom: Bind,
    pub pop_queue: Bind,
    pub delete_word: Bind,
    pub search: Bind,
    pub file_picker: Bind,
    pub toggle_verbose: Bind,
    pub open_editor: Bind,
    pub edit_system_prompt: Bind,
    pub plan_toggle: Bind,
    pub tasks: Bind,
    pub suspend: Bind,
    pub delete: Bind,
    pub kill_line: Bind,
    pub line_start: Bind,
    pub line_end: Bind,
    pub edit_input: Bind,
    pub sessions: Bind,
    pub shift_session_down: Bind,
    pub shift_session_up: Bind,
    pub delete_current_session: Bind,
    pub toggle_global_sessions: Bind,
}

impl Default for ConfiguredKeybindings {
    fn default() -> Self {
        Self {
            quit: ctrl_bind!('c', "quit"),
            help: ctrl_bind!('h', "help"),
            prev_chat: ctrl_bind!('p', "prev_chat"),
            next_chat: ctrl_bind!('n', "next_chat"),
            scroll_half_up: ctrl_bind!('u', "scroll_half_up"),
            scroll_half_down: ctrl_bind!('d', "scroll_half_down"),
            scroll_line_up: ctrl_bind!('y', "scroll_line_up"),
            scroll_line_down: ctrl_bind!('e', "scroll_line_down"),
            scroll_top: ctrl_bind!('g', "scroll_top"),
            scroll_bottom: ctrl_bind!('b', "scroll_bottom"),
            pop_queue: ctrl_bind!('q', "pop_queue"),
            delete_word: ctrl_bind!('w', "delete_word"),
            search: ctrl_bind!('f', "search"),
            file_picker: ctrl_bind!('s', "file_picker"),
            toggle_verbose: ctrl_bind!('o', "toggle_verbose"),
            open_editor: Bind {
                code: KeyCode::Char('p'),
                modifiers: KeyModifiers::ALT,
                label: "Alt+P",
                name: Some("open_editor"),
            },
            edit_system_prompt: Bind {
                code: KeyCode::Char('p'),
                modifiers: KeyModifiers::from_bits_truncate(
                    KeyModifiers::ALT.bits() | KeyModifiers::SHIFT.bits(),
                ),
                label: "Alt+Shift+P",
                name: Some("edit_system_prompt"),
            },
            plan_toggle: ctrl_bind!('t', "plan_toggle"),
            tasks: ctrl_bind!('x', "tasks"),
            suspend: ctrl_bind!('z', "suspend"),
            delete: ctrl_bind!('d', "delete"),
            kill_line: ctrl_bind!('k', "kill_line"),
            line_start: ctrl_bind!('a', "line_start"),
            line_end: ctrl_bind!('e', "line_end"),
            edit_input: Bind {
                code: KeyCode::Char('o'),
                modifiers: KeyModifiers::ALT,
                label: "Alt+O",
                name: Some("edit_input"),
            },
            sessions: Bind {
                code: KeyCode::Char('s'),
                modifiers: KeyModifiers::ALT,
                label: "Alt+S",
                name: Some("sessions"),
            },
            shift_session_down: Bind {
                code: KeyCode::Char('a'),
                modifiers: KeyModifiers::from_bits_truncate(
                    KeyModifiers::ALT.bits() | KeyModifiers::SHIFT.bits(),
                ),
                label: "Alt+Shift+A",
                name: Some("shift_session_down"),
            },
            shift_session_up: Bind {
                code: KeyCode::Char('s'),
                modifiers: KeyModifiers::from_bits_truncate(
                    KeyModifiers::ALT.bits() | KeyModifiers::SHIFT.bits(),
                ),
                label: "Alt+Shift+S",
                name: Some("shift_session_up"),
            },
            delete_current_session: Bind {
                code: KeyCode::Char('d'),
                modifiers: KeyModifiers::from_bits_truncate(
                    KeyModifiers::CONTROL.bits() | KeyModifiers::SHIFT.bits(),
                ),
                label: "Ctrl+Shift+D",
                name: Some("delete_current_session"),
            },
            toggle_global_sessions: Bind {
                code: KeyCode::Char('m'),
                modifiers: KeyModifiers::from_bits_truncate(
                    KeyModifiers::CONTROL.bits() | KeyModifiers::SHIFT.bits(),
                ),
                label: "Ctrl+Shift+M",
                name: Some("toggle_global_sessions"),
            },
        }
    }
}

pub static CURRENT_BINDS: std::sync::OnceLock<std::sync::RwLock<ConfiguredKeybindings>> =
    std::sync::OnceLock::new();

pub fn get_configured_bind(name: &str) -> Option<Bind> {
    let binds =
        CURRENT_BINDS.get_or_init(|| std::sync::RwLock::new(ConfiguredKeybindings::default()));
    let read = binds.read().unwrap();
    match name {
        "quit" => Some(read.quit),
        "help" => Some(read.help),
        "prev_chat" => Some(read.prev_chat),
        "next_chat" => Some(read.next_chat),
        "scroll_half_up" => Some(read.scroll_half_up),
        "scroll_half_down" => Some(read.scroll_half_down),
        "scroll_line_up" => Some(read.scroll_line_up),
        "scroll_line_down" => Some(read.scroll_line_down),
        "scroll_top" => Some(read.scroll_top),
        "scroll_bottom" => Some(read.scroll_bottom),
        "pop_queue" => Some(read.pop_queue),
        "delete_word" => Some(read.delete_word),
        "search" => Some(read.search),
        "file_picker" => Some(read.file_picker),
        "toggle_verbose" => Some(read.toggle_verbose),
        "open_editor" => Some(read.open_editor),
        "edit_system_prompt" => Some(read.edit_system_prompt),
        "plan_toggle" => Some(read.plan_toggle),
        "tasks" => Some(read.tasks),
        "suspend" => Some(read.suspend),
        "delete" => Some(read.delete),
        "kill_line" => Some(read.kill_line),
        "line_start" => Some(read.line_start),
        "line_end" => Some(read.line_end),
        "edit_input" => Some(read.edit_input),
        "sessions" => Some(read.sessions),
        "shift_session_down" => Some(read.shift_session_down),
        "shift_session_up" => Some(read.shift_session_up),
        "delete_current_session" => Some(read.delete_current_session),
        "toggle_global_sessions" => Some(read.toggle_global_sessions),
        _ => None,
    }
}

pub fn get_bind_label(name: &str) -> String {
    if let Some(bind) = get_configured_bind(name) {
        bind.label().to_string()
    } else {
        "".to_string()
    }
}

pub fn get_bind<F, R>(f: F) -> R
where
    F: FnOnce(&ConfiguredKeybindings) -> R,
{
    let binds =
        CURRENT_BINDS.get_or_init(|| std::sync::RwLock::new(ConfiguredKeybindings::default()));
    let read = binds.read().unwrap();
    f(&read)
}

pub fn parse_keybind(s: &str) -> Option<Bind> {
    let s = s.trim().to_lowercase();
    let parts: Vec<&str> = s.split('+').collect();
    let mut modifiers = KeyModifiers::empty();
    let mut code = None;

    for part in parts {
        match part {
            "ctrl" | "control" => modifiers.insert(KeyModifiers::CONTROL),
            "alt" | "option" => modifiers.insert(KeyModifiers::ALT),
            "shift" => modifiers.insert(KeyModifiers::SHIFT),
            "super" | "cmd" | "command" | "win" => modifiers.insert(KeyModifiers::SUPER),
            "space" => code = Some(KeyCode::Char(' ')),
            "enter" | "return" => code = Some(KeyCode::Enter),
            "esc" | "escape" => code = Some(KeyCode::Esc),
            "tab" => code = Some(KeyCode::Tab),
            "backspace" => code = Some(KeyCode::Backspace),
            "delete" | "del" => code = Some(KeyCode::Delete),
            "up" => code = Some(KeyCode::Up),
            "down" => code = Some(KeyCode::Down),
            "left" => code = Some(KeyCode::Left),
            "right" => code = Some(KeyCode::Right),
            "home" => code = Some(KeyCode::Home),
            "end" => code = Some(KeyCode::End),
            "pageup" | "pgup" => code = Some(KeyCode::PageUp),
            "pagedown" | "pgdn" => code = Some(KeyCode::PageDown),
            "insert" | "ins" => code = Some(KeyCode::Insert),
            other => {
                if let Some(stripped) = other.strip_prefix('f')
                    && let Ok(n) = stripped.parse::<u8>()
                {
                    code = Some(KeyCode::F(n));
                    continue;
                }
                if other.chars().count() == 1 {
                    code = Some(KeyCode::Char(other.chars().next().unwrap()));
                } else {
                    return None;
                }
            }
        }
    }

    let code = code?;
    let mut label_parts = Vec::new();
    if modifiers.contains(KeyModifiers::CONTROL) {
        label_parts.push("Ctrl");
    }
    if modifiers.contains(KeyModifiers::ALT) {
        label_parts.push("Alt");
    }
    if modifiers.contains(KeyModifiers::SHIFT) {
        label_parts.push("Shift");
    }
    if modifiers.contains(KeyModifiers::SUPER) {
        label_parts.push("Super");
    }

    let key_name = match code {
        KeyCode::Char(' ') => "Space".to_string(),
        KeyCode::Char(c) => c.to_uppercase().to_string(),
        KeyCode::Enter => "Enter".to_string(),
        KeyCode::Esc => "Esc".to_string(),
        KeyCode::Tab => "Tab".to_string(),
        KeyCode::Backspace => "Backspace".to_string(),
        KeyCode::Delete => "Delete".to_string(),
        KeyCode::Up => "Up".to_string(),
        KeyCode::Down => "Down".to_string(),
        KeyCode::Left => "Left".to_string(),
        KeyCode::Right => "Right".to_string(),
        KeyCode::Home => "Home".to_string(),
        KeyCode::End => "End".to_string(),
        KeyCode::PageUp => "PageUp".to_string(),
        KeyCode::PageDown => "PageDown".to_string(),
        KeyCode::Insert => "Insert".to_string(),
        KeyCode::F(n) => format!("F{}", n),
        _ => "".to_string(),
    };
    label_parts.push(&key_name);
    let label = label_parts.join("+");

    Some(Bind {
        code,
        modifiers,
        label: Box::leak(label.into_boxed_str()),
        name: None,
    })
}

pub fn update_bind(action: &str, bind_str: &str) -> bool {
    if let Some(mut bind) = parse_keybind(bind_str) {
        let binds =
            CURRENT_BINDS.get_or_init(|| std::sync::RwLock::new(ConfiguredKeybindings::default()));
        if let Ok(mut write) = binds.write() {
            bind.name = Some(Box::leak(action.to_string().into_boxed_str()));
            match action {
                "quit" => write.quit = bind,
                "help" => write.help = bind,
                "prev_chat" => write.prev_chat = bind,
                "next_chat" => write.next_chat = bind,
                "scroll_half_up" => write.scroll_half_up = bind,
                "scroll_half_down" => write.scroll_half_down = bind,
                "scroll_line_up" => write.scroll_line_up = bind,
                "scroll_line_down" => write.scroll_line_down = bind,
                "scroll_top" => write.scroll_top = bind,
                "scroll_bottom" => write.scroll_bottom = bind,
                "pop_queue" => write.pop_queue = bind,
                "delete_word" => write.delete_word = bind,
                "search" => write.search = bind,
                "file_picker" => write.file_picker = bind,
                "toggle_verbose" => write.toggle_verbose = bind,
                "open_editor" => write.open_editor = bind,
                "edit_system_prompt" => write.edit_system_prompt = bind,
                "plan_toggle" => write.plan_toggle = bind,
                "tasks" => write.tasks = bind,
                "suspend" => write.suspend = bind,
                "delete" => write.delete = bind,
                "kill_line" => write.kill_line = bind,
                "line_start" => write.line_start = bind,
                "line_end" => write.line_end = bind,
                "edit_input" => write.edit_input = bind,
                "sessions" => write.sessions = bind,
                "shift_session_down" => write.shift_session_down = bind,
                "shift_session_up" => write.shift_session_up = bind,
                _ => return false,
            }
            return true;
        }
    }
    false
}

pub mod key {
    use super::Bind;
    use crossterm::event::{KeyCode, KeyModifiers};

    pub const QUIT: Bind = ctrl_bind!('c', "quit");
    pub const HELP: Bind = ctrl_bind!('h', "help");
    pub const PREV_CHAT: Bind = ctrl_bind!('p', "prev_chat");
    pub const NEXT_CHAT: Bind = ctrl_bind!('n', "next_chat");
    pub const SCROLL_HALF_UP: Bind = ctrl_bind!('u', "scroll_half_up");
    pub const SCROLL_HALF_DOWN: Bind = ctrl_bind!('d', "scroll_half_down");
    pub const SCROLL_LINE_UP: Bind = ctrl_bind!('y', "scroll_line_up");
    pub const SCROLL_LINE_DOWN: Bind = ctrl_bind!('e', "scroll_line_down");
    pub const SCROLL_TOP: Bind = ctrl_bind!('g', "scroll_top");
    pub const SCROLL_BOTTOM: Bind = ctrl_bind!('b', "scroll_bottom");
    pub const POP_QUEUE: Bind = ctrl_bind!('q', "pop_queue");
    pub const DELETE_WORD: Bind = ctrl_bind!('w', "delete_word");
    pub const SEARCH: Bind = ctrl_bind!('f', "search");
    pub const FILE_PICKER: Bind = ctrl_bind!('s', "file_picker");
    pub const TOGGLE_VERBOSE: Bind = ctrl_bind!('o', "toggle_verbose");
    pub const OPEN_EDITOR: Bind = Bind {
        code: KeyCode::Char('p'),
        modifiers: KeyModifiers::ALT,
        label: "Alt+P",
        name: Some("open_editor"),
    };
    pub const EDIT_SYSTEM_PROMPT: Bind = Bind {
        code: KeyCode::Char('p'),
        modifiers: KeyModifiers::from_bits_truncate(
            KeyModifiers::ALT.bits() | KeyModifiers::SHIFT.bits(),
        ),
        label: "Alt+Shift+P",
        name: Some("edit_system_prompt"),
    };
    pub const PLAN_TOGGLE: Bind = ctrl_bind!('t', "plan_toggle");
    pub const TASKS: Bind = ctrl_bind!('x', "tasks");
    pub const REFRESH: Bind = ctrl_bind!('r', "refresh");
    pub const SUSPEND: Bind = ctrl_bind!('z', "suspend");
    pub const DELETE: Bind = ctrl_bind!('d', "delete");
    pub const KILL_LINE: Bind = ctrl_bind!('k', "kill_line");
    pub const LINE_START: Bind = ctrl_bind!('a', "line_start");
    pub const LINE_END: Bind = ctrl_bind!('e', "line_end");
    pub const EDIT_INPUT: Bind = Bind {
        code: KeyCode::Char('o'),
        modifiers: KeyModifiers::ALT,
        label: "Alt+O",
        name: Some("edit_input"),
    };
    pub const SESSIONS: Bind = Bind {
        code: KeyCode::Char('s'),
        modifiers: KeyModifiers::ALT,
        label: "Alt+S",
        name: Some("sessions"),
    };
    pub const SHIFT_SESSION_DOWN: Bind = Bind {
        code: KeyCode::Char('a'),
        modifiers: KeyModifiers::from_bits_truncate(
            KeyModifiers::ALT.bits() | KeyModifiers::SHIFT.bits(),
        ),
        label: "Alt+Shift+A",
        name: Some("shift_session_down"),
    };
    pub const SHIFT_SESSION_UP: Bind = Bind {
        code: KeyCode::Char('s'),
        modifiers: KeyModifiers::from_bits_truncate(
            KeyModifiers::ALT.bits() | KeyModifiers::SHIFT.bits(),
        ),
        label: "Alt+Shift+S",
        name: Some("shift_session_up"),
    };
    pub const DELETE_CURRENT_SESSION: Bind = Bind {
        code: KeyCode::Char('d'),
        modifiers: KeyModifiers::from_bits_truncate(
            KeyModifiers::CONTROL.bits() | KeyModifiers::SHIFT.bits(),
        ),
        label: "Ctrl+Shift+D",
        name: Some("delete_current_session"),
    };
    pub const TOGGLE_GLOBAL_SESSIONS: Bind = Bind {
        code: KeyCode::Char('m'),
        modifiers: KeyModifiers::from_bits_truncate(
            KeyModifiers::CONTROL.bits() | KeyModifiers::SHIFT.bits(),
        ),
        label: "Ctrl+Shift+M",
        name: Some("toggle_global_sessions"),
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, EnumIter)]
pub enum KeybindContext {
    General,
    Editing,
    Streaming,
    Picker,
    FormInput,
    TaskPicker,
    RewindPicker,
    GotoPicker,
    ThemePicker,
    SettingsPicker,
    ModelPicker,
    QueueFocus,
    CommandPalette,
    Search,
    FilePicker,
}

impl KeybindContext {
    pub const fn label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Editing => "Editing",
            Self::Streaming => "While Streaming",
            Self::Picker => "Pickers",
            Self::FormInput => "Form",
            Self::TaskPicker => "Task Picker",
            Self::RewindPicker => "Rewind Picker",
            Self::GotoPicker => "Goto Picker",
            Self::ThemePicker => "Theme Picker",
            Self::SettingsPicker => "Settings Picker",
            Self::ModelPicker => "Model Picker",
            Self::QueueFocus => "Queue",
            Self::CommandPalette => "Commands",
            Self::Search => "Search",
            Self::FilePicker => "File Picker",
        }
    }

    pub const fn parent(self) -> Option<KeybindContext> {
        match self {
            Self::TaskPicker
            | Self::RewindPicker
            | Self::GotoPicker
            | Self::ThemePicker
            | Self::SettingsPicker
            | Self::ModelPicker
            | Self::QueueFocus
            | Self::CommandPalette
            | Self::Search
            | Self::FilePicker => Some(Self::Picker),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    All,
    MacOnly,
    UnixOnly,
}

impl Platform {
    pub const fn is_visible(self) -> bool {
        match self {
            Self::All => true,
            Self::MacOnly => cfg!(target_os = "macos"),
            Self::UnixOnly => cfg!(unix),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum KeyLabel {
    Single(&'static str),
    Alt(&'static str, &'static str),
    /// Alt on Mac, Single (first) on other platforms
    MacAlt(&'static str, &'static str),
    /// Multi on Mac, Multi (first slice) on other platforms
    MacMulti(&'static [&'static str], &'static [&'static str]),
    Action(&'static str),
    ActionAlt(&'static str, &'static str),
    ActionMacAlt(&'static str, &'static str),
}

pub const ALT_SEP: &str = " / ";

#[derive(Debug, Clone)]
pub enum ResolvedLabel {
    Single(String),
    Alt(String, String),
    Multi(Vec<String>),
}

impl ResolvedLabel {
    pub fn display_width(&self) -> usize {
        match self {
            Self::Single(s) => UnicodeWidthStr::width(s.as_str()),
            Self::Alt(a, b) => {
                let sep_w = UnicodeWidthStr::width(ALT_SEP);
                UnicodeWidthStr::width(a.as_str()) + sep_w + UnicodeWidthStr::width(b.as_str())
            }
            Self::Multi(keys) => {
                let sep_w = UnicodeWidthStr::width(ALT_SEP);
                keys.iter()
                    .map(|k| UnicodeWidthStr::width(k.as_str()))
                    .sum::<usize>()
                    + sep_w * keys.len().saturating_sub(1)
            }
        }
    }
}

impl KeyLabel {
    pub fn resolve(self) -> ResolvedLabel {
        match self {
            Self::Single(s) => ResolvedLabel::Single(s.to_string()),
            Self::Alt(a, b) => ResolvedLabel::Alt(a.to_string(), b.to_string()),
            Self::MacAlt(a, b) => {
                if cfg!(target_os = "macos") {
                    ResolvedLabel::Alt(a.to_string(), b.to_string())
                } else {
                    ResolvedLabel::Single(a.to_string())
                }
            }
            Self::MacMulti(normal, mac) => {
                if cfg!(target_os = "macos") {
                    ResolvedLabel::Multi(mac.iter().map(|s| s.to_string()).collect())
                } else {
                    ResolvedLabel::Multi(normal.iter().map(|s| s.to_string()).collect())
                }
            }
            Self::Action(name) => ResolvedLabel::Single(get_bind_label(name)),
            Self::ActionAlt(name1, name2) => {
                ResolvedLabel::Alt(get_bind_label(name1), get_bind_label(name2))
            }
            Self::ActionMacAlt(name, mac_label) => {
                if cfg!(target_os = "macos") {
                    ResolvedLabel::Alt(get_bind_label(name), mac_label.to_string())
                } else {
                    ResolvedLabel::Single(get_bind_label(name))
                }
            }
        }
    }

    #[cfg(test)]
    fn flat_str(&self) -> String {
        match self.resolve() {
            ResolvedLabel::Single(s) => s,
            ResolvedLabel::Alt(a, b) => format!("{a}/{b}"),
            ResolvedLabel::Multi(keys) => keys.join("/"),
        }
    }
}

pub struct Keybind {
    pub label: KeyLabel,
    pub description: &'static str,
    pub context: KeybindContext,
    pub platform: Platform,
}

pub const KEYBINDS: &[Keybind] = &[
    Keybind {
        label: KeyLabel::Action("quit"),
        description: "Quit / clear input",
        context: KeybindContext::General,
        platform: Platform::All,
    },
    Keybind {
        label: KeyLabel::Action("help"),
        description: "Show keybindings",
        context: KeybindContext::General,
        platform: Platform::All,
    },
    Keybind {
        label: KeyLabel::ActionAlt("next_chat", "prev_chat"),
        description: "Next / previous task chat",
        context: KeybindContext::General,
        platform: Platform::All,
    },
    Keybind {
        label: KeyLabel::Action("search"),
        description: "Search messages",
        context: KeybindContext::General,
        platform: Platform::All,
    },
    Keybind {
        label: KeyLabel::Action("file_picker"),
        description: "File picker",
        context: KeybindContext::General,
        platform: Platform::All,
    },
    Keybind {
        label: KeyLabel::Action("toggle_verbose"),
        description: "Toggle verbose mode",
        context: KeybindContext::General,
        platform: Platform::All,
    },
    Keybind {
        label: KeyLabel::Action("sessions"),
        description: "Open sessions list",
        context: KeybindContext::General,
        platform: Platform::All,
    },
    Keybind {
        label: KeyLabel::Action("shift_session_down"),
        description: "Switch to next session",
        context: KeybindContext::General,
        platform: Platform::All,
    },
    Keybind {
        label: KeyLabel::Action("shift_session_up"),
        description: "Switch to previous session",
        context: KeybindContext::General,
        platform: Platform::All,
    },
    Keybind {
        label: KeyLabel::Action("open_editor"),
        description: "Open plan in editor",
        context: KeybindContext::General,
        platform: Platform::All,
    },
    Keybind {
        label: KeyLabel::Action("plan_toggle"),
        description: "Toggle plan panel",
        context: KeybindContext::General,
        platform: Platform::All,
    },
    Keybind {
        label: KeyLabel::Action("tasks"),
        description: "Open tasks",
        context: KeybindContext::General,
        platform: Platform::All,
    },
    Keybind {
        label: KeyLabel::Action("suspend"),
        description: "Suspend process",
        context: KeybindContext::General,
        platform: Platform::UnixOnly,
    },
    Keybind {
        label: KeyLabel::Single("Enter"),
        description: "Submit prompt",
        context: KeybindContext::Editing,
        platform: Platform::All,
    },
    Keybind {
        label: KeyLabel::MacMulti(
            &["Shift+Enter", "Ctrl+Enter", "Ctrl+J", "Alt+Enter"],
            &["⇧↵", "⌃↵", "⌃J", "⌥↵"],
        ),
        description: "Newline",
        context: KeybindContext::Editing,
        platform: Platform::All,
    },
    Keybind {
        label: KeyLabel::Single("Tab"),
        description: "Toggle mode",
        context: KeybindContext::Editing,
        platform: Platform::All,
    },
    Keybind {
        label: KeyLabel::Single("/command"),
        description: "Open command palette",
        context: KeybindContext::Editing,
        platform: Platform::All,
    },
    Keybind {
        label: KeyLabel::ActionMacAlt("delete_word", "⌥⌫"),
        description: "Delete word backward",
        context: KeybindContext::Editing,
        platform: Platform::All,
    },
    Keybind {
        label: KeyLabel::MacMulti(&["Alt+←", "Alt+→"], &["⌥←", "⌥→"]),
        description: "Move word left / right",
        context: KeybindContext::Editing,
        platform: Platform::All,
    },
    Keybind {
        label: KeyLabel::Alt(mod_key!("Del"), "⌥Del"),
        description: "Delete word forward",
        context: KeybindContext::Editing,
        platform: Platform::MacOnly,
    },
    Keybind {
        label: KeyLabel::Action("kill_line"),
        description: "Delete to end of line",
        context: KeybindContext::Editing,
        platform: Platform::MacOnly,
    },
    Keybind {
        label: KeyLabel::Action("line_start"),
        description: "Jump to start of line",
        context: KeybindContext::Editing,
        platform: Platform::All,
    },
    Keybind {
        label: KeyLabel::Alt("Home", "End"),
        description: "Jump to start/end of line",
        context: KeybindContext::Editing,
        platform: Platform::All,
    },
    Keybind {
        label: KeyLabel::ActionAlt("scroll_half_up", "scroll_half_down"),
        description: "Scroll half page up / down",
        context: KeybindContext::Editing,
        platform: Platform::All,
    },
    Keybind {
        label: KeyLabel::Action("line_end"),
        description: "Jump to end of line",
        context: KeybindContext::Editing,
        platform: Platform::All,
    },
    Keybind {
        label: KeyLabel::Action("scroll_top"),
        description: "Scroll to top",
        context: KeybindContext::Editing,
        platform: Platform::All,
    },
    Keybind {
        label: KeyLabel::Action("scroll_bottom"),
        description: "Scroll to bottom",
        context: KeybindContext::Editing,
        platform: Platform::All,
    },
    Keybind {
        label: KeyLabel::Action("pop_queue"),
        description: "Pop queue",
        context: KeybindContext::Editing,
        platform: Platform::All,
    },
    Keybind {
        label: KeyLabel::Single("Esc Esc"),
        description: "Rewind",
        context: KeybindContext::Editing,
        platform: Platform::All,
    },
    Keybind {
        label: KeyLabel::Action("edit_input"),
        description: "Edit input in external editor",
        context: KeybindContext::Editing,
        platform: Platform::All,
    },
    Keybind {
        label: KeyLabel::Alt("↑", "↓"),
        description: "Navigate input history",
        context: KeybindContext::Streaming,
        platform: Platform::All,
    },
    Keybind {
        label: KeyLabel::Single("Esc Esc"),
        description: "Cancel agent",
        context: KeybindContext::Streaming,
        platform: Platform::All,
    },
    Keybind {
        label: KeyLabel::Alt("↑", "↓"),
        description: "Navigate options",
        context: KeybindContext::FormInput,
        platform: Platform::All,
    },
    Keybind {
        label: KeyLabel::Single("Enter"),
        description: "Select option",
        context: KeybindContext::FormInput,
        platform: Platform::All,
    },
    Keybind {
        label: KeyLabel::Single("Esc"),
        description: "Close",
        context: KeybindContext::FormInput,
        platform: Platform::All,
    },
    Keybind {
        label: KeyLabel::Alt("↑", "↓"),
        description: "Navigate",
        context: KeybindContext::Picker,
        platform: Platform::All,
    },
    Keybind {
        label: KeyLabel::Single("Enter"),
        description: "Select",
        context: KeybindContext::Picker,
        platform: Platform::All,
    },
    Keybind {
        label: KeyLabel::Single("Esc"),
        description: "Close",
        context: KeybindContext::Picker,
        platform: Platform::All,
    },
    Keybind {
        label: KeyLabel::Single("Type"),
        description: "Filter",
        context: KeybindContext::Picker,
        platform: Platform::All,
    },
    Keybind {
        label: KeyLabel::Alt("PageUp", "PageDown"),
        description: "Scroll page up / down",
        context: KeybindContext::Picker,
        platform: Platform::All,
    },
    Keybind {
        label: KeyLabel::Alt(key::SCROLL_HALF_UP.label, key::SCROLL_HALF_DOWN.label),
        description: "Scroll page up / down",
        context: KeybindContext::Picker,
        platform: Platform::All,
    },
    Keybind {
        label: KeyLabel::Single("Enter"),
        description: "Remove item",
        context: KeybindContext::QueueFocus,
        platform: Platform::All,
    },
    Keybind {
        label: KeyLabel::Single("Tab"),
        description: "Complete command",
        context: KeybindContext::CommandPalette,
        platform: Platform::All,
    },
    Keybind {
        label: KeyLabel::Single("!/@/#/$"),
        description: "Set tier (strong/medium/weak/compaction)",
        context: KeybindContext::ModelPicker,
        platform: Platform::All,
    },
];

pub fn all_contexts() -> impl Iterator<Item = KeybindContext> {
    use strum::IntoEnumIterator;
    KeybindContext::iter()
}

pub(crate) fn key_event_to_string(key: &KeyEvent) -> String {
    let mut s = String::new();
    let mods = key.modifiers;
    let is_char = matches!(key.code, KeyCode::Char(_));
    if mods.contains(KeyModifiers::CONTROL) {
        s.push_str("ctrl+");
    }
    if mods.contains(KeyModifiers::ALT) {
        s.push_str("alt+");
    }
    if mods.contains(KeyModifiers::SHIFT)
        && (!is_char
            || mods.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER))
    {
        s.push_str("shift+");
    }
    match key.code {
        KeyCode::Char(' ') => s.push_str("space"),
        KeyCode::Char(c) => s.push(c),
        KeyCode::Enter => s.push_str("enter"),
        KeyCode::Esc => s.push_str("esc"),
        KeyCode::Tab => s.push_str("tab"),
        KeyCode::BackTab => {
            if !s.contains("shift+") {
                s.insert_str(0, "shift+");
            }
            s.push_str("tab");
        }
        KeyCode::Backspace => s.push_str("backspace"),
        KeyCode::Delete => s.push_str("delete"),
        KeyCode::Up => s.push_str("up"),
        KeyCode::Down => s.push_str("down"),
        KeyCode::Left => s.push_str("left"),
        KeyCode::Right => s.push_str("right"),
        KeyCode::Home => s.push_str("home"),
        KeyCode::End => s.push_str("end"),
        KeyCode::PageUp => s.push_str("pageup"),
        KeyCode::PageDown => s.push_str("pagedown"),
        KeyCode::F(n) => write!(s, "f{n}").unwrap(),
        KeyCode::Insert => s.push_str("insert"),
        _ => {}
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEvent;
    use test_case::test_case;

    #[test_case(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL), "ctrl+d")]
    #[test_case(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::ALT), "alt+x")]
    #[test_case(KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT), "shift+tab")]
    #[test_case(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT), "shift+tab")]
    #[test_case(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE), "space")]
    #[test_case(KeyEvent::new(KeyCode::F(5), KeyModifiers::NONE), "f5")]
    #[test_case(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE), "a")]
    fn key_event_to_string_cases(input: KeyEvent, expected: &str) {
        assert_eq!(key_event_to_string(&input), expected);
    }

    #[test]
    fn bind_requires_exact_modifiers() {
        let bind = key::TOGGLE_VERBOSE; // Ctrl+O
        let exact = KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL);
        let extra = KeyEvent::new(
            KeyCode::Char('o'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        );
        let wrong = KeyEvent::new(KeyCode::Char('o'), KeyModifiers::ALT);

        assert!(bind.matches(exact));
        assert!(!bind.matches(extra), "extra modifiers should not match");
        assert!(!bind.matches(wrong), "wrong modifier should not match");
    }

    #[test]
    fn every_context_has_at_least_one_keybind() {
        for ctx in all_contexts() {
            let has_own = KEYBINDS.iter().any(|kb| kb.context == ctx);
            let has_parent = ctx
                .parent()
                .is_some_and(|p| KEYBINDS.iter().any(|kb| kb.context == p));
            assert!(
                has_own || has_parent,
                "context {:?} has no keybinds and no parent with keybinds",
                ctx,
            );
        }
    }

    #[test]
    fn no_duplicate_entries() {
        for (i, a) in KEYBINDS.iter().enumerate() {
            for (j, b) in KEYBINDS.iter().enumerate() {
                if i != j && a.context == b.context {
                    assert!(
                        a.label.flat_str() != b.label.flat_str() || a.description != b.description,
                        "duplicate keybind: {} - {} in {:?}",
                        a.label.flat_str(),
                        a.description,
                        a.context,
                    );
                }
            }
        }
    }
}
