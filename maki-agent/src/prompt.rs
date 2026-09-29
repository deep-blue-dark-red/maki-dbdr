use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

use arc_swap::ArcSwapOption;
use strum::{Display, EnumIter, EnumString, IntoEnumIterator};

pub trait ValidNames: IntoEnumIterator + std::fmt::Display {
    fn valid_names() -> String {
        Self::iter()
            .map(|v| v.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

pub const SYSTEM_PROMPT: &str = include_str!("prompts/system.md");

/// File under the config dir that `/system_prompt` opens for editing.
pub const USER_SYSTEM_PROMPT_FILE: &str = "system.md";
pub const USER_IDENTITY_FILE: &str = "identity.md";
pub const USER_TONE_FILE: &str = "tone.md";

/// User-authored replacement for [`SYSTEM_PROMPT`], populated by
/// [`load_user_system_prompt`]. Empty until something loads it, so unit tests
/// and library consumers never pick up whatever is on the host's disk.
static USER_SYSTEM_PROMPT: ArcSwapOption<String> = ArcSwapOption::const_empty();

/// User-authored overrides for the singleton slots, populated by
/// [`load_user_slot_overrides`]. They outrank any plugin's `set_prompt`
/// claim: an explicit file on disk is the user speaking, a plugin is a
/// suggestion.
static USER_IDENTITY: ArcSwapOption<String> = ArcSwapOption::const_empty();
static USER_TONE: ArcSwapOption<String> = ArcSwapOption::const_empty();

/// Read `<config_dir>/system.md` into the system prompt override.
///
/// Call once at startup and again whenever the config is reloaded — the file
/// is only re-read here, so an edit lands on the next `/reload` or restart.
/// A missing or blank file clears the override and restores the built-in
/// prompt. Returns whether an override is now active.
pub fn load_user_system_prompt() -> Result<bool, std::io::Error> {
    let dir = maki_storage::paths::config_dir()?;
    let text = read_user_prompt_file(&dir.join(USER_SYSTEM_PROMPT_FILE))?;
    if let Some(text) = text {
        tracing::info!(dir = %dir.display(), bytes = text.len(), "loaded user system prompt");
        USER_SYSTEM_PROMPT.store(Some(Arc::new(text)));
        Ok(true)
    } else {
        USER_SYSTEM_PROMPT.store(None);
        Ok(false)
    }
}

/// Read `<config_dir>/identity.md` and `tone.md` into singleton-slot
/// overrides. Same semantics as [`load_user_system_prompt`]: a missing or
/// blank file clears that override and restores the plugin/default chain.
/// Returns how many overrides are active.
pub fn load_user_slot_overrides() -> Result<u8, std::io::Error> {
    let dir = maki_storage::paths::config_dir()?;
    apply_user_slot_overrides_from(&dir)
}

fn apply_user_slot_overrides_from(dir: &std::path::Path) -> Result<u8, std::io::Error> {
    let mut active = 0;
    for (file, store) in [
        (USER_IDENTITY_FILE, &USER_IDENTITY),
        (USER_TONE_FILE, &USER_TONE),
    ] {
        match read_user_prompt_file(&dir.join(file))? {
            Some(text) => {
                store.store(Some(Arc::new(text)));
                active += 1;
            }
            None => store.store(None),
        }
    }
    Ok(active)
}

/// The singleton-slot overrides currently loaded from disk, paired with the
/// slot they belong to.
pub fn user_slot_overrides() -> Vec<(Slot, Arc<str>)> {
    [(Slot::Identity, &USER_IDENTITY), (Slot::Tone, &USER_TONE)]
        .into_iter()
        .filter_map(|(slot, store)| {
            store
                .load_full()
                .map(|text| (slot, Arc::from(text.as_str())))
        })
        .collect()
}

/// Insert user overrides as the winning entries for their singleton slots.
/// Singleton rendering takes the last entry, so overlaying after plugin
/// collection is what gives the user file priority.
pub fn overlay_user_slots(slots: &mut ResolvedSlots, overrides: &[(Slot, Arc<str>)]) {
    for &(slot, ref content) in overrides {
        for &pid in PromptId::ALL {
            if pid.has_slot(slot) {
                slots.insert(
                    pid,
                    slot,
                    SlotEntry {
                        plugin: Arc::from(USER_PLUGIN_NAME),
                        content: content.to_string(),
                    },
                );
            }
        }
    }
}

pub const USER_PLUGIN_NAME: &str = "user";

/// The config file backing a user-editable slot, if it has one.
pub fn slot_override_file(slot: Slot) -> Option<&'static str> {
    match slot {
        Slot::Identity => Some(USER_IDENTITY_FILE),
        Slot::Tone => Some(USER_TONE_FILE),
        _ => None,
    }
}

/// Whether this prompt's body came from the user's `system.md` rather than the
/// built-in template. Callers that validate against the template's slot markers
/// use this to soften "no such slot" into a warning: a hand-edited prompt is
/// under no obligation to keep every marker, and a plugin shouldn't fail to
/// load because the user deleted one.
pub fn is_user_supplied(id: PromptId) -> bool {
    id == PromptId::System && USER_SYSTEM_PROMPT.load().is_some()
}

/// Read one user prompt file. `None` means "use the default": the file is
/// absent, or holds nothing but whitespace. Any other read failure is an
/// error, so a permissions problem is reported rather than silently swapping
/// the user's content back to the default.
fn read_user_prompt_file(path: &std::path::Path) -> Result<Option<String>, std::io::Error> {
    match std::fs::read_to_string(path) {
        Ok(text) if text.trim().is_empty() => Ok(None),
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}
pub const PLAN_PROMPT: &str = include_str!("prompts/plan.md");
pub const RESEARCH_PROMPT: &str = include_str!("prompts/research.md");
pub const GENERAL_PROMPT: &str = include_str!("prompts/general.md");
pub const COMPACTION_SYSTEM: &str = include_str!("prompts/compaction.md");
pub const COMPACTION_USER: &str = include_str!("prompts/compaction_user.md");
pub const CHECKPOINT_USER: &str = include_str!("prompts/checkpoint_user.md");

pub const DEFAULT_IDENTITY: &str = r#"You are Maki, an interactive CLI coding agent. Use the tools available to assist the user with software engineering tasks. Complete tasks successfully while minimizing token usage and tool calls to avoid context bloat.

You must **never** generate or guess URLs unless they are for helping the user with programming."#;

pub const DEFAULT_TONE: &str = r#"- Be concise. Your output is displayed on a CLI rendered in monospace. Use GitHub-flavored markdown.
- Only use emojis if explicitly requested.
- Do not add comments to code unless asked.
- Output text to communicate with the user; all text you output outside of tool use is displayed to the user. Only use tools to complete tasks. **never** use bash echo or other command-line tools to communicate thoughts, explanations, diagrams, or instructions to the user. Output all communication directly in your response text instead.
- **never** create files unless absolutely necessary. **always** prefer editing existing files. Use /tmp/maki/" if necessary"#;

const NATIVE_EFFICIENT_TOOLS: &[&str] = &["batch", "index", "list", "code_execution", "task"];
const INSTRUCTIONS_MARKER: &str = "{{instructions}}";

/// Singleton: alphabetically last plugin wins, discarding all prior content
/// and built-in defaults.  Used for slots with opinionated defaults where
/// multiple contributors would conflict (identity, tone).
///
/// Aggregate: all entries are joined.  Used for genuinely additive slots
/// where multiple plugins contributing is the point (tool usage hints,
/// efficient tools, after-instructions).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EnumString, Display)]
#[strum(serialize_all = "snake_case")]
pub enum SlotKind {
    Singleton,
    Aggregate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EnumString, Display, EnumIter)]
#[strum(serialize_all = "snake_case")]
pub enum Slot {
    Identity,
    Tone,
    ToolUsage,
    EfficientTools,
    Conventions,
    AfterInstructions,
}

impl Slot {
    fn marker(self) -> &'static str {
        match self {
            Slot::Identity => "{{identity}}",
            Slot::Tone => "{{tone}}",
            Slot::ToolUsage => "{{tool_usage}}",
            Slot::EfficientTools => "{{efficient_tools}}",
            Slot::Conventions => "{{conventions}}",
            Slot::AfterInstructions => "{{after_instructions}}",
        }
    }

    pub fn kind(self) -> SlotKind {
        match self {
            Slot::Identity | Slot::Tone => SlotKind::Singleton,
            Slot::ToolUsage
            | Slot::EfficientTools
            | Slot::Conventions
            | Slot::AfterInstructions => SlotKind::Aggregate,
        }
    }

    /// Built-in default content for singleton slots.  When no plugin
    /// registers content for a singleton slot, the default is used.
    /// Aggregate slots have no default (the template carries the static
    /// text around the marker).
    pub fn default_content(self) -> Option<&'static str> {
        match self {
            Slot::Identity => Some(DEFAULT_IDENTITY),
            Slot::Tone => Some(DEFAULT_TONE),
            _ => None,
        }
    }

