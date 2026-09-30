# Maki Project Architecture

## Lua Plugin System

### Runtime
- mlua crate on a dedicated OS thread ("maki-lua") with smol LocalExecutor
- 512 MB memory limit per VM; interrupt hook every 128 VM ticks for shutdown/cancel/deadline
- Sandbox: require/io/package stripped from globals; mlua.sandbox(true) restricts os/debug
- Per-plugin env tables with __index to globals (isolation between plugins)
- Custom scoped require blocks path traversal; embeds plugins at compile time via include_dir!

### Plugin Contract
Each plugin's init.lua calls maki.api.register_tool() with:
- name, description, schema (JSON Schema)
- handler(input, ctx) - sync return or nil then ctx:finish(result) for async
- Optional: header (UI summary), restore (session re-render), permission_scopes (dynamic perms), mutable_path, timeout, start_annotation

### Tool Call Flow
LLM tool_use -> ToolRegistry.get() -> LuaTool -> flume channel Request::CallTool -> Lua coroutine handler(input, ctx) -> ToolCallReply -> back to agent history

### API Surface (maki.*)
- api: register_tool, register_command, register_prompt_hint, set_prompt
- fs: permission-gated filesystem (read/write/abspath/metadata/dir)
- fn: subprocess jobstart/jobwait/jobstop (Neovim-style)
- ui: buffers, highlight, theme_color, humantime
- agent: sub-agent spawning (run/tools/system_prompt/resolve_model)
- treesitter: 22 language parsers (get_parser, get_node_text)
- env, json, yaml, net, log, text, async, keymap

### Permissions
5 capabilities via plugin.toml: FsRead, FsWrite, Net, Run, Env
API calls wrapped with permissions.guard() - errors if not granted

### ToolRegistry (maki-agent/src/tools/registry.rs)
Sources: Native, Mcp, Lua. Lock-free reads via ArcSwap.
Lua plugins can replace native tools; native fallback preserved.

### Shared Libraries (plugins/lib/maki/)
- tool_view.lua: collapsible output widget with ring buffer
- fuzzy_replace.lua: 7-strategy matching (exact, whitespace, block-anchor, Levenshtein)
- truncate.lua, shorten_path.lua, color.lua, list_picker.lua, text_input.lua

---

## Memories System

### Purpose
Project-scoped persistent notes for cross-session knowledge (decisions, gotchas, architecture).

### Storage
- Path: {state_dir}/projects/{basename}-{fnv1a_64(cwd)}/memories/*.md
- Linux: ~/.local/state/maki/ | macOS: ~/Library/Application Support/maki/
- Plain text files; no database
- Limits: 200 lines/file, 50 KB total/project; no subdirs

### Tools
1. memory tool (plugins/memory/init.lua:131) - view/write/delete/list commands
2. /memory command (plugins/memory/init.lua:188) - interactive TUI picker (Enter=editor, Ctrl+O=edit, Ctrl+D=delete)

### Prompt Integration
- Registers prompt_hint in after_instructions slot (init.lua:29)
- Lists filenames + sizes only; agent must call memory tool to read content
- Saves tokens vs injecting full content

### Security
- safe_resolve() in memory_helpers.lua:45 prevents path traversal
- Rejects absolute paths, null bytes, ../ attempts
- FNV-1a 64-bit hash in pure Lua (memory_helpers.lua:8)

### Relationships
- Independent of sessions (filesystem persistence, no coupling to conversation history)
- Independent of todos (todos = ephemeral within-session, memories = persistent project knowledge)
- Each session sees latest memories via prompt hint on prompt assembly