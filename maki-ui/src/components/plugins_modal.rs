use crate::components::Overlay;
use crate::components::modal::Modal;
use crate::theme;
use crossterm::event::{KeyCode, KeyEvent};
use maki_agent::tools::{ToolRegistry, ToolSource};
use maki_lua::EventHandle;
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct PluginInfo {
    pub name: String,
    pub source_path: PathBuf,
    pub is_loaded: bool,
}

pub enum PluginsAction {
    None,
    EditPlugin(PathBuf),
}

pub struct PluginsModal {
    open: bool,
    plugins: Vec<PluginInfo>,
    selected: usize,
    event_handle: Option<EventHandle>,
}

impl PluginsModal {
    pub fn new() -> Self {
        Self {
            open: false,
            plugins: Vec::new(),
            selected: 0,
            event_handle: None,
        }
    }

    pub fn open(&mut self, event_handle: &EventHandle) {
        self.open = true;
        self.event_handle = Some(event_handle.clone());
        self.selected = 0;
        self.refresh();
    }

    fn refresh(&mut self) {
        self.plugins = build_plugin_list();
        self.selected = self.selected.min(self.plugins.len().saturating_sub(1));
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> PluginsAction {
        if !self.open {
            return PluginsAction::None;
        }
        match key.code {
            KeyCode::Esc => {
                self.close();
                PluginsAction::None
            }
            KeyCode::Up => {
                if !self.plugins.is_empty() {
                    self.selected = if self.selected == 0 {
                        self.plugins.len() - 1
                    } else {
                        self.selected - 1
                    };
                }
                PluginsAction::None
            }
            KeyCode::Down => {
                if !self.plugins.is_empty() {
                    self.selected = (self.selected + 1) % self.plugins.len();
                }
                PluginsAction::None
            }
            KeyCode::Char(' ') | KeyCode::Enter => {
                if let Some(plugin) = self.plugins.get(self.selected).cloned() {
                    self.toggle(&plugin.name, plugin.is_loaded);
                    self.refresh();
                }
                PluginsAction::None
            }
            KeyCode::Char('e') => {
                if let Some(plugin) = self.plugins.get(self.selected) {
                    let path = plugin.source_path.clone();
                    self.close();
                    return PluginsAction::EditPlugin(path);
                }
                PluginsAction::None
            }
            _ => PluginsAction::None,
        }
    }

    fn toggle(&self, name: &str, currently_loaded: bool) {
        let mut settings = crate::components::settings_picker::UserSettings::load();
        if let Some(handle) = &self.event_handle {
            if currently_loaded {
                handle.unload_plugin(name);
                if !settings.disabled_plugins.contains(&name.to_string()) {
                    settings.disabled_plugins.push(name.to_string());
                }
            } else {
                handle.load_builtin(name);
                settings.disabled_plugins.retain(|p| p != name);
            }
        }
        settings.save();
    }

    pub fn view(&mut self, frame: &mut Frame, area: Rect) -> Rect {
        if !self.open {
            return Rect::default();
        }

        let content_height = (self.plugins.len() as u16 + 4).clamp(12, 30);
        let modal = Modal {
            title: " Plugins ",
            width_percent: 80,
            max_height_percent: 80,
        };
        let (popup, inner) = modal.render(frame, area, content_height);

        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(48), Constraint::Percentage(52)])
            .split(inner);

        self.render_list(frame, chunks[0]);
        self.render_detail(frame, chunks[1]);

        popup
    }

    fn render_list(&self, frame: &mut Frame, area: Rect) {
        let t = theme::current();
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(t.panel_border)
            .title(Span::styled(" Plugins ", t.panel_border));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let height = inner.height as usize;
        let total = self.plugins.len();
        let scroll = if self.selected >= height {
            self.selected - height + 1
        } else {
            0
        };

        let mut lines = Vec::new();
        for (i, plugin) in self.plugins.iter().enumerate().skip(scroll).take(height) {
            let check = if plugin.is_loaded { "[x]" } else { "[ ]" };
            let label = format!("{} {}", check, plugin.name);
            let style = if i == self.selected {
                t.item_selected.add_modifier(Modifier::BOLD)
            } else if plugin.is_loaded {
                t.item
            } else {
                t.tool_dim
            };
            lines.push(Line::from(Span::styled(label, style)));
        }

        if total == 0 {
            lines.push(Line::from(Span::styled("No plugins found", t.tool_dim)));
        }

        let para = Paragraph::new(lines);
        frame.render_widget(para, inner);
    }

    fn render_detail(&self, frame: &mut Frame, area: Rect) {
        let t = theme::current();
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(t.panel_border)
            .title(Span::styled(" Detail ", t.panel_border));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let mut lines: Vec<Line> = Vec::new();

        if let Some(plugin) = self.plugins.get(self.selected) {
            let status = if plugin.is_loaded {
                "enabled"
            } else {
                "disabled"
            };
            let status_style = if plugin.is_loaded { t.item } else { t.tool_dim };

            lines.push(Line::from(vec![
                Span::styled("Plugin: ", t.tool_dim),
                Span::styled(plugin.name.clone(), t.item.add_modifier(Modifier::BOLD)),
            ]));
            lines.push(Line::from(vec![
                Span::styled("Status: ", t.tool_dim),
                Span::styled(status, status_style),
            ]));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled("Source:", t.tool_dim)));
            let path_str = plugin.source_path.to_string_lossy().into_owned();
            // wrap long paths
            for chunk in path_str
                .as_bytes()
                .chunks(inner.width.saturating_sub(2) as usize)
                .map(|b| std::str::from_utf8(b).unwrap_or(""))
            {
                lines.push(Line::from(Span::styled(chunk.to_string(), t.item_desc)));
            }
            lines.push(Line::from(""));
        } else {
            lines.push(Line::from(Span::styled("No plugin selected", t.tool_dim)));
        }

        // Key hints
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "── Keys ──────────────",
            t.tool_dim,
        )));
        for (key, desc) in &[
            ("Space/Enter", "toggle enable/disable"),
            ("e", "open source in editor"),
            ("↑/↓", "navigate"),
            ("Esc", "close"),
        ] {
            lines.push(Line::from(vec![
                Span::styled(format!("{:<12}", key), t.item.add_modifier(Modifier::BOLD)),
                Span::styled(desc.to_string(), t.tool_dim),
            ]));
        }

        let para = Paragraph::new(lines).wrap(Wrap { trim: false });
        frame.render_widget(para, inner);
    }
}

impl Overlay for PluginsModal {
    fn is_open(&self) -> bool {
        self.open
    }
    fn close(&mut self) {
        self.open = false;
        self.plugins.clear();
        self.event_handle = None;
    }
}

fn build_plugin_list() -> Vec<PluginInfo> {
    let registry = ToolRegistry::global();
    let snapshot = registry.iter();
    let loaded_plugins: std::collections::HashSet<String> = snapshot
        .iter()
        .filter_map(|t| {
            if let ToolSource::Lua { plugin } = &t.source {
                Some(plugin.to_string())
            } else {
                None
            }
        })
        .collect();

    maki_lua::bundled_plugins()
        .filter(|(name, _)| *name != "lib") // lib is internal, not user-facing
        .map(|(name, source_path)| PluginInfo {
            name: name.to_string(),
            source_path: PathBuf::from(source_path),
            is_loaded: loaded_plugins.contains(name),
        })
        .collect()
}
