-- ast-grep CLI plumbing shared by the search and rewrite tools: argv
-- building, `--json` matches into entries, and the llm_output format the
-- restore hook parses back into entries.
--
-- An entry is `{ path, rows = { row } }`, grouped per file, in run order.
-- A row is `{ kind, nr?, text }` with kind "line" (a matched source line),
-- "meta" (metavariable bindings for the match above it), or "new" (the
-- replacement a rewrite produced).

local shorten_path = require("maki.shorten_path")

local M = {}

M.BIN = "ast-grep"
M.NOT_FOUND = "ast-grep is not on PATH. Install it (https://ast-grep.github.io) or disable the ast_grep plugin."
M.NO_MATCHES = "No matches"
M.SEARCH_REQUIRED =
  "error: pattern or kind is required (pattern: an AST snippet like `fn $F() { $BODY }`; kind: a node kind like `function_item`)"
M.REWRITE_REQUIRED = "error: rewrite is required (the replacement snippet, free to reuse the pattern's metavariables)"
M.PATH_REQUIRED = "error: path is required (absolute path to the file or directory to rewrite)"
M.MATCH_FMT = "%d matches in %d %s"
M.CHANGED_FMT = "%d changes in %d %s"
M.APPLIED_FMT = "Applied %s."
M.LIMIT_FMT = "... (%d of %d matches shown; raise `limit` for more)"
M.STRICTNESS = { "cst", "smart", "ast", "relaxed", "signature", "template" }

local JSON_FLAG = "--json=compact"
local GLOB_FLAG = "--globs"
local REWRITE_FLAG = "-r"
local UPDATE_FLAG = "--update-all"
local APPLIED_RE = "Applied (%d+) change"
local META_VALUE_BYTES = 48
local META_TOTAL_BYTES = 240
local ELLIPSIS = "…"

--- {text} cut at a UTF-8 boundary, so a byte cap never splits a character.
local function cut(text, max_bytes)
  if not max_bytes or #text <= max_bytes then
    return text
  end
  local cut_at = max_bytes
  while cut_at > 0 and text:find("^[\128-\191]", cut_at + 1) do
    cut_at = cut_at - 1
  end
  return text:sub(1, cut_at) .. ELLIPSIS
end

