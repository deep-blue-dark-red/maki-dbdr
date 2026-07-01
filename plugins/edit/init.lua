local shorten_path = require("maki.shorten_path")
local ToolView = require("maki.tool_view")
local fuzzy_replace = require("maki.fuzzy_replace")

local EDIT_DESCRIPTION = [[Replace an exact string in a file. old_string must be unique (or set replace_all). Read the file first; exclude the line-number prefix from read output when copying. Cheaper than write for targeted changes.]]

local MULTIEDIT_DESCRIPTION = [[Several exact-string replacements in one file, applied in order, all-or-nothing. Read the file first. Order edits so an earlier one doesn't alter text a later one matches on.]]

local function edit_header(input)
  local buf = maki.ui.buf()
  buf:line({ { shorten_path(input.path or ""), "path" } })
  return buf
end

local function edit_restore(_input, output, _is_error, _ctx)
  return ToolView.restore(output, { max_lines = 0 })
end

local function apply_edit(path, ctx, transform)
  path = maki.fs.abspath(path)

  local ok, err = ctx:check_before_edit(path)
  if not ok then
    return nil, err
  end

  local before, read_err = maki.fs.read(path)
  if read_err then
    return nil, "read error: " .. tostring(read_err)
  end

  local after, transform_err = transform(before)
  if transform_err then
    return nil, transform_err
  end

  local _, write_err = maki.fs.write(path, after)
  if write_err then
    return nil, "write error: " .. tostring(write_err)
  end

  ctx:record_read(path)

  return {
    path = path,
    before = before,
    after = after,
  }
end

local function diff_result(edit_result, summary)
  return {
    llm_output = summary,
    diff_path = edit_result.path,
    diff_before = edit_result.before,
    diff_after = edit_result.after,
    written_path = edit_result.path,
  }
end

maki.api.register_tool({
  name = "edit",
  kind = "edit",
  mutable_path = "path",
  permission_scope = "path",
  audiences = { "main", "general_sub", "interpreter" },
  description = EDIT_DESCRIPTION,

  schema = {
    type = "object",
    properties = {
      path = {
        type = "string",
        description = "Absolute path to the file",
        required = true,
        alias = "file_path",
      },
      old_string = {
        type = "string",
        description = "Exact string to find (must match uniquely unless replace_all is true)",
        required = true,
      },
      new_string = {
        type = "string",
        description = "Replacement string",
        required = true,
      },
      replace_all = {
        type = "boolean",
        description = "Replace all occurrences (default false)",
      },
    },
  },

  header = edit_header,
  restore = edit_restore,

  handler = function(input, ctx)
    local result, err = apply_edit(input.path, ctx, function(content)
      return fuzzy_replace.replace(content, input.old_string, input.new_string, input.replace_all or false)
    end)
    if not result then
      return { llm_output = err, is_error = true }
    end

    return diff_result(result, "edited " .. shorten_path(result.path))
  end,
})

maki.api.register_tool({
  name = "multiedit",
  kind = "edit",
  mutable_path = "path",
  permission_scope = "path",
  start_annotation = "edits",
  audiences = { "main", "general_sub", "interpreter" },
  description = MULTIEDIT_DESCRIPTION,

  schema = {
    type = "object",
    properties = {
      path = {
        type = "string",
        description = "Absolute path to the file",
        required = true,
        alias = "file_path",
      },
      edits = {
        type = "array",
        description = "Array of edit operations to apply sequentially",
        required = true,
        items = {
          type = "object",
          properties = {
            old_string = {
              type = "string",
              description = "Exact string to find",
              required = true,
            },
            new_string = {
              type = "string",
              description = "Replacement string",
              required = true,
            },
            replace_all = {
              type = "boolean",
              description = "Replace all occurrences (default false)",
            },
          },
        },
      },
    },
  },

  header = edit_header,
  restore = edit_restore,

  handler = function(input, ctx)
    local edits = input.edits
    if #edits == 0 then
      return { llm_output = "provide at least one edit", is_error = true }
    end

    local result, err = apply_edit(input.path, ctx, function(content)
      for i, edit in ipairs(edits) do
        local replaced, replace_err =
          fuzzy_replace.replace(content, edit.old_string, edit.new_string, edit.replace_all or false)
        if replace_err then
          return nil, string.format("edit %d: %s", i - 1, replace_err)
        end
        content = replaced
      end
      return content
    end)
    if not result then
      return { llm_output = err, is_error = true }
    end

    local n = #edits
    local s = n == 1 and "" or "s"
    return diff_result(result, string.format("applied %d edit%s to %s", n, s, shorten_path(result.path)))
  end,
})
