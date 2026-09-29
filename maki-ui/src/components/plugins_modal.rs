use crate::components::Overlay;
use crate::components::modal::Modal;
use crate::theme;
use crossterm::event::{KeyCode, KeyEvent};
use maki_lua::{EventHandle, LoadedPlugins};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use std::path::PathBuf;
use std::sync::Arc;

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
    /// Generation of the snapshot `plugins` was built from, so a load that
    /// finishes while the modal is up redraws without a keypress.
    generation: u64,
}

impl PluginsModal {
    pub fn new() -> Self {
        Self {
            open: false,
            plugins: Vec::new(),
            selected: 0,
            event_handle: None,
            generation: 0,
        }
    }

    pub fn open(&mut self, event_handle: &EventHandle) {
        self.open = true;
        self.event_handle = Some(event_handle.clone());
        self.selected = 0;
        self.refresh();
    }

    fn loaded(&self) -> Arc<LoadedPlugins> {
        self.event_handle
            .as_ref()
            .map(|handle| handle.loaded_plugins().load_full())
            .unwrap_or_else(|| Arc::new(LoadedPlugins::default()))
    }

    fn refresh(&mut self) {
        let loaded = self.loaded();
        self.generation = loaded.generation();
        self.plugins = build_plugin_list(&loaded);
        self.selected = self.selected.min(self.plugins.len().saturating_sub(1));
    }

    /// Rebuilds when the runtime published a new set while the modal is up:
    /// a toggle answers asynchronously, so the row it changed has to catch up
    /// without a keypress.
    fn sync_generation(&mut self) {
        if self.loaded().generation() != self.generation {
            self.refresh();
        }
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
                }
                PluginsAction::None
            }
            KeyCode::Char('e') => {
                if let Some(plugin) = self.plugins.get(self.selected)
                    && let Some(path) = plugin_source_path(&plugin.name)
                {
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
                settings.enabled_plugins.retain(|plugin| plugin != name);
            } else {
                handle.load_builtin(name);
                settings.disabled_plugins.retain(|plugin| plugin != name);
                // A default builtin loads again on the next start without a
                // note; an opt-in one needs the record to come back at all.
                if !maki_config::DEFAULT_BUILTINS.contains(&name)
                    && !settings.enabled_plugins.contains(&name.to_string())
                {
                    settings.enabled_plugins.push(name.to_string());
                }
            }
        }
        settings.save();
    }

    pub fn view(&mut self, frame: &mut Frame, area: Rect) -> Rect {
        if !self.open {
            return Rect::default();
        }
        self.sync_generation();

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

/// Where a bundled plugin's source lives, for "open the source" flows. The
/// display path `bundled_plugins()` reports is the plugin directory, which is
/// not a file an editor can open.
pub(crate) fn plugin_source_path(name: &str) -> Option<PathBuf> {
    maki_lua::bundled_plugin_entry_file(name)
}

fn build_plugin_list(loaded: &LoadedPlugins) -> Vec<PluginInfo> {
    maki_lua::bundled_plugins()
        .filter(|(name, _)| *name != "lib") // lib is internal, not user-facing
        .map(|(name, source_path)| PluginInfo {
            name: name.to_string(),
            source_path: PathBuf::from(source_path),
            is_loaded: loaded.contains(name),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use maki_lua::test_support::loaded_plugins_pair;

    fn is_loaded(modal: &PluginsModal, name: &str) -> bool {
        modal
            .plugins
            .iter()
            .find(|plugin| plugin.name == name)
            .unwrap_or_else(|| panic!("{name} should be listed"))
            .is_loaded
    }

    /// The registry never sees a plugin without tools, so the list has to read
    /// the runtime's own set to tell `/plugins` the truth.
    #[test]
    fn rows_follow_the_runtime_state_not_the_tool_registry() {
        let (writer, reader) = loaded_plugins_pair();
        writer.publish(&["sessions", "thinking", "status"]);
        let mut modal = PluginsModal::new();
        modal.open(&EventHandle::disconnected_for_test().with_loaded_reader(reader));

        assert!(is_loaded(&modal, "sessions"), "command-only plugin is on");
        assert!(is_loaded(&modal, "thinking"), "picker plugin is on");
        assert!(is_loaded(&modal, "status"), "autocmd-only plugin is on");
        assert!(!is_loaded(&modal, "cronjob"), "opt-in builtin is off");
        assert!(
            modal.plugins.iter().all(|plugin| plugin.name != "lib"),
            "lib stays internal"
        );
    }

    #[test]
    fn a_publish_while_open_rebuilds_the_list() {
        let (writer, reader) = loaded_plugins_pair();
        writer.publish(&["sessions"]);
        let mut modal = PluginsModal::new();
        modal.open(&EventHandle::disconnected_for_test().with_loaded_reader(reader));
        assert!(
            !is_loaded(&modal, "cronjob"),
            "off until the runtime says so"
        );

        writer.publish(&["sessions", "cronjob"]);
        modal.sync_generation();
        assert!(is_loaded(&modal, "cronjob"), "picked up without a keypress");
    }
}
