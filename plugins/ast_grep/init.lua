local truncate = require("maki.truncate")
local ToolView = require("maki.tool_view")
local shorten_path = require("maki.shorten_path")
local output_limits = require("maki.output_limits")
local core = require("astgrep")

local VERSION_TIMEOUT_MS = 2000
local MAX_LIMIT = 1000
local SEARCH_TOL = "grep"
local SEARCH_TOL_FALLBACK = 3
local REPLACE_TOL = "write"
local REPLACE_TOL_FALLBACK = 7
local ARROW_PREFIX = "-> "
local ERROR_NODE_NOTE = "Pattern contains an ERROR node"

local opts = maki.api.register_options(output_limits.extend({
  search_result_limit = {
    default = 100,
    min = 10,
    desc = "Max matches per search. A call's `limit` param overrides it.",
  },
  timeout_secs = {
    default = 60,
    min = 5,
    desc = "Kill the ast-grep run after this many seconds. A call's `timeout` param overrides it.",
  },
  max_line_bytes = { default = 500, min = 80, desc = "Truncate displayed match lines longer than this many bytes." },
}))

local bin_ready = false

--- ast-grep is an external binary, so a machine without it gets one clear
--- error instead of a spawn failure per call. Only a working probe is cached:
--- installing it mid-session then costs one more `--version`.
local function ast_grep_ready()
  if bin_ready then
    return true
  end
  local ok, id = pcall(maki.fn.jobstart, { core.BIN, "--version" })
  if not ok then
    return false
  end
  local result = maki.fn.jobwait(id, VERSION_TIMEOUT_MS)
  if not result then
    maki.fn.jobstop(id)
    return false
  end
  bin_ready = result.exit_code == 0
  return bin_ready
end

local function trim(text)
  return (text:gsub("^%s+", ""):gsub("%s+$", ""))
end

local function run_ast_grep(args, timeout_ms)
  local ok, id = pcall(maki.fn.jobstart, args)
  if not ok then
    return nil, "error: could not start ast-grep: " .. tostring(id)
  end
  local result = maki.fn.jobwait(id, timeout_ms)
  if not result then
    maki.fn.jobstop(id)
    return nil, string.format("error: ast-grep timed out after %d ms", timeout_ms)
  end
  if result.exit_code ~= 0 then
    local stdout = result.stdout or ""
    -- grep convention: exit 1 with a match array on stdout means "no matches",
    -- which the handlers turn into a plain No matches reply.
    if result.exit_code == 1 and stdout:find("^%s*%[") then
      return result
    end
    local stderr = result.stderr or ""
    local detail = trim(stderr ~= "" and stderr or stdout)
    return nil,
      string.format("error: ast-grep failed (exit %d): %s", result.exit_code, detail ~= "" and detail or "no output")
  end
  return result
end

--- Warnings (a pattern ast-grep could not parse, a "matched nothing" hint)
--- come back on stderr with exit 0, so they ride along after the matches.
local function with_stderr(text, stderr)
  local warnings = trim(stderr or "")
  if warnings == "" then
    return text
  end
  return text .. "\n" .. warnings
end

local function view_opts(ctx, tol_field, fallback)
  local tol = ctx:tool_output_lines()
  return { max_lines = (tol and tol[tol_field]) or fallback, keep = "head" }
end