    pub fn names_for_kind(kind: SlotKind) -> String {
        Self::iter()
            .filter(|s| s.kind() == kind)
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EnumString, Display, EnumIter)]
#[strum(serialize_all = "snake_case")]
pub enum PromptId {
    System,
    Research,
    General,
}

impl PromptId {
    pub const ALL: &[PromptId] = &[PromptId::System, PromptId::Research, PromptId::General];
}

impl ValidNames for Slot {}
impl ValidNames for PromptId {}

pub struct SlotEntry {
    pub plugin: Arc<str>,
    pub content: String,
}

#[derive(Default)]
pub struct ResolvedSlots {
    entries: HashMap<(PromptId, Slot), Vec<SlotEntry>>,
}

impl ResolvedSlots {
    pub fn get(&self, prompt: PromptId, slot: Slot) -> &[SlotEntry] {
        self.entries
            .get(&(prompt, slot))
            .map(|v| v.as_slice())
            .unwrap_or_default()
    }

    pub fn insert(&mut self, prompt: PromptId, slot: Slot, entry: SlotEntry) {
        self.entries.entry((prompt, slot)).or_default().push(entry);
    }
}

impl PromptId {
    /// The prompt body before slots are filled. `System` yields the user's
    /// `system.md` when one is loaded; the other prompts are always built in.
    fn template(self) -> Cow<'static, str> {
        match self {
            PromptId::System => match USER_SYSTEM_PROMPT.load_full() {
                Some(user) => Cow::Owned(user.as_str().to_owned()),
                None => Cow::Borrowed(SYSTEM_PROMPT),
            },
            PromptId::Research => Cow::Borrowed(RESEARCH_PROMPT),
            PromptId::General => Cow::Borrowed(GENERAL_PROMPT),
        }
    }

    /// A slot exists for this prompt iff its marker is present in the template.
    /// Markers that are absent get no content (and we warn at collection time
    /// when a plugin targets them explicitly).
    pub fn has_slot(self, slot: Slot) -> bool {
        self.template().contains(slot.marker())
    }
}

