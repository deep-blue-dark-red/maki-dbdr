local ToolView = require("maki.tool_view")
local helpers = require("memory_helpers")
local ListPicker = require("maki.list_picker")
local TextInput = require("maki.text_input")
local Toast = require("maki.toast")

local WRITE_TOOLS = { "write", "edit", "multiedit", "edit_lines", "insert_lines", "ast_grep_replace", "create_plugin" }
local VIEW_PREF_FILE = "picker_view"

-- A toast and not a flash, because this feedback has to stay readable while
-- the picker is still covering the screen. Kept local on purpose: claiming the
-- global `maki.notify` slot here would put a "memory" toast in front of every
-- other plugin's notices too.
local function notify(msg)
  Toast.show(msg, { title = "memory" })
end

local function memories_path_suffix()
  local cwd = maki.uv.cwd()
  local root = maki.fs.root(cwd, ".git") or cwd
  return "projects/" .. helpers.project_id(root) .. "/memories"
end

local function legacy_dir_if_exists(suffix)
  local legacy = maki.env.legacy_dir()
  if not legacy then
    return nil
  end
  local dir = maki.fs.joinpath(legacy, suffix)
  local meta = maki.fs.metadata(dir)
  if meta and meta.is_dir then
    return dir
  end
end

-- Notes live outside cwd, where file-write tools normally prompt; pre-allow
-- them here so the agent can edit notes directly. Reads may come from the
-- legacy dir while writes go to the state dir, so cover both.
local function register_write_rules()
  local suffix = memories_path_suffix()
  local dirs = { legacy_dir_if_exists(suffix) }
  local state = maki.env.state_dir()
  if state then
    dirs[#dirs + 1] = maki.fs.joinpath(state, suffix)
  end
  for _, dir in ipairs(dirs) do
    for _, tool in ipairs(WRITE_TOOLS) do
      -- The edit sub-tools are opt-in, and a rule naming an unregistered tool
      -- is dropped with a warning. Ask first, or a default config logs that
      -- warning at every startup.
      if maki.api.get_tool(tool) then
        maki.api.register_permission_rule({ tool = tool, scope = dir .. "/**" })
      end
    end
  end
end
register_write_rules()

local function resolve_dir(check_legacy)
  local suffix = memories_path_suffix()
  if check_legacy then
    local dir = legacy_dir_if_exists(suffix)
    if dir then
      return dir
    end
  end
  local state = maki.env.state_dir()
  if not state then
    return nil, "cannot resolve state dir"
  end
  return maki.fs.joinpath(state, suffix)
end

maki.api.register_prompt_hint({
  prompt = "system",
  slot = "after_instructions",
  content = function()
    local dir = resolve_dir(true)
    if not dir then
      return nil
    end
    local tag_line = helpers.format_tag_line(dir, helpers.MAX_TAGS)
    if not tag_line then
      return nil
    end
    return "\n\nMemory tags (memory tool, `read tags=[...]`): " .. tag_line .. "\n"
  end,
})

maki.api.register_prompt_hint({
  slot = "tool_usage",
  content = "- Proactively save non-obvious project gotchas and architecture decisions to **memory**.",
})

local function render_content(content, path, ctx)
  local buf = maki.ui.buf()
  local tol = ctx:tool_output_lines()
  local view = ToolView.new(buf, {
    max_lines = (tol and tol.other) or 20,
    keep = "head",
  })
  buf:on("click", function()
    view:toggle()
  end)

  local ext = path:match("%.([^%.]+)$") or "md"
  if not view:set_highlight(content, ext) then
    view:append_text(content)
  end
  view:finish()
  return buf
end

local function cmd_read(path, dir, ctx)
  local file_path, err = helpers.safe_resolve(dir, path)
  if not file_path then
    return nil, err
  end
  local content, err = maki.fs.read(file_path)
  if not content then
    return nil, "read error: " .. err
  end
  local formatted =
    helpers.cap_read_output(helpers.format_read_entry(path, #content, content), helpers.CAP_HINT_REWRITE)
  return {
    llm_output = formatted,
    body = render_content(formatted, path, ctx),
  }
end

local function cmd_write(path, content, tags, dir, ctx)
  local file_path, err = helpers.safe_resolve(dir, path)
  if not file_path then
    return nil, err
  end

  local size_err = helpers.validate_write_size(content)
  if size_err then
    return nil, size_err
  end
  local normalized, tag_err, note = helpers.validate_write_tags(tags or {})
  if tag_err then
    return nil, tag_err
  end
  local full = helpers.encode_frontmatter(normalized) .. content
  maki.fs.mkdir(dir, { parents = true })
  local ok, write_err = maki.fs.write(file_path, full)
  if not ok then
    return nil, "write error: " .. tostring(write_err)
  end
  return {
    llm_output = "wrote "
      .. path
      .. " (tags: "
      .. (#normalized > 0 and table.concat(normalized, ", ") or "none")
      .. ")"
      .. (note and ("; " .. note) or ""),
    body = render_content(content, path, ctx),
  }
end

local function cmd_delete(path, dir)
  local file_path, err = helpers.safe_resolve(dir, path)
  if not file_path then
    return nil, err
  end
  if not maki.fs.metadata(file_path) then
    return nil, "'" .. path .. "' does not exist"
  end
  local ok, rm_err = maki.fs.rm(file_path)
  if not ok then
    return nil, "delete error: " .. tostring(rm_err)
  end
  return "deleted " .. path
end

local function cmd_move(path, new_path, dir)
  local ok, err = helpers.move_file(dir, path, new_path)
  if not ok then
    return nil, err
  end
  return "moved " .. path .. " -> " .. new_path
end

local function with_dir(res, dir)
  local prefix = "dir: " .. dir .. "\n\n"
  if type(res) == "string" then
    return prefix .. res
  end
  res.llm_output = prefix .. res.llm_output
  return res
end

maki.api.register_tool({
  name = "memory",
  description = "Save and retrieve concise project facts across sessions. Reuse relevant tags from the system prompt. "
    .. "Keep notes current; update or delete stale facts. list/read return the notes directory; "
    .. "use edit on <dir>/<path> for targeted updates.",

  schema = {
    type = "object",
    properties = {
      command = {
        type = "string",
        enum = { "list", "read", "write", "delete", "move" },
        description = "Action name only: list, read, write, delete, or move. Pass arguments in separate fields. "
          .. "list: optional tags, returns index. read: path or tags, returns bodies. "
          .. "write: path and content, optional tags, creates or overwrites. delete: path. "
          .. "move: path and new_path, renames a note; never overwrites.",
        required = true,
      },
      path = {
        type = "string",
        description = "Relative path, e.g. 'architecture.md'.",
      },
      new_path = {
        type = "string",
        description = "Target file name for move (notes are flat; must not exist).",
      },
      content = { type = "string", description = "Body for write (frontmatter added automatically)." },
      tags = {
        type = "array",
        items = { type = "string" },
        description = "snake_case tags. Filter for list/read; assigned on write (defaults to filename stem).",
      },
    },
  },

  header = function(input)
    local parts = { input.command or "" }
    if input.path then
      parts[#parts + 1] = input.path
    elseif input.tags then
      parts[#parts + 1] = table.concat(input.tags, ",")
    end
    if input.new_path then
      parts[#parts + 1] = "-> " .. input.new_path
    end
    return table.concat(parts, " ")
  end,

  restore = function(input, output, _is_error, ctx)
    local content = (input.command == "write" and input.content) or output
    return render_content(content, input.path or "memory.md", ctx)
  end,

  handler = function(input, ctx)
    if type(input.tags) == "string" then
      input.tags = { input.tags }
    end
    local verr = helpers.validate_input(input)
    if verr then
      return { llm_output = "error: " .. verr, is_error = true }
    end
    local cmd = input.command
    local dir, dir_err = resolve_dir(cmd == "list" or cmd == "read")
    if not dir then
      return { llm_output = "error: " .. dir_err, is_error = true }
    end

    local result, err
    if cmd == "list" then
      result, err = helpers.format_list(dir, input.tags)
    elseif cmd == "read" then
      if input.tags and #input.tags > 0 then
        result, err = helpers.format_read(dir, input.tags)
      else
        result, err = cmd_read(input.path, dir, ctx)
      end
    elseif cmd == "write" then
      result, err = cmd_write(input.path, input.content, input.tags, dir, ctx)
    elseif cmd == "delete" then
      result, err = cmd_delete(input.path, dir)
    elseif cmd == "move" then
      result, err = cmd_move(input.path, input.new_path, dir)
    end
    if err then
      return { llm_output = "error: " .. err, is_error = true }
    end
    if cmd == "list" or cmd == "read" then
      return with_dir(result, dir)
    end
    return result
  end,
})

local function flat_rows(files)
  local items = {}
  for i, f in ipairs(files) do
    items[i] = { label = f.name, detail = helpers.detail_parts(f.size, f.tags) }
  end
  return items
end

-- No tags on the row: the section header names the tag, and the tint shows
-- which other sections hold the same file.
local function grouped_rows(files)
  local items = {}
  for _, g in ipairs(helpers.group_by_tag(files)) do
    for _, f in ipairs(g.files) do
      items[#items + 1] = {
        label = f.name,
        detail = helpers.format_size(f.size),
        section = g.tag,
        section_detail = "(" .. #g.files .. ")",
      }
    end
  end
  return items
end

local VIEWS = {
  { id = "flat", rows = flat_rows },
  { id = "grouped", rows = grouped_rows },
}

-- Remembered across runs, so /memory opens the way you left it. A preference
-- and not project data, so it lives with maki's other state and not in the
-- memories directory.
local function view_pref_path()
  local state = maki.env.state_dir()
  return state and maki.fs.joinpath(state, "memory", VIEW_PREF_FILE)
end

local function load_view()
  local path = view_pref_path()
  local saved = path and maki.fs.read(path)
  for i, v in ipairs(VIEWS) do
    if v.id == saved then
      return i
    end
  end
  return 1
end

local function save_view(view)
  local path = view_pref_path()
  if not path then
    return
  end
  local _, err = maki.fs.mkdir(maki.fs.dirname(path), { parents = true })
  if not err then
    _, err = maki.fs.write(path, VIEWS[view].id)
  end
  if err then
    maki.log.warn("memory: cannot save the picker view to " .. path .. ": " .. tostring(err))
  end
end

-- One-line prompt in a small focused float, prefilled so a move is an edit of
-- the current name. Returns the trimmed text, or nil when dismissed.
local function prompt_name(title, initial)
  local input = TextInput.new()
  input:insert_text(initial)
  local buf = maki.ui.buf()
  local win = maki.ui.open_win(buf, {
    title = title,
    width = math.max(#title + 16, 44),
    height = 3,
    footer = { { "Enter", "move" }, { "Esc", "cancel" } },
  })

  local function draw(width)
    local r = input:render("> ", nil, width)
    buf:set_lines(r.lines)
  end
  draw(win.width)

  while true do
    local ev = win:recv()
    if not ev or ev.type == "close" then
      return nil
    end
    if ev.type == "resize" then
      draw(ev.width)
    elseif ev.type == "key" then
      if ev.key == "<Esc>" or ev.key == "<C-c>" then
        win:close()
        return nil
      elseif ev.key == "<CR>" then
        local name = input:value():match("^%s*(.-)%s*$")
        win:close()
        return name ~= "" and name or nil
      elseif input:handle_key(ev.key) ~= "ignored" then
        draw(win.width)
      end
    end
  end
end

maki.api.register_command({
  name = "/memory",
  description = "View, edit, move, and delete memory files",
  handler = function()
    local dir = resolve_dir(true)
    if not dir then
      maki.ui.flash("Cannot resolve memory directory")
      return
    end

    local view = load_view()
    -- The only place rows are built, so the two views can never disagree about
    -- what is on disk.
    local function build()
      local files, warnings = helpers.files_with_tags(dir)
      if #warnings > 0 then
        notify(#warnings .. " unreadable memory file(s)")
      end
      return VIEWS[view].rows(files)
    end

    local items = build()
    if #items == 0 then
      maki.ui.flash("No memories yet")
      return
    end
    local last_cursor = 1
    while true do
      local event = ListPicker.open(items, {
        title = " Memory Files ",
        cursor = last_cursor,
        key = function(item)
          return item.label
        end,
        submit_keys = { "<C-o>" },
        action_keys = { "<C-m>" },
        live_keys = {
          ["<Tab>"] = function()
            view = view % #VIEWS + 1
            save_view(view)
            return build()
          end,
        },
        footer = {
          { "Enter", "open" },
          { "Ctrl+O", "edit" },
          { "Ctrl+M", "move" },
          { "Ctrl+D", "delete" },
          { "Tab", "switch view" },
        },
      })

      if event.type == "close" then
        break
      end

      last_cursor = event.index
      if event.type == "choice" then
        local path = maki.fs.joinpath(dir, event.item.label)
        if maki.ui.open_editor(path) == 0 then
          items = build()
        end
      elseif event.type == "delete" then
        local ok, err = maki.fs.rm(maki.fs.joinpath(dir, event.item.label))
        if ok then
          notify("Deleted " .. event.item.label)
          items = build()
          if #items == 0 then
            break
          end
        else
          notify("Delete failed: " .. tostring(err))
        end
      elseif event.type == "key" and event.key == "<C-m>" then
        local label = event.item and event.item.label
        local new_name = label and prompt_name(" Move " .. label .. " to ", label)
        if new_name and new_name ~= label then
          local ok, err = helpers.move_file(dir, label, new_name)
          if ok then
            notify("Moved " .. label .. " -> " .. new_name)
          else
            notify("Move failed: " .. tostring(err))
          end
        end
        items = build()
      else
        break
      end
    end
  end,
})