local function apply_highlights(tasks, view)
  for _, task in ipairs(tasks) do
    local texts = {}
    for _, line in ipairs(task.lines) do
      texts[#texts + 1] = line.text
    end
    local highlighted = maki.ui.highlight(table.concat(texts, "\n"), task.ext, { independent = true })
    if highlighted then
      for i, line in ipairs(task.lines) do
        local spans = highlighted[i]
        if spans then
          local rebuilt = { view.all_lines[line.idx][1] }
          for _, span in ipairs(spans) do
            rebuilt[#rebuilt + 1] = span
          end
          view:update_line(line.idx, rebuilt)
        end
      end
    end
  end
  view:flush()
end

local function build_view(entries, ctx, tol_field, fallback)
  local buf = maki.ui.buf()
  local view = ToolView.new(buf, view_opts(ctx, tol_field, fallback))

  local max_nr = 0
  for _, entry in ipairs(entries) do
    for _, row in ipairs(entry.rows) do
      if row.kind == "line" and row.nr > max_nr then
        max_nr = row.nr
      end
    end
  end
  local nr_fmt = ToolView.line_nr_fmt(max_nr) .. " "

  local hl_tasks = {}
  for _, entry in ipairs(entries) do
    if #entries > 1 then
      view:append({ { shorten_path(entry.path), "path" } })
    end

    local lines = {}
    for _, row in ipairs(entry.rows) do
      if row.kind == "meta" then
        view:append({ { row.text, "dim" } })
      elseif row.kind == "new" then
        view:append({ { ARROW_PREFIX, "line_nr" }, { row.text } })
        lines[#lines + 1] = { idx = #view.all_lines, text = row.text }
      else
        view:append({ { string.format(nr_fmt, row.nr), "line_nr" }, { row.text } })
        lines[#lines + 1] = { idx = #view.all_lines, text = row.text }
      end
    end

    if #lines > 0 then
      hl_tasks[#hl_tasks + 1] = { ext = entry.path:match("%.([^%.]+)$") or "", lines = lines }
    end
  end

  view:finish()
  if #hl_tasks > 0 then
    maki.async.run(function()
      apply_highlights(hl_tasks, view)
    end)
  end

  buf:on("click", function()
    view:toggle()
  end)
  return buf
end

local function build_header(input, show_rewrite)
  local buf = maki.ui.buf()
  local spans = {}
  if input.lang then
    spans[#spans + 1] = { input.lang, "dim" }
    spans[#spans + 1] = { " " }
  end
  spans[#spans + 1] = { input.pattern or input.kind or "", "tool" }
  if show_rewrite and input.rewrite then
    spans[#spans + 1] = { " " .. ARROW_PREFIX, "dim" }
    spans[#spans + 1] = { input.rewrite, "tool" }
  end
  if input.include then
    spans[#spans + 1] = { " [" .. input.include .. "]", "dim" }
  end
  if input.path then
    spans[#spans + 1] = { " " .. shorten_path(input.path), "path" }
  end
  buf:line(spans)
  return buf
end

local function resolve_limit(input)
  return math.min(math.max(input.limit or opts.search_result_limit, 1), MAX_LIMIT)
end

local function resolve_timeout_ms(input)
  return math.max(input.timeout or opts.timeout_secs, 1) * 1000
end

local function error_output(message)
  return { llm_output = message, is_error = true }
end

--- A pattern ast-grep could not fully parse matches nothing and says so on
--- stderr; answer as an error so the pattern gets fixed, not read as an
--- honest empty search.
local function no_match_output(result)
  local stderr = result.stderr or ""
  if stderr:find(ERROR_NODE_NOTE, 1, true) then
    return error_output(trim(stderr))
  end
  return { llm_output = with_stderr(core.NO_MATCHES, stderr) }
end

--- Parses the run's matches, or the (nil, nil, why) an error response needs.
local function matches_of(result, input)
  local entries, total, err = core.parse_matches(result.stdout, resolve_limit(input), opts.max_line_bytes)
  if not entries then
    return nil, nil, "error: " .. err
  end
  return entries, total, nil
end

maki.api.register_prompt_hint({
  slot = "tool_usage",
  content = "- Use the **ast_grep** tool for structural search by code pattern (metavariables like `$F`), and **ast_grep_replace** to rewrite those matches. Prefer **grep** for plain text/regex search.",
})

maki.api.register_tool({
  name = "ast_grep",
  kind = "search",
  description = [[Search code by syntax structure. Supply pattern (code with metavariables, e.g. console.log($X)) or kind (node type, e.g. function_item). Returns matched lines and metavariable bindings. Language is inferred from extensions unless lang is set; respects .gitignore. Use grep for text or regex. Requires ast-grep on PATH.]],

  schema = {
    type = "object",
    properties = {
      pattern = {
        type = "string",
        description = "AST pattern: code with metavariables, e.g. `fn $F() { $BODY }`. One of `pattern`/`kind`.",
      },
      kind = {
        type = "string",
        description = "Syntax node kind, e.g. function_item. Supply this or pattern.",
      },
      lang = {
        type = "string",
        description = "Language name (e.g. rust, ts, tsx, python). Inferred from extensions when omitted.",
      },
      path = { type = "string", description = "Directory or file to search in (default: cwd)" },
      include = {
        type = "string",
        description = "File glob filter (e.g. *.rs)",
        alias = "globs",
      },
      strictness = {
        type = "string",
        description = "Pattern strictness: " .. table.concat(core.STRICTNESS, " | "),
      },
      limit = { type = "integer", description = "Max matches to return" },
      timeout = { type = "integer", description = "Timeout in seconds (default 60)" },
    },
  },

  header = function(input)
    return build_header(input, false)
  end,

  restore = function(_input, output, is_error, ctx)
    if is_error then
      return nil
    end
    local entries = core.parse_output(output)
    if #entries == 0 then
      return nil
    end
    return build_view(entries, ctx, SEARCH_TOL, SEARCH_TOL_FALLBACK)
  end,

  handler = function(input, ctx)
    local invalid = core.validate(input, false)
    if invalid then
      return error_output(invalid)
    end
    if not ast_grep_ready() then
      return error_output(core.NOT_FOUND)
    end

    local result, err = run_ast_grep(core.json_argv(input), resolve_timeout_ms(input))
    if not result then
      return error_output(err)
    end

    local entries, total, invalid_run = matches_of(result, input)
    if not entries then
      return error_output(invalid_run)
    end
    if #entries == 0 then
      return no_match_output(result)
    end

    for _, entry in ipairs(entries) do
      ctx:record_read(entry.path)
    end

    local shown = math.min(total, resolve_limit(input))
    local output = core.format(entries)
    if total > shown then
      output = output .. "\n" .. core.limit_note(shown, total)
    end
    output = truncate(with_stderr(output, result.stderr), output_limits.resolve(opts, ctx))

    return {
      llm_output = output,
      body = build_view(entries, ctx, SEARCH_TOL, SEARCH_TOL_FALLBACK),
      annotation = core.plural_files(core.MATCH_FMT, shown, #entries),
    }
  end,
})

maki.api.register_tool({
  name = "ast_grep_replace",
  kind = "edit",
  permission = "fs_write",
  mutable_path = "path",
  permission_scopes = "path",
  audiences = { "main", "general_sub", "interpreter" },
  description = [[Rewrite all AST matches in the absolute file or directory path. Search first with ast_grep using the same scope and pattern/kind. Supply rewrite; it may reuse pattern metavariables such as $X.
Every match is changed: limit caps displayed matches, not edits. Returns replacements and the applied count. Requires ast-grep on PATH.]],

  schema = {
    type = "object",
    properties = {
      pattern = {
        type = "string",
        description = "AST pattern: code with metavariables, e.g. `fn $F() { $BODY }`. One of `pattern`/`kind`.",
      },
      kind = {
        type = "string",
        description = "Syntax node kind, e.g. function_item. Supply this or pattern.",
      },
      rewrite = {
        type = "string",
        description = "Replacement snippet, may use the pattern's metavariables",
        required = true,
      },
      path = {
        type = "string",
        description = "Absolute path to the file or directory to rewrite",
        required = true,
      },
      lang = {
        type = "string",
        description = "Language name (e.g. rust, ts, tsx, python). Inferred from extensions when omitted.",
      },
      include = {
        type = "string",
        description = "File glob filter (e.g. *.rs)",
        alias = "globs",
      },
      strictness = {
        type = "string",
        description = "Pattern strictness: " .. table.concat(core.STRICTNESS, " | "),
      },
      limit = { type = "integer", description = "Max matches to show" },
      timeout = { type = "integer", description = "Timeout in seconds (default 60)" },
    },
  },

  header = function(input)
    return build_header(input, true)
  end,

  restore = function(_input, output, is_error, ctx)
    if is_error then
      return nil
    end
    local entries = core.parse_output(output)
    if #entries == 0 then
      return nil
    end
    return build_view(entries, ctx, REPLACE_TOL, REPLACE_TOL_FALLBACK)
  end,

  handler = function(input, ctx)
    local invalid = core.validate(input, true)
    if invalid then
      return error_output(invalid)
    end
    if not ast_grep_ready() then
      return error_output(core.NOT_FOUND)
    end

    local timeout_ms = resolve_timeout_ms(input)
    local preview, err = run_ast_grep(core.json_argv(input), timeout_ms)
    if not preview then
      return error_output(err)
    end

    local entries, total, invalid_run = matches_of(preview, input)
    if not entries then
      return error_output(invalid_run)
    end
    if #entries == 0 then
      return no_match_output(preview)
    end

    local applied_run, apply_err = run_ast_grep(core.apply_argv(input), timeout_ms)
    if not applied_run then
      local preview_output = core.format(entries) .. "\n" .. apply_err
      return error_output(truncate(preview_output, output_limits.resolve(opts, ctx)))
    end

    local applied = core.applied_count(applied_run.stdout, total)
    local changes = core.plural_files(core.CHANGED_FMT, applied, #entries)
    local output = core.format(entries) .. "\n" .. string.format(core.APPLIED_FMT, changes)
    output = truncate(with_stderr(output, preview.stderr), output_limits.resolve(opts, ctx))

    return {
      llm_output = output,
      body = build_view(entries, ctx, REPLACE_TOL, REPLACE_TOL_FALLBACK),
      annotation = core.plural_files(core.CHANGED_FMT, applied, #entries),
    }
  end,
})