fn render_slot(slots: &ResolvedSlots, prompt: PromptId, slot: Slot) -> String {
    if slot == Slot::EfficientTools {
        return render_efficient_tools(slots, prompt);
    }
    let entries = slots.get(prompt, slot);
    match slot.kind() {
        SlotKind::Singleton => {
            if let Some(last) = entries.last() {
                last.content.clone()
            } else if let Some(default) = slot.default_content() {
                default.to_string()
            } else {
                String::new()
            }
        }
        // Aggregate slots have no built-in defaults; content comes entirely from plugins.
        SlotKind::Aggregate => {
            let mut parts = Vec::new();
            for entry in entries {
                parts.push(entry.content.as_str());
            }
            parts.join("\n")
        }
    }
}

fn render_efficient_tools(slots: &ResolvedSlots, prompt: PromptId) -> String {
    let extras = slots.get(prompt, Slot::EfficientTools);
    let names = NATIVE_EFFICIENT_TOOLS
        .iter()
        .copied()
        .chain(extras.iter().map(|e| e.content.as_str()))
        .collect::<Vec<_>>()
        .join(", ");
    format!("Most efficient tools: {names}.")
}

/// Fill each `{{slot}}` marker in the template with its rendered content and
/// drop the project instructions (AGENTS.md and friends) into `{{instructions}}`.
pub fn assemble(id: PromptId, slots: &ResolvedSlots, instructions: &str) -> String {
    fill_template(id.template().into_owned(), id, slots, instructions)
}

/// Slot- and instruction-fill an already-chosen template. Split out from
/// [`assemble`] so a caller-supplied body (a user's `system.md`) can be
/// exercised without touching the process-wide override.
fn fill_template(
    mut out: String,
    id: PromptId,
    slots: &ResolvedSlots,
    instructions: &str,
) -> String {
    for slot in Slot::iter() {
        out = fill_marker(&out, slot.marker(), &render_slot(slots, id, slot));
    }
    out.replace(INSTRUCTIONS_MARKER, instructions)
}

