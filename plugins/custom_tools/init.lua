local ToolView = require("maki.tool_view")
local helpers = require("custom_tools_helpers")
local ListPicker = require("maki.list_picker")
local Toast = require("maki.toast")

local VIEW_PREF_FILE = "picker_view"
local DESCRIPTION = "Persist helper tools across sessions: scripts (bash, python) or small projects in any language, built "
  .. "wherever you like inside the project with write/edit, then registered here so later sessions can find and run them "
  .. "from bash. When you are about to write a script for a task you may face again (project-specific "
  .. "build/test/search/git chores), build it in the project, register it once, and reuse it instead of reinventing it. "
  .. "Before writing a new helper, `list` existing ones to avoid duplicates; `info` shows metadata and help without the "
  .. "source; `read` returns the source. A file tool must run from bash, support `--help`, and document its arguments. "
  .. "A command tool is defined by its `body` and needs no project file. Register one only when it saves tokens and will "
  .. "be reused in later sessions or more than three times in this session; do not build tools for their own sake. "
  .. "list: optional tags, query, and group_by='tag'; returns one row per tool by default. info: path, tags, or query; returns "
  .. "description, tags, and help or the command body without the source. read: path or tags; returns source or command body. "
  .. "write: registers an existing project file (path relative to the project root); omitted tags are derived from the "
  .. "path and an omitted description from the leading comment; it verifies `--help` and warns on failure. Pass `body` to "
  .. "define a command tool instead: the body is a shell command and no project file is required. Re-registering updates "
  .. "tags/description/body without re-sending the file. delete: unregisters; the file "
  .. "stays. Edit a registered tool directly with write/edit; re-register only to change tags or description."

local function notify(msg)
  Toast.show(msg, { title = "custom_tools" })
end

-- Tool files live wherever the agent built them in the project; only the
-- registry (paths, tags, descriptions) lives in the state dir, keyed on the
-- project root the same way other per-project state is.
local function project_root()
  local cwd = maki.uv.cwd()
  return maki.fs.root(cwd, ".git") or cwd
end

local function resolve_meta_dir()
  local state = maki.env.state_dir()
  if not state then
    return nil, "cannot resolve state dir"
  end
  return maki.fs.joinpath(state, "projects", helpers.project_id(project_root()), "custom_tools")
end

maki.api.register_prompt_hint({
  prompt = "system",
  slot = "after_instructions",
  content = function()
    local meta_dir = resolve_meta_dir()
    if not meta_dir then
      return nil
    end
    local tag_line = helpers.format_tag_line(meta_dir, project_root(), helpers.MAX_TAGS)
    if not tag_line then
      return nil
    end
    return "\n\nCustom tool tags (custom_tools tool, `read tags=[...]`): " .. tag_line .. "\n"
  end,
})

