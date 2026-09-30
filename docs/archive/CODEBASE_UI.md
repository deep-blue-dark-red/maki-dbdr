# Codebase: UI — Elm Architecture, Event Loop, Components, Agent Integration

## Elm-Style Architecture

**`maki-ui/src/app/mod.rs`** — `App`, `Msg`, `update()`.

Pure unidirectional data flow:
1. `Msg` enters `App::update(Msg) -> Vec<Action>`
2. `Action`s dispatched by `EventLoop::handle_action()` on main thread
3. `terminal.draw()` renders `App::view()` each cycle

### Messages

```rust
pub enum Msg {
    Key(KeyEvent),
    Paste(String),
    Mouse(MouseEvent),
    Scroll { column, row, delta },
    Agent(Box<Envelope>),
}
```

Five types: keyboard, paste, mouse, scroll, agent events from async loop.

### `App::update()`

Handles:
- `Msg::Key` → overlay routing (`dispatch_overlay`), ctrl shortcuts, plugin keymaps, input box
- `Msg::Agent` → `handle_agent_event()` — updates chats, tracks timing, handles subagents, detects Done/Error
- `Msg::Mouse` / `Msg::Scroll` → position-based routing to whichever overlay is under cursor

### Actions

Side effects from `update()`, dispatched by `EventLoop`:
- `SendMessage(Box<AgentInput>)`
- `CancelAgent { run_id }` / `CancelSubagent { tool_use_id }`
- `ChangeModel(String)`
- `Compact`
- `ShellCommand { id, command, visible }`
- `OpenEditor(PathBuf)`
- `Suspend`

### Overlay System

14 overlay components, all implement `Overlay` trait. `dispatch_overlay()` routes keys/pastes by priority:
permission prompt → plan form → help → btw → floats → search → pickers.

Key overlays:
- `permission_prompt` — tool permission decision UI
- `plan_form` — plan mode creation
- `help_modal`
- `btw_modal` — background task notifications
- `lua_float` — Lua popup rendering (1939 lines)
- `search_modal` — text search
- Pickers: session, model, theme, mcp, settings, file, login

## Event Loop

**`maki-ui/src/event_loop.rs`** — main thread, controls terminal and terminal I/O.

```
loop {
    tick()           // animation, timers, status bar
    drain_channels() // non-blocking recv from agent, shell, warnings
    terminal.draw()  // render app.view()
    check exit       // ExitRequest
    poll_and_handle_input() // crossterm event poll
}
```

- Poll interval adapts: **0ms when agent is active**, **16ms during animation**, **100ms idle**
- Scroll/drag events **coalesced** via `aggregate_scroll` and `coalesce_drag`

### `EventLoop::handle_action()`

Processes side effects from `update()`:
- `SendMessage` → pushes `QueueItem::Message` to agent queue
- `CancelAgent` → sends cancel signal
- `ChangeModel` → updates model slot
- `Compact` → pushes `QueueItem::Compact` to agent queue
- `ShellCommand` → spawns shell process
- `OpenEditor` → opens external editor with temp file

## Agent Integration (UI side)

**`maki-ui/src/agent/agent_loop.rs`** — bridges UI and agent crate.

Agent runs in a **separate thread** on a `smol` executor. Communication via `flume` channels:
- **UI → Agent:** `QueueReceiver<QueueItem>` (Message / Compact / Interrupt)
- **Agent → UI:** `flume::Sender<Envelope>` (AgentEvent + run_id + subagent)

`run_id: u64` incrementing counter. Stale events (cancel) are dropped:
```
envelope.run_id != app.current_run_id → drop
```

Flow for user input:
1. User types in `input_box`, presses Enter
2. `handle_submit()` → `Action::SendMessage`
3. `EventLoop::handle_action()` → pushes `QueueItem::Message` to agent queue
4. `AgentLoop::process_entry()` → runs `Agent::run()`, streaming events back
5. Events arrive as `Msg::Agent` → `App::update()` → updates `Chat` state
6. Next `terminal.draw()` renders new state

**`command_router.rs`** — routes keyboard commands to overlay/components.
**`cancel_map.rs`** — tracks active cancel tokens.
**`shared_queue.rs`** — shared queue between UI and agent threads.

## Components

**`maki-ui/src/components/`** — UI building blocks.

### Messages (Chat Rendering)

**`messages/mod.rs`** (1555 lines) — chat message list, rendering state, chat management.
**`messages/render.rs`** — render each message (user/assistant).
**`messages/segment.rs`** — segment types (text, tool_use, tool_result).
**`messages/selection.rs`** — text selection within messages.
**`messages/tests.rs`** (1515 lines) — comprehensive rendering tests.

### `tool_display.rs` (2024 lines)

Renders tool output in TUI. Handles all `ToolOutput` variants:
- ReadCode → syntax-highlighted code view
- Diff → unified diff with hunks
- GrepResult → matched files and lines
- TodoList → todo items with status markers
- Batch → status-tracked batch tool entries
- WriteCode → confirmation with byte count
- Plain/Markdown → text or rendered markdown
- Instructions → instruction block display

### `lua_float.rs` (1939 lines)

Lua popup rendering: buffers, windows, click handlers, form inputs, splits.

### Other Components

- `input.rs` (1159 lines) — input box with autocomplete, key handling
- `code_view.rs` — syntax-highlighted code viewer
- `command.rs` — command palette
- `help_modal.rs` — help popup
- `search_modal.rs` — text search
- `permission_prompt.rs` — permission decision UI
- `split_layout.rs` — split panes
- `status_bar.rs` — bottom status bar
- `streaming_content.rs` — live streaming text
- `list_picker.rs` (1227 lines) — list-based pickers (file, session, model, etc.)
- `form.rs` — form inputs
- `keybindings.rs` — keybinding display
- `mcp_picker.rs`, `model_picker.rs`, `theme_picker.rs`, `session_picker.rs`, `settings_picker.rs`, `login_picker.rs` — various pickers
- `file_picker.rs` — file selection
- `render_hints.rs` — render hints
- `scrollbar.rs` — scroll indicators

## Terminal & Rendering

**`terminal.rs`** — crossterm setup, enter/exit alt screen, cursor management.
**`render_worker.rs`** — background render tasks (markdown, code highlighting).
**`markdown.rs`** — markdown rendering.
**`highlight.rs`** — syntax highlighting via syntect.
**`selection.rs`** (1266 lines) — text selection and copy.

## Themes

**`maki-ui/src/themes/`** — 22 TOML theme files defining color schemes:
ayu_dark, carbonfox, catppuccin variants, dracula, everforest, fleet_dark, github_dark, gruvbox, kanagawa, material_darker, monokai_pro, night_owl, nightfox, nord, onedark, rose_pine, solarized, tokyonight, vscode_dark_plus, zenburn.

**`theme.rs`** — theme loading, color resolution, baked snapshots via `theme_gen`.

## Shared State

- `Arc<ArcSwap<...>>` for zero-copy reads across threads (model slot, shared history)
- `StorageWriter` — async-persisted disk writes (deferred to avoid blocking UI)
- `SharedBuf` — append-only dirty-flag buffer for streaming tool output (same type as in agent crate, shared via Arc)