/// Replace a slot marker with its content. When the content is empty, also drop
/// the marker's own line (the trailing newline) so empty slots leave no blank
/// gap, without touching any other whitespace in the prompt.
fn fill_marker(template: &str, marker: &str, content: &str) -> String {
    if content.is_empty() {
        return template
            .replace(&format!("{marker}\n"), "")
            .replace(marker, "");
    }
    template.replace(marker, content)
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_case::test_case;

    const NATIVE_EFFICIENT_LINE: &str = "Most efficient tools: batch, index, code_execution, task";

    fn slots(prompt: PromptId, entries: &[(Slot, &str)]) -> ResolvedSlots {
        let mut slots = ResolvedSlots::default();
        for &(slot, content) in entries {
            slots.insert(
                prompt,
                slot,
                SlotEntry {
                    plugin: Arc::from("p"),
                    content: content.into(),
                },
            );
        }
        slots
    }

    fn at(out: &str, needle: &str) -> usize {
        out.find(needle)
            .unwrap_or_else(|| panic!("missing: {needle}"))
    }

    #[test]
    fn absent_system_md_falls_back_to_builtin() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            read_user_prompt_file(&dir.path().join(USER_SYSTEM_PROMPT_FILE)).unwrap(),
            None
        );
    }

    #[test_case("" ; "empty")]
    #[test_case("   \n\t\n  " ; "whitespace")]
    fn blank_system_md_falls_back_to_builtin(body: &str) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(USER_SYSTEM_PROMPT_FILE), body).unwrap();
        assert_eq!(
            read_user_prompt_file(&dir.path().join(USER_SYSTEM_PROMPT_FILE)).unwrap(),
            None
        );
    }

    #[test]
    fn system_md_is_read_verbatim() {
        let dir = tempfile::tempdir().unwrap();
        let body = "You are Custom.\n\n{{instructions}}\n";
        std::fs::write(dir.path().join(USER_SYSTEM_PROMPT_FILE), body).unwrap();
        assert_eq!(
            read_user_prompt_file(&dir.path().join(USER_SYSTEM_PROMPT_FILE))
                .unwrap()
                .as_deref(),
            Some(body)
        );
    }

    #[test]
    fn user_prompt_replaces_builtin_and_still_fills_slots() {
        let user = "You are Custom.\n{{tool_usage}}\n{{instructions}}\n";
        let s = slots(PromptId::System, &[(Slot::ToolUsage, "HINT")]);
        let out = fill_template(user.to_string(), PromptId::System, &s, "INSTR");

        assert!(out.starts_with("You are Custom."), "got:\n{out}");
        assert!(out.contains("HINT"), "slot not filled:\n{out}");
        assert!(out.contains("INSTR"), "instructions not filled:\n{out}");
        assert!(!out.contains("{{"), "unfilled marker left:\n{out}");
    }

    #[test]
    fn empty_slots_emit_template_and_native_efficient_line() {
        let out = assemble(PromptId::System, &ResolvedSlots::default(), "");
        assert!(out.starts_with("You are Maki"));
        assert!(
            !out.contains("{{"),
            "unfilled marker left in output:\n{out}"
        );
        assert!(out.contains(&format!("{NATIVE_EFFICIENT_LINE}.")));
    }

    /// One test to pin the whole System layout: every slot shows up, in order,
    /// around the instructions. Covers presence and ordering for all of them.
    #[test]
    fn system_sections_land_in_layout_order() {
        let s = slots(
            PromptId::System,
            &[
                (Slot::ToolUsage, "TOOL_USAGE"),
                (Slot::EfficientTools, "EXTRA_TOOL"),
                (Slot::Conventions, "CONVENTIONS"),
                (Slot::AfterInstructions, "AFTER"),
            ],
        );
        let out = assemble(PromptId::System, &s, "INSTR");
        let positions = ["TOOL_USAGE", "EXTRA_TOOL", "CONVENTIONS", "INSTR", "AFTER"]
            .map(|needle| at(&out, needle));
        assert!(
            positions.is_sorted(),
            "sections out of layout order ({positions:?}):\n{out}"
        );
    }

    /// Regression: a `tool_usage` hint must land inside the `# Tool usage`
    /// section, not be appended after the rest of the prompt.
    #[test]
    fn tool_usage_hint_lands_inside_tool_usage_section() {
        const HINT: &str = "- HINT_LINE";
        let s = slots(PromptId::System, &[(Slot::ToolUsage, HINT)]);
        let out = assemble(PromptId::System, &s, "");
        let hint = at(&out, HINT);
        assert!(
            at(&out, "# Tool usage") < hint,
            "hint before its section:\n{out}"
        );
        assert!(
            hint < at(&out, "# Conventions"),
            "hint leaked past section:\n{out}"
        );
    }

    #[test]
    fn efficient_tools_extras_join_native_list() {
        let s = slots(
            PromptId::System,
            &[
                (Slot::EfficientTools, "index"),
                (Slot::EfficientTools, "foo"),
            ],
        );
        let out = assemble(PromptId::System, &s, "");
        assert!(out.contains(&format!("{NATIVE_EFFICIENT_LINE}, index, foo.")));
    }

    #[test]
    fn same_slot_preserves_insertion_order() {
        let s = slots(
            PromptId::System,
            &[(Slot::ToolUsage, "FIRST"), (Slot::ToolUsage, "SECOND")],
        );
        let out = assemble(PromptId::System, &s, "");
        assert!(at(&out, "FIRST") < at(&out, "SECOND"));
    }

    /// Only System carries AfterInstructions, so the same content shows up there
    /// but never leaks into the subagent prompts.
    #[test]
    fn after_instructions_only_reaches_system() {
        let mut s = ResolvedSlots::default();
        for &pid in PromptId::ALL {
            s.insert(
                pid,
                Slot::AfterInstructions,
                SlotEntry {
                    plugin: Arc::from("p"),
                    content: "AFTER".into(),
                },
            );
        }
        assert!(assemble(PromptId::System, &s, "").contains("AFTER"));
        assert!(!assemble(PromptId::Research, &s, "").contains("AFTER"));
        assert!(!assemble(PromptId::General, &s, "").contains("AFTER"));
    }

    #[test]
    fn research_drops_conventions_but_keeps_efficient_extras() {
        let s = slots(
            PromptId::Research,
            &[
                (Slot::Conventions, "DROPPED"),
                (Slot::EfficientTools, "EXTRA"),
            ],
        );
        let out = assemble(PromptId::Research, &s, "");
        assert!(!out.contains("DROPPED"));
        assert!(out.contains(&format!("{NATIVE_EFFICIENT_LINE}, EXTRA.")));
    }

    #[test_case(PromptId::System, Slot::ToolUsage, true ; "system_tool_usage")]
    #[test_case(PromptId::System, Slot::EfficientTools, true ; "system_efficient")]
    #[test_case(PromptId::System, Slot::Conventions, true ; "system_conventions")]
    #[test_case(PromptId::System, Slot::AfterInstructions, true ; "system_after")]
    #[test_case(PromptId::System, Slot::Identity, true ; "system_identity")]
    #[test_case(PromptId::System, Slot::Tone, true ; "system_tone")]
    #[test_case(PromptId::Research, Slot::Conventions, false ; "research_no_conventions")]
    #[test_case(PromptId::Research, Slot::AfterInstructions, false ; "research_no_after")]
    #[test_case(PromptId::Research, Slot::Identity, false ; "research_no_identity")]
    #[test_case(PromptId::Research, Slot::Tone, false ; "research_no_tone")]
    #[test_case(PromptId::General, Slot::AfterInstructions, false ; "general_no_after")]
    #[test_case(PromptId::General, Slot::Identity, false ; "general_no_identity")]
    #[test_case(PromptId::General, Slot::Tone, false ; "general_no_tone")]
    fn has_slot(prompt: PromptId, slot: Slot, expected: bool) {
        assert_eq!(prompt.has_slot(slot), expected);
    }

    #[test_case("after_instructions", Some(Slot::AfterInstructions) ; "valid_slot")]
    #[test_case("tool_usagee", None ; "typo_slot")]
    #[test_case("identity", Some(Slot::Identity) ; "identity_slot")]
    #[test_case("tone", Some(Slot::Tone) ; "tone_slot")]
    fn slot_parse_is_plugin_contract(input: &str, expected: Option<Slot>) {
        assert_eq!(input.parse::<Slot>().ok(), expected);
    }

    #[test_case("system", Some(PromptId::System) ; "valid_prompt")]
    #[test_case("systm", None ; "typo_prompt")]
    fn prompt_parse_is_plugin_contract(input: &str, expected: Option<PromptId>) {
        assert_eq!(input.parse::<PromptId>().ok(), expected);
    }

    #[test_case(Slot::Identity, SlotKind::Singleton ; "identity_singleton")]
    #[test_case(Slot::Tone, SlotKind::Singleton ; "tone_singleton")]
    #[test_case(Slot::Conventions, SlotKind::Aggregate ; "conventions_aggregate")]
    #[test_case(Slot::ToolUsage, SlotKind::Aggregate ; "tool_usage_aggregate")]
    #[test_case(Slot::EfficientTools, SlotKind::Aggregate ; "efficient_aggregate")]
    #[test_case(Slot::AfterInstructions, SlotKind::Aggregate ; "after_aggregate")]
    fn slot_kind_matches_expectations(slot: Slot, expected: SlotKind) {
        assert_eq!(slot.kind(), expected);
    }

    #[test]
    fn singleton_default_used_when_empty() {
        let out = assemble(PromptId::System, &ResolvedSlots::default(), "");
        assert!(out.starts_with("You are Maki"));
    }

    #[test]
    fn singleton_entry_replaces_default() {
        let mut s = ResolvedSlots::default();
        s.insert(
            PromptId::System,
            Slot::Identity,
            SlotEntry {
                plugin: Arc::from("user"),
                content: "Custom identity".into(),
            },
        );
        let out = assemble(PromptId::System, &s, "");
        assert!(out.contains("Custom identity"));
        assert!(!out.contains("You are Maki"));
    }

    #[test]
    fn singleton_last_entry_wins() {
        let mut s = ResolvedSlots::default();
        s.insert(
            PromptId::System,
            Slot::Identity,
            SlotEntry {
                plugin: Arc::from("first"),
                content: "FIRST".into(),
            },
        );
        s.insert(
            PromptId::System,
            Slot::Identity,
            SlotEntry {
                plugin: Arc::from("second"),
                content: "SECOND".into(),
            },
        );
        let out = assemble(PromptId::System, &s, "");
        assert!(out.contains("SECOND"));
        assert!(!out.contains("FIRST"));
        assert!(!out.contains("You are Maki"));
    }

    #[test]
    fn identity_only_in_system_not_subagents() {
        assert!(PromptId::System.has_slot(Slot::Identity));
        assert!(!PromptId::Research.has_slot(Slot::Identity));
        assert!(!PromptId::General.has_slot(Slot::Identity));
    }

    #[test]
    fn tone_only_in_system_not_subagents() {
        assert!(PromptId::System.has_slot(Slot::Tone));
        assert!(!PromptId::Research.has_slot(Slot::Tone));
        assert!(!PromptId::General.has_slot(Slot::Tone));
    }

    #[test]
    fn conventions_entry_appends_to_template_defaults() {
        let mut s = ResolvedSlots::default();
        s.insert(
            PromptId::System,
            Slot::Conventions,
            SlotEntry {
                plugin: Arc::from("plugin"),
                content: "- Extra rule".into(),
            },
        );
        let out = assemble(PromptId::System, &s, "");
        assert!(out.contains("Confirm a library exists in the project's dependency files"));
        assert!(out.contains("- Extra rule"));
    }

    #[test]
    fn user_slot_files_are_singleton_only() {
        assert_eq!(slot_override_file(Slot::Identity), Some(USER_IDENTITY_FILE));
        assert_eq!(slot_override_file(Slot::Tone), Some(USER_TONE_FILE));
        assert_eq!(slot_override_file(Slot::ToolUsage), None);
    }

    #[test]
    fn slot_override_files_override_plugins_and_blank_files_fall_through() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(USER_IDENTITY_FILE), "You are Custom.\n").unwrap();
        std::fs::write(dir.path().join(USER_TONE_FILE), "   \n").unwrap();
        assert_eq!(apply_user_slot_overrides_from(dir.path()).unwrap(), 1);

        let mut slots = ResolvedSlots::default();
        slots.insert(
            PromptId::System,
            Slot::Tone,
            SlotEntry {
                plugin: Arc::from("plugin"),
                content: "plugin tone".into(),
            },
        );
        overlay_user_slots(&mut slots, &user_slot_overrides());

        let identity: Vec<_> = slots
            .get(PromptId::System, Slot::Identity)
            .iter()
            .map(|e| (e.plugin.to_string(), e.content.clone()))
            .collect();
        assert_eq!(identity.len(), 1);
        assert_eq!(identity[0].0, USER_PLUGIN_NAME);
        assert_eq!(identity[0].1, "You are Custom.\n");

        let out = assemble(PromptId::System, &slots, "");
        assert!(out.contains("You are Custom."));
        assert!(out.contains("plugin tone"), "blank file must fall through");
    }
}