maki.api.register_prompt_hint({
  slot = "conventions",
  content = "- Check **custom_tools** before writing a helper; scripts (bash, python) or small projects in any language live in "
    .. "the project tree and are registered with **custom_tools** so they persist across sessions and run directly from "
    .. "bash. Register one only when it saves tokens and will be reused in later sessions or more than three times in "
    .. "this session. File tools must support `--help` and document their arguments; command tools carry their shell body "
    .. "in the registry.",
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

local function cmd_read(path, root, meta_dir, ctx)
  local tags, description, command_body, warn = helpers.metadata_for(meta_dir, path)
  local content = command_body
  local size = "command"
  if not content then
    local file_path, err = helpers.safe_resolve(root, path)
    if not file_path then
      return nil, err
    end
    local read_err
    content, read_err = maki.fs.read(file_path)
    if not content then
      return nil, "read error: " .. tostring(read_err)
    end
    size = #content
  end

  local formatted = helpers.format_read_entry(path, size, content, tags, description)
  formatted = helpers.cap_read_output(formatted, helpers.CAP_HINT_REWRITE)
  if warn then
    formatted = "warning: " .. warn .. "\n\n" .. formatted
  end
  return {
    llm_output = formatted,
    body = render_content(formatted, command_body and "tool.txt" or path, ctx),
  }
end

local function cmd_write(path, tags, description, input_body, root, meta_dir)
  local file_path, err = helpers.safe_resolve(root, path)
  if not file_path then
    return nil, err
  end

  local existing, read_err = helpers.read_meta(meta_dir, path)
  if read_err then
    return nil, read_err
  end

  local has_body = type(input_body) == "string" and input_body ~= ""
  local clearing_body = input_body == ""
  local is_command = has_body or (existing and existing.body ~= nil and not clearing_body)
  if not is_command and not maki.fs.metadata(file_path) then
    return nil,
      "'"
        .. path
        .. "' does not exist in the project; create it in the project first, or pass body to define a command tool"
  end

  local final_tags = tags
  if final_tags == nil and not existing then
    final_tags = helpers.derive_tags(path)
  end

  local final_description = description
  local derived_description = false
  if final_description == nil and (not existing or not existing.description) then
    if has_body then
      final_description = helpers.derive_description(input_body)
      derived_description = final_description ~= nil
    else
      local content, content_err = maki.fs.read(file_path)
      if content then
        final_description = helpers.derive_description(content)
        derived_description = final_description ~= nil
      elseif content_err then
        return nil, "read error: " .. tostring(content_err)
      end
    end
  end

  local merged, merge_err = helpers.merge_meta(existing, final_tags, final_description, input_body)
  if merge_err then
    return nil, merge_err
  end

  local exec_ok, exec_err, help, help_err
  if not merged.body then
    exec_ok, exec_err = helpers.make_executable(file_path)
    help, help_err = helpers.capture_help(file_path)
  end
  local meta_ok, meta_err = helpers.write_meta(meta_dir, path, merged.tags, merged.description, merged.body)
  if not meta_ok then
    return nil, tostring(meta_err)
  end

  local action = existing and "updated registration for " or (merged.body and "defined " or "registered ")
  local parts = {
    action .. path,
    "tags: "
      .. (#merged.tags > 0 and table.concat(merged.tags, ", ") or "none")
      .. (merged.note and ("; " .. merged.note) or ""),
  }
  if derived_description then
    parts[#parts + 1] = "description derived from the leading comment"
  end
  if exec_ok == nil then
    parts[#parts + 1] = "executable bit not set: " .. tostring(exec_err)
  end
  if help == nil and not merged.body then
    parts[#parts + 1] = "--help check failed: " .. tostring(help_err)
  elseif help_err then
    parts[#parts + 1] = "--help check failed: " .. help_err
  end
  return table.concat(parts, "; ")
end

local function format_help(path)
  local help, err = helpers.capture_help(path)
  if not help then
    return "--help unavailable: " .. tostring(err)
  end
  local out = helpers.cap_help(help)
  if err then
    out = out .. "\n(--help " .. err .. ")"
  end
  return out
end

local function info_entry(name, root, meta_dir)
  local tags, description, command_body, warn = helpers.metadata_for(meta_dir, name)
  if command_body then
    return {
      name = name,
      size = "command",
      tags = tags,
      description = description,
      body = command_body,
      warn = warn,
    }
  end

  local file_path, err = helpers.safe_resolve(root, name)
  if not file_path then
    return nil, err
  end
  local meta = maki.fs.metadata(file_path)
  if not meta then
    return nil, "'" .. name .. "' does not exist in the project"
  end
  return {
    name = name,
    path = file_path,
    size = meta.size,
    tags = tags,
    description = description,
    warn = warn,
  }
end

local function cmd_info(input, root, meta_dir, ctx)
  local entries = {}
  local tag_warning, unreadable

  if input.path then
    local entry, err = info_entry(input.path, root, meta_dir)
    if not entry then
      return nil, err
    end
    entries[1] = entry
    if entry.warn then
      tag_warning = "warning: " .. entry.name .. ": " .. entry.warn
    end
  else
    local want, warning, werr
    if input.tags and #input.tags > 0 then
      want, warning, werr = helpers.normalize_want(input.tags)
      if werr then
        return nil, werr
      end
    end
    tag_warning = warning
    local files, warnings = helpers.files_with_tags(meta_dir, root)
    unreadable = helpers.combine_unreadable(warnings)
    for _, f in ipairs(helpers.filter_files(files, want, input.query)) do
      entries[#entries + 1] = {
        name = f.name,
        path = f.body and nil or maki.fs.joinpath(root, f.name),
        size = f.body and "command" or f.size,
        tags = f.tags,
        description = f.description,
        body = f.body,
      }
    end
    if #entries == 0 then
      return { llm_output = helpers.NO_MATCH_MSG }
    end
  end

  local parts = {}
  for _, e in ipairs(entries) do
    local detail = e.body and ("command:\n" .. e.body) or format_help(e.path)
    parts[#parts + 1] = helpers.format_info_entry(e.name, e.size, e.tags, e.description, detail)
  end
  local text = table.concat(parts, "\n\n")
  local prefix = {}
  if tag_warning then
    prefix[#prefix + 1] = tag_warning
  end
  if unreadable then
    prefix[#prefix + 1] = unreadable
  end
  if #prefix > 0 then
    text = table.concat(prefix, "\n\n") .. "\n\n" .. text
  end
  return {
    llm_output = text,
    body = render_content(text, input.path or "info.txt", ctx),
  }
end

local function cmd_delete(path, meta_dir)
  local _, _, body = helpers.metadata_for(meta_dir, path)
  local ok, err = helpers.remove_tool(meta_dir, path)
  if not ok then
    return nil, err
  end
  return "unregistered " .. path .. (body and "" or " (file left in place)")
end

local function with_root(res, root)
  local prefix = "project root: " .. root .. "\n\n"
  if type(res) == "string" then
    return prefix .. res
  end
  res.llm_output = prefix .. res.llm_output
  return res
end

maki.api.register_tool({
  name = "custom_tools",
  description = DESCRIPTION,

  schema = {
    type = "object",
    properties = {
      command = {
        type = "string",
        enum = { "list", "read", "info", "write", "delete" },
        description = "Action name only: list, read, info, write, or delete. Pass arguments in separate fields. "
          .. "list: optional tags, query, and group_by; one row per tool by default. read: tags (all matching sources) "
          .. "or path (one tool); tags win. info: path, tags, or query; metadata and help or the command body without the "
          .. "source. write: registers an existing project file, defines a command tool when body is set, or updates its "
          .. "tags/description/body. delete: unregisters; the file stays.",
        required = true,
      },
      path = {
        type = "string",
        description = "Path relative to the project root, e.g. 'scripts/commit-helper.sh'.",
      },
      description = {
        type = "string",
        description = "One-line summary of what the tool does and when to use it. Optional on first write: derived from the file's leading comment when omitted.",
      },
      tags = {
        type = "array",
        items = { type = "string" },
        description = "snake_case categories. Filter for list/read/info; derived from the path when omitted on write.",
      },
      body = {
        type = "string",
        description = "write: shell command defining a command tool; no project file is required. Omit to keep an existing body or register a file.",
      },
      query = {
        type = "string",
        description = "list/info: case-insensitive substring filter over path, description, and tags.",
      },
      group_by = {
        type = "string",
        enum = { "tool", "tag" },
        description = "list: 'tool' (default) shows one row per tool; 'tag' groups rows under tag sections.",
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
    return table.concat(parts, " ")
  end,

  restore = function(input, output, _is_error, ctx)
    local text = type(output) == "table" and output.llm_output or output
    return render_content(text or "", input.path or "tool.sh", ctx)
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
    local root = project_root()
    local meta_dir, dir_err = resolve_meta_dir()
    if not meta_dir then
      return { llm_output = "error: " .. dir_err, is_error = true }
    end

    local result, err
    if cmd == "list" then
      result, err = helpers.format_list(meta_dir, root, input.tags, input.query, input.group_by)
    elseif cmd == "read" then
      if input.tags and #input.tags > 0 then
        result, err = helpers.format_read(meta_dir, root, input.tags)
      else
        result, err = cmd_read(input.path, root, meta_dir, ctx)
      end
    elseif cmd == "info" then
      result, err = cmd_info(input, root, meta_dir, ctx)
    elseif cmd == "write" then
      result, err = cmd_write(input.path, input.tags, input.description, input.body, root, meta_dir)
    elseif cmd == "delete" then
      result, err = cmd_delete(input.path, meta_dir)
    end
    if err then
      return { llm_output = "error: " .. err, is_error = true }
    end
    if cmd == "list" or cmd == "read" or cmd == "info" or cmd == "write" then
      return with_root(result, root)
    end
    return result
  end,
})

local function flat_rows(files)
  local items = {}
  for i, f in ipairs(files) do
    items[i] = {
      label = f.name,
      detail = helpers.detail_parts(f.body and "command" or f.size, f.description, f.tags),
      body = f.body,
    }
  end
  return items
end

-- No tags on the row: the section header names the tag, and the tint shows
-- which other sections hold the same tool.
local function grouped_rows(files)
  local items = {}
  for _, g in ipairs(helpers.group_by_tag(files)) do
    for _, f in ipairs(g.files) do
      items[#items + 1] = {
        label = f.name,
        detail = helpers.detail_parts(f.body and "command" or f.size, f.description, nil),
        section = g.tag,
        section_detail = "(" .. #g.files .. ")",
        body = f.body,
      }
    end
  end
  return items
end

local VIEWS = {
  { id = "flat", rows = flat_rows },
  { id = "grouped", rows = grouped_rows },
}

-- Remembered across runs, so /custom_tools opens the way you left it. A
-- preference and not project data, so it lives with maki's other state and not
-- in the tools directory.
local function view_pref_path()
  local state = maki.env.state_dir()
  return state and maki.fs.joinpath(state, "custom_tools", VIEW_PREF_FILE)
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
    maki.log.warn("custom_tools: cannot save the picker view to " .. path .. ": " .. tostring(err))
  end
end

local function paste_tool(item)
  local text
  if item.body then
    text = "!" .. (item.body:gsub("^%s+", ""):gsub("%s+$", ""))
  else
    text = "!" .. item.label
  end
  local st, read_err = maki.ui.input()
  if not st then
    maki.ui.flash("Cannot read input: " .. tostring(read_err))
    return
  end
  local ok, edit_err = maki.ui.input_edit({
    start = st.cursor,
    stop = st.cursor,
    text = text,
    version = st.version,
    session_id = st.session_id,
  })
  if not ok then
    maki.ui.flash("Cannot paste: " .. tostring(edit_err))
  end
end

maki.api.register_command({
  name = "/custom_tools",
  description = "View, paste, edit, and unregister custom tools",
  handler = function()
    local meta_dir = resolve_meta_dir()
    if not meta_dir then
      maki.ui.flash("Cannot resolve custom tools registry")
      return
    end
    local root = project_root()

    local view = load_view()
    -- The only place rows are built, so the two views can never disagree about
    -- what is registered.
    local function build()
      local files, warnings = helpers.files_with_tags(meta_dir, root)
      if #warnings > 0 then
        notify(#warnings .. " unreadable custom tool registration(s)")
      end
      return VIEWS[view].rows(files)
    end

    local items = build()
    if #items == 0 then
      maki.ui.flash("No custom tools yet")
      return
    end
    local last_cursor = 1
    while true do
      local event = ListPicker.open(items, {
        title = " Custom Tools ",
        cursor = last_cursor,
        key = function(item)
          return item.label
        end,
        submit_keys = {},
        action_keys = { "<C-o>", "<C-e>" },
        live_keys = {
          ["<Tab>"] = function()
            view = view % #VIEWS + 1
            save_view(view)
            return build()
          end,
        },
        footer = {
          { "Enter", "paste !cmd" },
          { "Ctrl+O", "edit" },
          { "Ctrl+E", "edit metadata" },
          { "Ctrl+D", "unregister" },
          { "Tab", "switch view" },
        },
      })

      if event.type == "close" then
        break
      end

      last_cursor = event.index
      if event.type == "choice" then
        paste_tool(event.item)
        break
      elseif event.type == "delete" then
        local ok, err = helpers.remove_tool(meta_dir, event.item.label)
        if ok then
          notify("Unregistered " .. event.item.label)
          items = build()
          if #items == 0 then
            break
          end
        else
          notify("Unregister failed: " .. tostring(err))
        end
      elseif event.type == "key" and event.key == "<C-o>" then
        local path = event.item.body and helpers.meta_path(meta_dir, event.item.label)
          or maki.fs.joinpath(root, event.item.label)
        if maki.ui.open_editor(path) == 0 then
          items = build()
        end
      elseif event.type == "key" and event.key == "<C-e>" then
        local path = helpers.meta_path(meta_dir, event.item.label)
        if maki.ui.open_editor(path) == 0 then
          items = build()
        end
      else
        break
      end
    end
  end,
})
