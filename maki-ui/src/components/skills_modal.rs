use crate::components::Overlay;
use crate::components::modal::Modal;
use crate::theme;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct SkillsJson {
    #[serde(default)]
    pub entries: Vec<SkillEntry>,
    #[serde(default)]
    pub inherits: Vec<SkillInherit>,
    #[serde(default)]
    pub exclude: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SkillEntry {
    pub path: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SkillInherit {
    pub path: String,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
    pub path: std::path::PathBuf,
    pub folder_name: String,
    pub source_dir: std::path::PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FolderTag {
    Maki,
    Global,
    Other,
    Local,
    Custom,
}

impl FolderTag {
    pub fn label(&self) -> &'static str {
        match self {
            FolderTag::Maki => "[MAKI]",
            FolderTag::Global => "[GLOBAL]",
            FolderTag::Other => "[OTHER]",
            FolderTag::Local => "[LOCAL]",
            FolderTag::Custom => "",
        }
    }
}

#[derive(Debug, Clone)]
pub struct FolderInfo {
    pub path: std::path::PathBuf,
    pub display_path: String,
    pub tag: FolderTag,
    pub is_enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Folders,
    Skills,
}

pub enum SkillsAction {
    None,
    CreateSkill(std::path::PathBuf),
    EditSkillsJson(std::path::PathBuf),
    EditSkill(std::path::PathBuf),
}

pub struct SkillsModal {
    open: bool,
    focus: Focus,
    folders: Vec<FolderInfo>,
    skills: Vec<SkillInfo>,
    selected_folder: usize,
    selected_skill: usize,
    skills_json: SkillsJson,
    cwd: std::path::PathBuf,
    input_mode: bool,
    input_buffer: crate::text_buffer::TextBuffer,
}

impl SkillsModal {
    pub fn new() -> Self {
        Self {
            open: false,
            focus: Focus::Skills,
            folders: Vec::new(),
            skills: Vec::new(),
            selected_folder: 0,
            selected_skill: 0,
            skills_json: SkillsJson::default(),
            cwd: std::path::PathBuf::new(),
            input_mode: false,
            input_buffer: crate::text_buffer::TextBuffer::new(String::new()),
        }
    }

    pub fn open(&mut self, cwd: std::path::PathBuf) {
        self.open = true;
        self.cwd = cwd;
        self.focus = Focus::Skills;
        self.selected_folder = 0;
        self.selected_skill = 0;
        self.refresh();
    }

    pub fn close(&mut self) {
        self.open = false;
        self.folders.clear();
        self.skills.clear();
        self.skills_json = SkillsJson::default();
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn refresh(&mut self) {
        let (skills, folders, json) = discover_skills_and_folders(&self.cwd);
        self.skills = skills;
        self.folders = folders;
        self.skills_json = json;

        self.selected_folder = self.selected_folder.min(self.folders.len().saturating_sub(1));
        self.selected_skill = self.selected_skill.min(self.skills.len().saturating_sub(1));
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> SkillsAction {
        if !self.open {
            return SkillsAction::None;
        }

        if self.input_mode {
            match key.code {
                KeyCode::Esc => {
                    self.input_mode = false;
                    self.input_buffer = crate::text_buffer::TextBuffer::new(String::new());
                }
                KeyCode::Enter => {
                    let path_str = self.input_buffer.value().trim().to_string();
                    if !path_str.is_empty() {
                        let path = std::path::Path::new(&path_str);
                        if path_str.starts_with('~') || path_str.starts_with('/') || path.is_absolute() {
                            let _resolved = if let Some(stripped) = path_str.strip_prefix("~/") {
                                if let Some(home) = maki_storage::paths::home() {
                                    home.join(stripped).to_string_lossy().into_owned()
                                } else {
                                    path_str.clone()
                                }
                            } else {
                                path_str.clone()
                            };
                        } else {
                            if !self.skills_json.entries.iter().any(|e| e.path == path_str) {
                                self.skills_json.entries.push(SkillEntry { path: path_str });
                                let _ = save_skills_json(&self.cwd, &self.skills_json);
                            }
                        }
                        self.input_mode = false;
                        self.input_buffer = crate::text_buffer::TextBuffer::new(String::new());
                        self.refresh();
                    }
                }
                _ => {
                    let _ = self.input_buffer.handle_key(key);
                }
            }
            return SkillsAction::None;
        }

        match key.code {
            KeyCode::Esc => {
                self.close();
                SkillsAction::None
            }
            KeyCode::Tab => {
                self.focus = match self.focus {
                    Focus::Folders => Focus::Skills,
                    Focus::Skills => Focus::Folders,
                };
                SkillsAction::None
            }
            KeyCode::Up => {
                match self.focus {
                    Focus::Folders => {
                        if !self.folders.is_empty() {
                            self.selected_folder = if self.selected_folder == 0 {
                                self.folders.len() - 1
                            } else {
                                self.selected_folder - 1
                            };
                        }
                    }
                    Focus::Skills => {
                        if !self.skills.is_empty() {
                            self.selected_skill = if self.selected_skill == 0 {
                                self.skills.len() - 1
                            } else {
                                self.selected_skill - 1
                            };
                        }
                    }
                }
                SkillsAction::None
            }
            KeyCode::Down => {
                match self.focus {
                    Focus::Folders => {
                        if !self.folders.is_empty() {
                            self.selected_folder = (self.selected_folder + 1) % self.folders.len();
                        }
                    }
                    Focus::Skills => {
                        if !self.skills.is_empty() {
                            self.selected_skill = (self.selected_skill + 1) % self.skills.len();
                        }
                    }
                }
                SkillsAction::None
            }
            KeyCode::Char(' ') | KeyCode::Enter => {
                match self.focus {
                    Focus::Folders => {
                        if let Some(folder) = self.folders.get(self.selected_folder).filter(|f| f.tag == FolderTag::Custom) {
                            if let Some(pos) = self.skills_json.entries.iter().position(|e| e.path == folder.display_path) {
                                self.skills_json.entries.remove(pos);
                            } else {
                                self.skills_json.entries.push(SkillEntry { path: folder.display_path.clone() });
                            }
                            let _ = save_skills_json(&self.cwd, &self.skills_json);
                            self.refresh();
                        }
                    }
                    Focus::Skills => {
                        if let Some(skill) = self.skills.get(self.selected_skill) {
                            let name_or_folder = &skill.folder_name;
                            if let Some(pos) = self.skills_json.exclude.iter().position(|x| x == name_or_folder) {
                                self.skills_json.exclude.remove(pos);
                            } else {
                                self.skills_json.exclude.push(name_or_folder.clone());
                            }
                            let _ = save_skills_json(&self.cwd, &self.skills_json);
                            self.refresh();
                        }
                    }
                }
                SkillsAction::None
            }
            KeyCode::Char('d') | KeyCode::Backspace | KeyCode::Delete if self.focus == Focus::Folders => {
                if let Some(folder) = self.folders.get(self.selected_folder) {
                    let mut changed = false;
                    if let Some(pos) = self.skills_json.entries.iter().position(|e| e.path == folder.display_path) {
                        self.skills_json.entries.remove(pos);
                        let _ = save_skills_json(&self.cwd, &self.skills_json);
                        changed = true;
                    }
                    if changed {
                        self.refresh();
                    }
                }
                SkillsAction::None
            }
            KeyCode::Char('a') if self.focus == Focus::Folders => {
                self.input_mode = true;
                self.input_buffer = crate::text_buffer::TextBuffer::new(String::new());
                SkillsAction::None
            }
            KeyCode::Char('c') => {
                let workspace_root = self.cwd.join(".agents");
                let workspace_skills = workspace_root.join("skills");
                let mut skill_num = 1;
                let mut skill_dir = workspace_skills.join(format!("skill_{}", skill_num));
                while skill_dir.exists() {
                    skill_num += 1;
                    skill_dir = workspace_skills.join(format!("skill_{}", skill_num));
                }
                if std::fs::create_dir_all(&skill_dir).is_ok() {
                    let skill_md = skill_dir.join("SKILL.md");
                    let template = format!(
                        "---\nname: skill_{}\ndescription: New skill description\n---\n\nWrite your skill instructions here...\n",
                        skill_num
                    );
                    if std::fs::write(&skill_md, template).is_ok() {
                        self.close();
                        return SkillsAction::CreateSkill(skill_md);
                    }
                }
                SkillsAction::None
            }
            KeyCode::Char('e') => {
                match self.focus {
                    Focus::Skills => {
                        let path = self.skills.get(self.selected_skill).map(|s| s.path.clone());
                        if let Some(p) = path {
                            self.close();
                            return SkillsAction::EditSkill(p);
                        }
                        SkillsAction::None
                    }
                    Focus::Folders => {
                        let path = self.folders.get(self.selected_folder).map(|f| f.path.clone());
                        if let Some(p) = path {
                            self.close();
                            return SkillsAction::EditSkillsJson(p);
                        }
                        SkillsAction::None
                    }
                }
            }
            _ => SkillsAction::None,
        }
    }

    pub fn view(&mut self, frame: &mut Frame, area: Rect) -> Rect {
        if !self.open {
            return Rect::default();
        }

        let modal = Modal {
            title: " Skills Manager ",
            width_percent: 85,
            max_height_percent: 80,
        };
        let (popup, inner) = modal.render(frame, area, 30);

        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
            .split(inner);

        let folder_height = ((self.folders.len() as u16).saturating_add(2))
            .min(chunks[0].height.saturating_div(2))
            .max(4);

        let left_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(folder_height), Constraint::Min(0)])
            .split(chunks[0]);

        let t = theme::current();

        // 1. Folders Block
        let folders_block = Block::default()
            .borders(Borders::ALL)
            .title(" Folders ")
            .border_style(if self.focus == Focus::Folders { t.accent } else { t.tool_dim });

        let mut folder_lines = Vec::new();
        for (i, folder) in self.folders.iter().enumerate() {
            let is_selected = self.focus == Focus::Folders && i == self.selected_folder;
            let check = match folder.tag {
                FolderTag::Custom => {
                    if self.skills_json.entries.iter().any(|e| e.path == folder.display_path) {
                        "[x]"
                    } else {
                        "[ ]"
                    }
                }
                other => other.label(),
            };
            let style = if is_selected { t.item_selected } else { t.item };
            folder_lines.push(Line::from(vec![
                Span::styled(format!("{} ", check), style),
                Span::styled(folder.display_path.clone(), style),
            ]));
        }
        let folders_paragraph = Paragraph::new(folder_lines).block(folders_block);
        frame.render_widget(folders_paragraph, left_chunks[0]);

        // 2. Skills Block
        let skills_block = Block::default()
            .borders(Borders::ALL)
            .title(" Skills ")
            .border_style(if self.focus == Focus::Skills { t.accent } else { t.tool_dim });

        let mut skill_lines = Vec::new();
        for (i, skill) in self.skills.iter().enumerate() {
            let is_selected = self.focus == Focus::Skills && i == self.selected_skill;
            let is_excluded = self.skills_json.exclude.iter().any(|x| x == &skill.folder_name);
            let check = if is_excluded { "[ ]" } else { "[x]" };
            let style = if is_selected { t.item_selected } else { t.item };
            
            let source_display = if skill.source_dir.to_string_lossy().contains(".agents/skills") {
                ".agents"
            } else if skill.source_dir.to_string_lossy().contains(".gemini/config/skills") {
                "global"
            } else {
                "custom"
            };

            skill_lines.push(Line::from(vec![
                Span::styled(format!("{} ", check), style),
                Span::styled(format!("{} ({})", skill.name, source_display), style),
            ]));
        }
        let skills_paragraph = Paragraph::new(skill_lines).block(skills_block);
        frame.render_widget(skills_paragraph, left_chunks[1]);

        // 3. Details / Help Block
        let details_block = Block::default()
            .borders(Borders::ALL)
            .title(" Details & Help ")
            .border_style(t.tool_dim);

        let mut details_lines = Vec::new();
        match self.focus {
            Focus::Folders => {
                if self.input_mode {
                    details_lines.push(Line::from(vec![
                        Span::styled("Add Skills Folder Path: ", t.accent),
                    ]));
                    let value = self.input_buffer.value();
                    let cursor_byte = crate::text_buffer::TextBuffer::char_to_byte(&value, self.input_buffer.x());
                    let (before, after) = value.split_at(cursor_byte);
                    let mut spans = vec![
                        Span::styled(before.to_string(), t.item),
                    ];
                    if let Some(c) = after.chars().next() {
                        spans.push(Span::styled(c.to_string(), t.item_selected));
                        spans.push(Span::styled(after[c.len_utf8()..].to_string(), t.item));
                    } else {
                        spans.push(Span::styled(" ", t.item_selected));
                    }
                    details_lines.push(Line::from(spans));
                    details_lines.push(Line::default());
                    details_lines.push(Line::from(vec![
                        Span::styled("Press Enter to add, Esc to cancel.", t.item_desc)
                    ]));
                } else if let Some(folder) = self.folders.get(self.selected_folder) {
                    details_lines.push(Line::from(vec![
                        Span::styled("Path: ", t.tool_dim),
                        Span::styled(folder.path.to_string_lossy().into_owned(), t.item),
                    ]));
                    details_lines.push(Line::default());
                    match folder.tag {
                        FolderTag::Custom => {
                            details_lines.push(Line::from(vec![
                                Span::styled("This is a custom workspace skills folder. Press Space/Enter to toggle inclusion in skills.json entries, or 'd' to remove it.", t.item_desc)
                            ]));
                        }
                        FolderTag::Local => {
                            details_lines.push(Line::from(vec![
                                Span::styled("This is a local workspace standard skills folder. Workspace-standard folders are automatically discovered and loaded.", t.item_desc)
                            ]));
                        }
                        other => {
                            details_lines.push(Line::from(vec![
                                Span::styled(format!("This is a {} standard skills folder. It is defined in your maki.config and can be removed by pressing 'd'.", other.label()), t.item_desc)
                            ]));
                        }
                    }
                }
            }
            Focus::Skills => {
                if let Some(skill) = self.skills.get(self.selected_skill) {
                    details_lines.push(Line::from(vec![
                        Span::styled("Skill: ", t.tool_dim),
                        Span::styled(skill.name.clone(), t.item.add_modifier(Modifier::BOLD)),
                    ]));
                    details_lines.push(Line::from(vec![
                        Span::styled("Folder: ", t.tool_dim),
                        Span::styled(skill.folder_name.clone(), t.item),
                    ]));
                    details_lines.push(Line::from(vec![
                        Span::styled("Source: ", t.tool_dim),
                        Span::styled(skill.source_dir.to_string_lossy().into_owned(), t.item),
                    ]));
                    details_lines.push(Line::default());
                    details_lines.push(Line::from(vec![
                        Span::styled("Description:", t.tool_dim)
                    ]));
                    details_lines.push(Line::from(vec![
                        Span::styled(skill.description.clone(), t.item)
                    ]));
                } else {
                    details_lines.push(Line::from(vec![
                        Span::styled("No skills found. Press 'c' to create a new skill in .agents/skills.", t.item_desc)
                    ]));
                }
            }
        }

        details_lines.push(Line::default());
        details_lines.push(Line::from(vec![
            Span::styled("--- Keyboard Shortcuts ---", t.tool_dim)
        ]));
        details_lines.push(Line::from(vec![
            Span::styled("  Tab        ", t.accent),
            Span::styled("Switch focus between Folders and Skills", t.item_desc)
        ]));
        details_lines.push(Line::from(vec![
            Span::styled("  Space/Enter", t.accent),
            Span::styled("Toggle custom folder or exclude skill", t.item_desc)
        ]));
        details_lines.push(Line::from(vec![
            Span::styled("  a          ", t.accent),
            Span::styled("Add a new skills folder (global config / workspace)", t.item_desc)
        ]));
        details_lines.push(Line::from(vec![
            Span::styled("  d/Backspace", t.accent),
            Span::styled("Remove selected skills folder", t.item_desc)
        ]));
        details_lines.push(Line::from(vec![
            Span::styled("  c          ", t.accent),
            Span::styled("Create a new skill in .agents/skills", t.item_desc)
        ]));
        details_lines.push(Line::from(vec![
            Span::styled("  e          ", t.accent),
            Span::styled("Open the selected folder or skill in your editor", t.item_desc)
        ]));
        details_lines.push(Line::from(vec![
            Span::styled("  Esc        ", t.accent),
            Span::styled("Close menu", t.item_desc)
        ]));

        let details_paragraph = Paragraph::new(details_lines)
            .block(details_block)
            .wrap(Wrap { trim: false });
        frame.render_widget(details_paragraph, chunks[1]);

        popup
    }
}

impl Overlay for SkillsModal {
    fn is_open(&self) -> bool {
        self.open
    }

    fn close(&mut self) {
        self.close()
    }
}

fn parse_skill_md(content: &str) -> Option<(String, String)> {
    if !content.starts_with("---") {
        return None;
    }
    let rest = &content[3..];
    let end_pos = rest.find("---")?;
    let yaml_str = &rest[..end_pos];
    let mut name = None;
    let mut description = None;
    for line in yaml_str.lines() {
        if let Some(val) = line.strip_prefix("name:") {
            name = Some(val.trim().trim_matches('"').trim_matches('\'').to_string());
        } else if let Some(val) = line.strip_prefix("description:") {
            description = Some(val.trim().trim_matches('"').trim_matches('\'').to_string());
        }
    }
    Some((name?, description.unwrap_or_default()))
}

fn find_project_ancestors(cwd: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut dirs = vec![cwd.to_path_buf()];
    let mut current = cwd;
    while let Some(parent) = current.parent() {
        dirs.push(parent.to_path_buf());
        if parent.join(".git").exists() {
            break;
        }
        current = parent;
    }
    dirs
}

fn determine_folder_tag(path: &std::path::Path, cwd: &std::path::Path) -> FolderTag {
    let path_str = path.to_string_lossy();
    
    // Find the project root (the first ancestor containing .git, or cwd if not found)
    let mut project_root = cwd.to_path_buf();
    let mut current = cwd;
    while let Some(parent) = current.parent() {
        if parent.join(".git").exists() {
            project_root = parent.to_path_buf();
            break;
        }
        current = parent;
    }

    if path_str.contains(".config/maki/skills") {
        FolderTag::Maki
    } else if path.starts_with(&project_root) {
        FolderTag::Local
    } else if path_str.contains(".agents/skills") {
        FolderTag::Global
    } else if path_str.contains(".config/opencode/skills")
        || path_str.contains(".gemini/config/skills")
        || path_str.contains(".claude/skills")
    {
        FolderTag::Other
    } else {
        FolderTag::Custom
    }
}

pub fn discover_skills_and_folders(
    cwd: &std::path::Path,
) -> (Vec<SkillInfo>, Vec<FolderInfo>, SkillsJson) {
    let mut folders = Vec::new();
    let mut skills = Vec::new();

    // 1. Global config skills (loaded from maki.config)

    // 2. Project workspace directories (local standard folders)
    let project_dirs = [
        (".agents/skills", ".agents/skills"),
        (".maki/skills", ".maki/skills"),
        (".claude/skills", ".claude/skills"),
        (".opencode/skills", ".opencode/skills"),
    ];
    let ancestors = find_project_ancestors(cwd);
    for (i, ancestor) in ancestors.iter().enumerate() {
        for &(rel, display) in &project_dirs {
            let path = ancestor.join(rel);
            if path.exists() {
                let display_path = if i == 0 {
                    display.to_string()
                } else {
                    let mut parent_steps = String::new();
                    let mut temp = cwd;
                    while temp != ancestor && temp.parent().is_some() {
                        parent_steps.push_str("../");
                        temp = temp.parent().unwrap();
                    }
                    format!("{parent_steps}{rel}")
                };
                folders.push(FolderInfo {
                    path,
                    display_path,
                    tag: FolderTag::Local,
                    is_enabled: true,
                });
            }
        }
    }

    let workspace_root = cwd.join(".agents");

    let skills_json_path = workspace_root.join("skills.json");
    let skills_json = if skills_json_path.exists() {
        std::fs::read_to_string(&skills_json_path)
            .ok()
            .and_then(|s| serde_json::from_str::<SkillsJson>(&s).ok())
            .unwrap_or_default()
    } else {
        SkillsJson::default()
    };

    for entry in &skills_json.entries {
        let path = std::path::PathBuf::from(&entry.path);
        let resolved_path = if path.is_relative() {
            cwd.join(&path)
        } else {
            path.clone()
        };
        folders.push(FolderInfo {
            path: resolved_path,
            display_path: entry.path.clone(),
            tag: FolderTag::Custom,
            is_enabled: true,
        });
    }

    for folder in &folders {
        if !folder.is_enabled || !folder.path.exists() {
            continue;
        }
        if let Ok(entries) = std::fs::read_dir(&folder.path) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let skill_md = path.join("SKILL.md");
                    if skill_md.exists() {
                        let folder_name = path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned();
                        let (name, description) = std::fs::read_to_string(&skill_md)
                            .ok()
                            .and_then(|c| parse_skill_md(&c))
                            .unwrap_or_else(|| (folder_name.clone(), String::new()));
                        
                        skills.push(SkillInfo {
                            name,
                            description,
                            path: skill_md,
                            folder_name,
                            source_dir: folder.path.clone(),
                        });
                    }
                }
            }
        }
    }

    let mut unique_skills = Vec::new();
    for s in skills {
        if !unique_skills.iter().any(|us: &SkillInfo| us.name == s.name) {
            unique_skills.push(s);
        }
    }
    (unique_skills, folders, skills_json)
}

pub fn save_skills_json(cwd: &std::path::Path, skills_json: &SkillsJson) -> Result<(), std::io::Error> {
    let workspace_root = cwd.join(".agents");
    std::fs::create_dir_all(&workspace_root)?;
    let path = workspace_root.join("skills.json");
    let content = serde_json::to_string_pretty(skills_json)?;
    std::fs::write(path, content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_skill_md_extracts_fields() {
        let content = "---\nname: my-skill\ndescription: \"does some things\"\n---\nbody content";
        let parsed = parse_skill_md(content);
        assert!(parsed.is_some());
        let (name, desc) = parsed.unwrap();
        assert_eq!(name, "my-skill");
        assert_eq!(desc, "does some things");
    }
}