--- `$NAME=value` bindings for one match, sorted so runs are reproducible and
--- dropped once the suffix would drown the code it annotates.
local function meta_text(meta_variables)
  if not meta_variables then
    return nil
  end
  local single = meta_variables.single or {}
  local multi = meta_variables.multi or {}
  local names = {}
  for name in pairs(single) do
    names[#names + 1] = name
  end
  for name in pairs(multi) do
    names[#names + 1] = name
  end
  table.sort(names)

  local parts, total = {}, 0
  for _, name in ipairs(names) do
    local binding = single[name]
    local value = binding and binding.text
    if not value then
      local texts = {}
      for _, item in ipairs(multi[name] or {}) do
        texts[#texts + 1] = item.text
      end
      value = table.concat(texts, ",")
    end
    local part = "$" .. name .. "=" .. cut(value, META_VALUE_BYTES)
    if total + #part > META_TOTAL_BYTES then
      break
    end
    parts[#parts + 1] = part
    total = total + #part + 1
  end
  if #parts == 0 then
    return nil
  end
  return table.concat(parts, " ")
end

local function match_rows(match, max_bytes)
  local rows = {}
  local start_line = ((match.range and match.range.start and match.range.start.line) or 0) + 1
  for i, text in ipairs(maki.split(match.lines or match.text or "", "\n")) do
    rows[#rows + 1] = { kind = "line", nr = start_line + i - 1, text = cut(text, max_bytes) }
  end
  local meta = meta_text(match.metaVariables)
  if meta then
    rows[#rows + 1] = { kind = "meta", text = meta }
  end
  if match.replacement then
    for _, text in ipairs(maki.split(match.replacement, "\n")) do
      rows[#rows + 1] = { kind = "new", text = cut(text, max_bytes) }
    end
  end
  return rows
end

local function base_argv(input, flags)
  local args = { M.BIN, "run" }
  local function add(flag, value)
    args[#args + 1] = flag
    args[#args + 1] = value
  end
  if input.pattern then
    add("-p", input.pattern)
  end
  if input.kind then
    add("-k", input.kind)
  end
  if input.strictness then
    add("--strictness", input.strictness)
  end
  if input.lang then
    add("-l", input.lang)
  end
  if input.include then
    add(GLOB_FLAG, input.include)
  end
  for _, flag in ipairs(flags) do
    args[#args + 1] = flag
  end
  if input.path then
    args[#args + 1] = input.path
  end
  return args
end

--- A search (and the dry run a rewrite previews with): matches only, nothing
--- written.
function M.json_argv(input)
  return base_argv(input, { JSON_FLAG })
end

--- The same run with the rewrite applied to every match under `path`.
function M.apply_argv(input)
  return base_argv(input, { REWRITE_FLAG, input.rewrite, UPDATE_FLAG })
end

function M.validate(input, needs_path)
  if not input.pattern and not input.kind then
    return M.SEARCH_REQUIRED
  end
  if needs_path then
    if not input.rewrite or input.rewrite == "" then
      return M.REWRITE_REQUIRED
    end
    if not input.path or input.path == "" then
      return M.PATH_REQUIRED
    end
  end
  return nil
end

--- Matches from an ast-grep `--json=compact` run, grouped per file. Answers
--- (entries, total, nil) or (nil, nil, why the output is not a match array).
function M.parse_matches(stdout, limit, max_bytes)
  local decoded, err = maki.json.decode(stdout or "")
  if not decoded then
    return nil, nil, "could not parse ast-grep output: " .. tostring(err)
  end
  if type(decoded) ~= "table" then
    return nil, nil, "could not parse ast-grep output: expected an array of matches"
  end

  local total = #decoded
  local entries, by_path = {}, {}
  for i = 1, math.min(total, limit) do
    local match = decoded[i]
    local path = maki.fs.abspath(match.file or ".")
    local entry = by_path[path]
    if not entry then
      entry = { path = path, rows = {} }
      by_path[path] = entry
      entries[#entries + 1] = entry
    end
    for _, row in ipairs(match_rows(match, max_bytes)) do
      entry.rows[#entry.rows + 1] = row
    end
  end
  return entries, total, nil
end

--- Entries back out of {text}, so a restored view renders what the model saw.
--- Lines that are neither a header, a row, nor a separator are dropped: the
--- trailing notes an error or a limit appends.
function M.parse_output(text)
  local entries, current = {}, nil
  for _, line in ipairs(maki.split(text, "\n")) do
    local path = line:match("^(%S.+):$")
    if path then
      current = { path = path, rows = {} }
      entries[#entries + 1] = current
    elseif current then
      local nr, content = line:match("^%s+(%d+): (.*)$")
      if nr then
        current.rows[#current.rows + 1] = { kind = "line", nr = tonumber(nr), text = content }
      else
        local new_text = line:match("^%s+%-> (.*)$")
        if new_text then
          current.rows[#current.rows + 1] = { kind = "new", text = new_text }
        else
          local meta = line:match("^%s+(%$.*)$")
          if meta then
            current.rows[#current.rows + 1] = { kind = "meta", text = meta }
          end
        end
      end
    end
  end
  return entries
end

function M.format(entries)
  local parts = {}
  for i, entry in ipairs(entries) do
    if i > 1 then
      parts[#parts + 1] = ""
    end
    parts[#parts + 1] = shorten_path(entry.path) .. ":"
    for _, row in ipairs(entry.rows) do
      if row.kind == "line" then
        parts[#parts + 1] = string.format("  %d: %s", row.nr, row.text)
      elseif row.kind == "meta" then
        parts[#parts + 1] = "  " .. row.text
      else
        parts[#parts + 1] = "   -> " .. row.text
      end
    end
  end
  return table.concat(parts, "\n")
end

--- "{count} matches|changes in {files} files", singular in the file count the
--- way grep reports it.
function M.plural_files(fmt, count, files)
  return string.format(fmt, count, files, files == 1 and "file" or "files")
end

function M.limit_note(shown, total)
  return string.format(M.LIMIT_FMT, shown, total)
end

--- How many changes the apply run reported, falling back to what the preview
--- saw when ast-grep printed no count.
function M.applied_count(stdout, fallback)
  return tonumber(stdout and stdout:match(APPLIED_RE)) or fallback
end

return M
