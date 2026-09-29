use crate::components::Overlay;
use crate::components::modal::Modal;
use crate::theme;
use crossterm::event::{KeyCode, KeyEvent};
use maki_lua::{EventHandle, LoadedPlugins, PluginToolInfo};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct PluginInfo {
    pub name: String,
    pub source_path: PathBuf,
    pub is_loaded: bool,
    pub tools: Vec<PluginToolInfo>,
}

pub enum PluginsAction {
    None,
    EditPlugin(PathBuf),
}

const KEY_HINTS: &str = " ↑/↓ navigate · Space toggle · e edit · Esc close ";

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

        let t = theme::current();
        let footer = Rect {
            x: popup.x + 1,
            y: popup.y + popup.height - 1,
            width: popup.width.saturating_sub(2),
            height: 1,
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(KEY_HINTS, t.tool_dim)).right_aligned()),
            footer,
        );

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
            for tool in &plugin.tools {
                lines.push(Line::from(vec![
                    Span::styled("Tool: ", t.tool_dim),
                    Span::styled(tool.name.to_string(), t.item.add_modifier(Modifier::BOLD)),
                ]));
                lines.push(Line::from(Span::styled(tool.description.clone(), t.item_desc)));
                push_input_schema(&mut lines, &tool.schema, &t);
                lines.push(Line::from(""));
            }
            if plugin.is_loaded && plugin.tools.is_empty() {
                lines.push(Line::from(Span::styled(
                    "This plugin registers no tools",
                    t.tool_dim,
                )));
                lines.push(Line::from(""));
            }
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
        } else {
            lines.push(Line::from(Span::styled("No plugin selected", t.tool_dim)));
        }

        let para = Paragraph::new(lines).wrap(Wrap { trim: false });
        frame.render_widget(para, inner);
    }
}

fn push_input_schema(lines: &mut Vec<Line>, schema: &Value, t: &theme::Theme) {
    let Some(props) = schema.get("properties").and_then(Value::as_object) else {
        return;
    };
    if props.is_empty() {
        return;
    }
    let required: Vec<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|names| names.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();

    lines.push(Line::from(Span::styled("Input:", t.tool_dim)));
    for (name, prop) in props {
        let kind = prop.get("type").and_then(Value::as_str).unwrap_or("any");
        let mut spans = vec![
            Span::styled(format!("  {name}"), t.item),
            Span::styled(format!(" {kind}"), t.tool_dim),
        ];
        if required.contains(&name.as_str()) {
            spans.push(Span::styled(" (required)", t.tool_dim));
        }
        lines.push(Line::from(spans));
        if let Some(desc) = prop.get("description").and_then(Value::as_str)
            && !desc.is_empty()
        {
            lines.push(Line::from(Span::styled(
                format!("    {desc}"),
                t.item_desc,
            )));
        }
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
        .map(|(name, source_path)| {
            let entry = loaded.plugin(name);
            PluginInfo {
                name: name.to_string(),
                source_path: PathBuf::from(source_path),
                is_loaded: entry.is_some(),
                tools: entry.map(|p| p.tools.clone()).unwrap_or_default(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use maki_lua::LoadedPlugin;
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

    const TOOL_DESCRIPTION: &str = "Read a file from disk.";
    const PATH_DESCRIPTION: &str = "Absolute path to the file";

    fn modal_with_tool() -> PluginsModal {
        let (writer, reader) = loaded_plugins_pair();
        writer.publish_plugins(vec![LoadedPlugin {
            name: Arc::from("read"),
            tools: vec![PluginToolInfo {
                name: Arc::from("read"),
                description: TOOL_DESCRIPTION.into(),
                schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": PATH_DESCRIPTION },
                        "limit": { "type": "integer" },
                    },
                    "required": ["path"],
                }),
            }],
        }]);
        let mut modal = PluginsModal::new();
        modal.open(&EventHandle::disconnected_for_test().with_loaded_reader(reader));
        modal
    }

    fn screen(modal: &mut PluginsModal) -> String {
        let backend = ratatui::backend::TestBackend::new(100, 30);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|f| {
            modal.view(f, f.area());
        }).unwrap();
        crate::components::buffer_text(terminal.backend().buffer())
    }

    #[test]
    fn detail_shows_tool_description_and_input_schema() {
        let mut modal = modal_with_tool();
        modal.selected = modal
            .plugins
            .iter()
            .position(|plugin| plugin.name == "read")
            .unwrap();

        let text = screen(&mut modal);
        assert!(text.contains(TOOL_DESCRIPTION), "description missing:\n{text}");
        assert!(text.contains("path string (required)"), "typed prop missing:\n{text}");
        assert!(text.contains(PATH_DESCRIPTION), "prop doc missing:\n{text}");
        assert!(text.contains("limit integer"), "optional prop missing:\n{text}");
        assert!(!text.contains("limit integer (required)"));
    }

    #[test]
    fn a_plugin_without_tools_says_so() {
        let (writer, reader) = loaded_plugins_pair();
        writer.publish(&["sessions"]);
        let mut modal = PluginsModal::new();
        modal.open(&EventHandle::disconnected_for_test().with_loaded_reader(reader));
        modal.selected = modal
            .plugins
            .iter()
            .position(|plugin| plugin.name == "sessions")
            .unwrap();

        assert!(
            screen(&mut modal).contains("This plugin registers no tools"),
            "command-only plugin needs a note"
        );
    }
}
