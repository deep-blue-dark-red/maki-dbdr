use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum HeaderStyle {
    #[default]
    Plain,
    Path,
    Command,
    Grep,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OutputKeep {
    #[default]
    Head,
    Tail,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum BodyFormat {
    #[default]
    Plain,
    Markdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolRenderHints {
    pub header_style: HeaderStyle,
    pub body_format: BodyFormat,
    pub truncate_lines: Option<usize>,
    pub truncate_at: OutputKeep,
}

impl Default for ToolRenderHints {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl ToolRenderHints {
    pub const DEFAULT: Self = Self {
        header_style: HeaderStyle::Plain,
        body_format: BodyFormat::Plain,
        truncate_lines: None,
        truncate_at: OutputKeep::Head,
    };
}

macro_rules! hint {
    ($name:expr $(, $field:ident : $val:expr)* $(,)?) => {
        ($name, {
            #[allow(unused_mut)]
            let mut h = ToolRenderHints::DEFAULT;
            $(h.$field = $val;)*
            h
        })
    };
}

const DEFAULT_HINTS: &[(&str, ToolRenderHints)] = &[
    hint!("code_execution",
        truncate_at: OutputKeep::Tail,
    ),
    hint!("task",
        body_format: BodyFormat::Markdown,
    ),
    hint!("grep",
        header_style: HeaderStyle::Grep,
    ),
    hint!("glob",
        header_style: HeaderStyle::Command,
    ),
    hint!("read", header_style: HeaderStyle::Path),
    hint!("write", header_style: HeaderStyle::Path),
    hint!("edit", header_style: HeaderStyle::Path),
    hint!("multiedit", header_style: HeaderStyle::Path),
    hint!("question", truncate_lines: Some(100)),
];

pub struct RenderHintsRegistry {
    hints: HashMap<Arc<str>, ToolRenderHints>,
}

impl RenderHintsRegistry {
    pub fn new() -> Self {
        let hints = DEFAULT_HINTS
            .iter()
            .map(|(name, h)| (Arc::from(*name), *h))
            .collect();
        Self { hints }
    }

    pub fn get(&self, name: &str) -> &ToolRenderHints {
        static DEFAULT: ToolRenderHints = ToolRenderHints::DEFAULT;
        self.hints.get(name).unwrap_or(&DEFAULT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_case::test_case;

    #[test]
    fn default_hints_are_known_tools() {
        let known: Vec<&str> = DEFAULT_HINTS.iter().map(|(n, _)| *n).collect();
        for name in &known {
            assert!(!name.is_empty(), "empty tool name in DEFAULT_HINTS");
        }
    }

    #[test]
    fn no_all_default_entries() {
        for (name, hints) in DEFAULT_HINTS {
            assert_ne!(
                *hints,
                ToolRenderHints::DEFAULT,
                "{name} has all-default hints and should not be in DEFAULT_HINTS"
            );
        }
    }

    #[test_case("code_execution", true ; "code_execution_tail_keep")]
    #[test_case("read", false ; "read_head_keep")]
    fn truncate_at_direction(tool: &str, expect_tail: bool) {
        let reg = RenderHintsRegistry::new();
        let hints = reg.get(tool);
        assert_eq!(matches!(hints.truncate_at, OutputKeep::Tail), expect_tail);
    }

    #[test]
    fn unknown_tool_returns_default() {
        let reg = RenderHintsRegistry::new();
        let hints = reg.get("nonexistent_tool");
        assert!(matches!(hints.header_style, HeaderStyle::Plain));
    }
}