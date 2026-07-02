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

#[derive(Debug, Clone)]
pub struct FolderInfo {
    pub path: std::path::PathBuf,
    pub display_path: String,
    pub is_standard: bool,
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
                        if let Some(folder) = self.folders.get(self.selected_folder).filter(|f| !f.is_standard) {
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
                self.close();
                let path = self.cwd.join(".agents").join("skills.json");
                if !path.exists() {
                    let _ = save_skills_json(&self.cwd, &self.skills_json);
                }
                SkillsAction::EditSkillsJson(path)
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

        let left_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(8), Constraint::Min(0)])
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
            let check = if folder.is_standard {
                "[Standard]"
            } else if self.skills_json.entries.iter().any(|e| e.path == folder.display_path) {
                "[x]"
            } else {
                "[ ]"
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
                if let Some(folder) = self.folders.get(self.selected_folder) {
                    details_lines.push(Line::from(vec![
                        Span::styled("Path: ", t.tool_dim),
                        Span::styled(folder.path.to_string_lossy().into_owned(), t.item),
                    ]));
                    details_lines.push(Line::default());
                    if folder.is_standard {
                        details_lines.push(Line::from(vec![
                            Span::styled("This is a standard customization root folder. Standard roots are automatically discovered and cannot be toggled or deleted.", t.item_desc)
                        ]));
                    } else {
                        details_lines.push(Line::from(vec![
                            Span::styled("This is a custom skills folder. Press Space/Enter to toggle inclusion in skills.json entries.", t.item_desc)
                        ]));
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
            Span::styled("Toggle selected item (Exclude/Include)", t.item_desc)
        ]));
        details_lines.push(Line::from(vec![
            Span::styled("  c          ", t.accent),
            Span::styled("Create a new skill in .agents/skills", t.item_desc)
        ]));
        details_lines.push(Line::from(vec![
            Span::styled("  e          ", t.accent),
            Span::styled("Open .agents/skills.json in your editor", t.item_desc)
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

pub fn discover_skills_and_folders(
    cwd: &std::path::Path,
) -> (Vec<SkillInfo>, Vec<FolderInfo>, SkillsJson) {
    let mut folders = Vec::new();
    let mut skills = Vec::new();

    let global_root = std::path::PathBuf::from("/Users/mcp/.gemini/config");
    let global_skills = global_root.join("skills");
    folders.push(FolderInfo {
        path: global_skills.clone(),
        display_path: "~/config/skills".to_string(),
        is_standard: true,
        is_enabled: true,
    });

    let workspace_root = cwd.join(".agents");
    let workspace_skills = workspace_root.join("skills");
    folders.push(FolderInfo {
        path: workspace_skills.clone(),
        display_path: ".agents/skills".to_string(),
        is_standard: true,
        is_enabled: true,
    });

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
            is_standard: false,
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

    (skills, folders, skills_json)
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
