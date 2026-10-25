local backoff = require("maki.utf8").backoff

local M = {}

-- The project-id hash and the known-paths enumeration live in the shared lib,
-- so the /cd picker and these helpers can never disagree about the key a
-- project's state is filed under.
local known_paths = require("maki.known_paths")
M.project_id = known_paths.project_id

M.MAX_TAGS = 50
M.MAX_FILE_BYTES = 20 * 1024
M.MAX_DESCRIPTION_LEN = 240
M.CAP_HINT_REWRITE = "read the tool by path or run it from bash"
M.CAP_HINT_NARROW = "narrow tags or inspect tools by path"
M.CAP_HINT_FILTER = "pass tags or query to filter the list"
M.NO_TOOLS_MSG = "No custom tools yet."
M.META_SUFFIX = ".yaml"

local MAX_TAG_LEN = 64
local MAX_REJECT_DISPLAY = 64
local MAX_REL_DEPTH = 32
local SIZE_UNIT = 1024
local DETAIL_SEP = "·"
local TAG_STYLE = "keybind_section"
local DESCRIPTION_STYLE = "dim"
local SIZE_STYLE = "dim"
local WRITE_REJECT_PREFIX = "invalid tag(s) rejected: "
local READ_REJECT_PREFIX = "warning: ignored invalid tag(s): "
local UNREADABLE_PREFIX = "warning: unreadable custom tool files: "
local NO_MATCH_MSG = "no custom tools matched the filters; use `list` without filters to see all tools"
M.NO_MATCH_MSG = NO_MATCH_MSG
local PRUNE_ADVISORY = "Consider removing or consolidating stale custom tools to stay under " .. M.MAX_TAGS .. " tags."
local MISSING_WARN = "registered but missing from the project; delete the registration or restore the file"
local LEGACY_META_DIR = ".meta/"
local LEGACY_MISSING_WARN = "legacy .meta registration has no project file; re-register the tool from the project tree"
local DESCRIPTION_REQUIRED_ERR = "'description' is required for write"
local DESCRIPTION_EMPTY_ERR = "'description' must not be empty"
local DESCRIPTION_TOO_LONG_ERR = "description exceeds " .. M.MAX_DESCRIPTION_LEN .. " characters"
local CHMOD_TIMEOUT_MS = 5000
local COMMENT_PREFIXES = { "#", "//", "--" }
M.HELP_TIMEOUT_MS = 5000
M.HELP_MAX_BYTES = 4 * 1024
M.MAX_DERIVED_TAGS = 4

local COMMANDS = { "list", "read", "info", "write", "delete" }
local VALID_COMMANDS = {}
for _, c in ipairs(COMMANDS) do
  VALID_COMMANDS[c] = true
end

function M.safe_resolve(base_dir, relative)
  if not relative or relative == "" then
    return nil, "path is required"
  end
  if type(relative) ~= "string" then
    return nil, "path must be a string"
  end
  local first = relative:sub(1, 1)
  if relative:find("\0") or first == "/" or first == "\\" or relative:match("^%a:") then
    return nil, "path must be relative"
  end
  local resolved = maki.fs.normalize(maki.fs.joinpath(base_dir, relative))
  local norm_base = maki.fs.normalize(base_dir)
  local sep = norm_base:find("\\") and "\\" or "/"
  local prefix = norm_base .. sep
  if resolved:sub(1, #prefix) ~= prefix then
    return nil, "path traversal outside the project root is not allowed"
  end
  return resolved
end

-- The registry lives in the state dir; one YAML file per registered tool,
-- mirroring its project-relative path.
function M.meta_path(meta_dir, relative)
  return maki.fs.joinpath(meta_dir, relative .. M.META_SUFFIX)
end

function M.legacy_meta_path(meta_dir, relative)
  return maki.fs.joinpath(meta_dir, LEGACY_META_DIR .. relative .. M.META_SUFFIX)
end

-- Returns one record per registered tool, preferring the current mirrored
-- metadata over a legacy .meta sidecar for the same tool.
local function registry_entries(meta_dir)
  local by_name, names = {}, {}
  for _, entry in ipairs(maki.fs.dir(meta_dir, { depth = MAX_REL_DEPTH }) or {}) do
    local path, kind = entry[1], entry[2]
    if kind == "file" and path:sub(-#M.META_SUFFIX) == M.META_SUFFIX then
      local legacy_name = path:match("^%.meta[/\\](.+)$")
      local name = legacy_name or path
      name = name:sub(1, #name - #M.META_SUFFIX)
      if name ~= "" then
        local existing = by_name[name]
        if not existing then
          names[#names + 1] = name
        end
        if not existing or existing.legacy then
          by_name[name] = { name = name, legacy = legacy_name ~= nil }
        end
      end
    end
  end
  table.sort(names)
  local out = {}
  for _, name in ipairs(names) do
    out[#out + 1] = by_name[name]
  end
  return out
end

-- Coerces rather than rejects: any non-alphanumeric run becomes "_".
-- Returns nil only when nothing survives.
function M.normalize_tag(raw)
  if raw == nil then
    return nil
  end
  local s = tostring(raw):lower():gsub("[^%a%d]+", "_"):gsub("^_+", ""):gsub("_+$", "")
  if s == "" then
    return nil
  end
  return s:sub(1, MAX_TAG_LEN)
end

local function stem_tag(path)
  local base = maki.fs.basename(path) or path
  local stem = base:gsub("%.[^.]*$", "")
  if stem == "" then
    stem = base
  end
  return M.normalize_tag(stem) or "untagged"
end

function M.derive_tags(relative)
  local parts = {}
  local dir = maki.fs.dirname(relative)
  if dir and dir ~= "." and dir ~= "" then
    for part in dir:gmatch("[^/\\]+") do
      parts[#parts + 1] = part
    end
  end
  local base = maki.fs.basename(relative) or relative
  parts[#parts + 1] = base:gsub("%.[^.]*$", "")

  local tags, seen = {}, {}
  for i = math.max(1, #parts - M.MAX_DERIVED_TAGS + 1), #parts do
    local tag = M.normalize_tag(parts[i])
    if tag and not seen[tag] then
      seen[tag] = true
      tags[#tags + 1] = tag
    end
  end
  return tags
end

function M.normalize_tags(list)
  local seen, out, rejected, coerced = {}, {}, {}, {}
  for _, t in ipairs(list) do
    local n = M.normalize_tag(t)
    if n then
      if not seen[n] then
        seen[n] = true
        out[#out + 1] = n
      end
      if n ~= tostring(t) then
        coerced[#coerced + 1] = tostring(t) .. " -> " .. n
      end
    else
      rejected[#rejected + 1] = tostring(t)
    end
  end
  return out, rejected, coerced
end

function M.format_rejected(rejected)
  if not rejected or #rejected == 0 then
    return nil
  end
  local out = {}
  for i, v in ipairs(rejected) do
    out[i] = #v > MAX_REJECT_DISPLAY and v:sub(1, MAX_REJECT_DISPLAY) .. "..." or v
  end
  return table.concat(out, ", ")
end

function M.validate_input(input)
  local cmd = input.command
  if not VALID_COMMANDS[cmd] then
    return "unknown command '" .. tostring(cmd) .. "'. Valid commands: " .. table.concat(COMMANDS, ", ")
  end
  if input.tags ~= nil and type(input.tags) ~= "table" then
    return "'tags' must be an array"
  end
  if input.path ~= nil and type(input.path) ~= "string" then
    return "'path' must be a string"
  end
  if input.description ~= nil and type(input.description) ~= "string" then
    return "'description' must be a string"
  end
  if input.body ~= nil and type(input.body) ~= "string" then
    return "'body' must be a string"
  end
  if input.query ~= nil and type(input.query) ~= "string" then
    return "'query' must be a string"
  end
  if input.group_by ~= nil and input.group_by ~= "tool" and input.group_by ~= "tag" then
    return "'group_by' must be 'tool' or 'tag'"
  end
  local has_path = input.path ~= nil and input.path ~= ""
  local has_tags = type(input.tags) == "table" and #input.tags > 0
  local has_query = type(input.query) == "string" and input.query:match("%S") ~= nil
  if cmd == "read" then
    if has_path and has_tags then
      return "provide 'path' or 'tags', not both"
    end
    if not has_path and not has_tags then
      return "'path' or 'tags' is required for read"
    end
  elseif cmd == "info" then
    if has_path and (has_tags or has_query) then
      return "provide 'path' or tags/query, not both"
    end
    if not has_path and not has_tags and not has_query then
      return "'path', 'tags', or 'query' is required for info"
    end
  elseif cmd == "write" then
    if not has_path then
      return "'path' is required for write"
    end
  elseif cmd == "delete" then
    if not has_path then
      return "'path' is required for delete"
    end
  end
  return nil
end

local function trim(s)
  return (s:gsub("^%s+", ""):gsub("%s+$", ""))
end

function M.derive_description(content)
  if type(content) ~= "string" then
    return nil
  end
  for line in content:gmatch("[^\r\n]+") do
    local s = trim(line)
    if s ~= "" and s:sub(1, 2) ~= "#!" then
      local text
      for _, prefix in ipairs(COMMENT_PREFIXES) do
        if s:sub(1, #prefix) == prefix then
          text = trim(s:sub(#prefix + 1))
          break
        end
      end
      if not text then
        return nil
      end
      if text ~= "" and not text:match("^[-=*#/]+$") and not text:match("^shellcheck ") and not text:match("^vim:") then
        return M.normalize_description(text)
      end
    end
  end
  return nil
end

function M.capture_help(path)
  local ok, id = pcall(maki.fn.jobstart, { path, "--help" })
  if not ok or type(id) ~= "number" then
    return nil, "could not start: " .. tostring(id)
  end
  local result = maki.fn.jobwait(id, M.HELP_TIMEOUT_MS)
  if not result then
    maki.fn.jobstop(id)
    return nil, "timed out after " .. M.HELP_TIMEOUT_MS .. " ms"
  end
  local out = trim(result.stdout or "")
  if out == "" then
    out = trim(result.stderr or "")
  end
  if out == "" then
    return nil, "no output (exit " .. tostring(result.exit_code) .. ")"
  end
  if result.exit_code ~= 0 then
    return out, "exited " .. tostring(result.exit_code)
  end
  return out, nil
end

function M.normalize_description(raw)
  if type(raw) ~= "string" then
    return nil
  end
  local s = raw:gsub("%s+", " "):gsub("^ ", ""):gsub(" $", "")
  if s == "" then
    return nil
  end
  return s:sub(1, M.MAX_DESCRIPTION_LEN)
end

function M.validate_description(raw)
  if type(raw) ~= "string" then
    return nil, DESCRIPTION_REQUIRED_ERR
  end
  local s = raw:gsub("%s+", " "):gsub("^ ", ""):gsub(" $", "")
  if s == "" then
    return nil, DESCRIPTION_EMPTY_ERR
  end
  if #s > M.MAX_DESCRIPTION_LEN then
    return nil, DESCRIPTION_TOO_LONG_ERR
  end
  return s
end

-- Returns (want set, warning?, error?): error when no tag survives
-- normalization, warning when only some were rejected.
local function normalize_to_want(raw_tags)
  local normalized, rejected = M.normalize_tags(raw_tags)
  local r = M.format_rejected(rejected)
  if #normalized == 0 then
    return nil, nil, "no valid tags after normalization" .. (r and ("; rejected: " .. r) or "")
  end
  local want = {}
  for _, n in ipairs(normalized) do
    want[n] = true
  end
  return want, r and (READ_REJECT_PREFIX .. r) or nil
end

function M.validate_write_tags(raw_tags)
  local normalized, rejected, coerced = M.normalize_tags(raw_tags)
  local r = M.format_rejected(rejected)
  if r then
    return nil, WRITE_REJECT_PREFIX .. r
  end
  local note = #coerced > 0 and ("normalized: " .. table.concat(coerced, ", ")) or nil
  return normalized, nil, note
end

-- Fields omitted on a write keep the existing tool's values, so retagging or
-- re-describing a tool does not resend its source. An explicit empty body
-- clears a command definition.
function M.merge_meta(existing, tags, description, body)
  local normalized, tag_err, note
  if tags ~= nil then
    normalized, tag_err, note = M.validate_write_tags(tags)
    if tag_err then
      return nil, tag_err
    end
  elseif existing then
    normalized = existing.tags
  else
    normalized = {}
  end
  local desc
  if description ~= nil then
    local desc_err
    desc, desc_err = M.validate_description(description)
    if desc_err then
      return nil, desc_err
    end
  elseif existing then
    desc = existing.description
  end
  if not desc then
    return nil, DESCRIPTION_REQUIRED_ERR
  end
  local final_body
  if body ~= nil then
    if type(body) ~= "string" then
      return nil, "'body' must be a string"
    end
    final_body = body ~= "" and body or nil
  elseif existing then
    final_body = existing.body
  end
  return { tags = normalized, description = desc, body = final_body, note = note }
end

local function join_parts(sep, ...)
  local parts = {}
  for i = 1, select("#", ...) do
    local v = select(i, ...)
    if v then
      parts[#parts + 1] = v
    end
  end
  return table.concat(parts, sep)
end

local function combine_unreadable(warnings)
  if #warnings == 0 then
    return nil
  end
  return UNREADABLE_PREFIX .. table.concat(warnings, ", ")
end

function M.parse_meta(content)
  local decoded, err = maki.yaml.decode(content)
  if err then
    return nil, "metadata decode error: " .. tostring(err)
  end
  if type(decoded) ~= "table" then
    return nil, "metadata is not a mapping"
  end
  return {
    tags = M.extract_tags(decoded) or {},
    description = M.normalize_description(decoded.description),
    body = type(decoded.body) == "string" and decoded.body ~= "" and decoded.body or nil,
  }
end

function M.encode_meta(tags, description, body)
  local payload = { tags = tags or {} }
  if description then
    payload.description = description
  end
  if body then
    payload.body = body
  end
  local yaml, err = maki.yaml.encode(payload)
  if not yaml then
    return nil, "metadata encode error: " .. tostring(err)
  end
  return yaml
end

function M.read_meta(meta_dir, name)
  local path = M.meta_path(meta_dir, name)
  local content, read_err = maki.fs.read(path)
  if not content then
    if maki.fs.metadata(path) then
      return nil, "metadata read error: " .. tostring(read_err)
    end
    path = M.legacy_meta_path(meta_dir, name)
    content, read_err = maki.fs.read(path)
    if not content then
      if not maki.fs.metadata(path) then
        return nil, nil
      end
      return nil, "metadata read error: " .. tostring(read_err)
    end
  end
  return M.parse_meta(content)
end

function M.write_meta(meta_dir, name, tags, description, body)
  local yaml, err = M.encode_meta(tags, description, body)
  if not yaml then
    return nil, err
  end
  local path = M.meta_path(meta_dir, name)
  local _, mkdir_err = maki.fs.mkdir(maki.fs.dirname(path), { parents = true })
  if mkdir_err then
    return nil, "metadata mkdir error: " .. tostring(mkdir_err)
  end
  local ok, write_err = maki.fs.write(path, yaml)
  if not ok then
    return nil, "metadata write error: " .. tostring(write_err)
  end
  return true
end

function M.metadata_for(meta_dir, name)
  local meta, warn = M.read_meta(meta_dir, name)
  if meta then
    local tags = #meta.tags > 0 and meta.tags or { stem_tag(name) }
    return tags, meta.description, meta.body, warn
  end
  return { stem_tag(name) }, nil, nil, warn
end

-- Moves a legacy .meta sidecar to the mirrored registry path once the tool
-- itself has been moved into the project tree.
function M.migrate_legacy(meta_dir, name)
  local path = M.legacy_meta_path(meta_dir, name)
  local content, read_err = maki.fs.read(path)
  if not content then
    return nil, "legacy metadata read error: " .. tostring(read_err)
  end
  local meta, parse_err = M.parse_meta(content)
  if not meta then
    return nil, parse_err
  end
  local ok, write_err = M.write_meta(meta_dir, name, meta.tags, meta.description, meta.body)
  if not ok then
    return nil, "legacy metadata migration failed: " .. tostring(write_err)
  end
  maki.fs.rm(path)
  return meta
end

function M.extract_tags(frontmatter)
  if type(frontmatter) ~= "table" then
    return nil
  end
  local raw = frontmatter.tags
  if type(raw) == "string" then
    raw = { raw }
  end
  if type(raw) ~= "table" then
    return nil
  end
  local out = M.normalize_tags(raw)
  return #out > 0 and out or nil
end

-- Tags and descriptions per registered tool, keyed on the registry file's
-- mtime/size so unchanged metadata is never re-read. Invalid metadata is
-- never cached.
local meta_cache = {}

local function cached_metadata(meta_dir, name)
  local path = M.meta_path(meta_dir, name)
  if not maki.fs.metadata(path) and maki.fs.metadata(M.legacy_meta_path(meta_dir, name)) then
    path = M.legacy_meta_path(meta_dir, name)
  end
  local meta = maki.fs.metadata(path)
  local meta_size = meta and meta.size or 0
  local meta_mtime = meta and meta.mtime or 0
  local c = meta_cache[path]
  if c and c.meta_size == meta_size and c.meta_mtime == meta_mtime then
    return c.tags, c.description, c.body, nil
  end
  local tags, description, body, warn = M.metadata_for(meta_dir, name)
  if not warn then
    meta_cache[path] = {
      meta_size = meta_size,
      meta_mtime = meta_mtime,
      tags = tags,
      description = description,
      body = body,
    }
  end
  return tags, description, body, warn
end

local function has_wanted_tag(tags, want)
  for _, t in ipairs(tags) do
    if want[t] then
      return true
    end
  end
  return false
end

local function matching_entries(meta_dir, root, want)
  local matches, warnings = {}, {}
  local files, fwarnings = M.files_with_tags(meta_dir, root)
  for _, w in ipairs(fwarnings) do
    warnings[#warnings + 1] = w
  end
  for _, f in ipairs(files) do
    if has_wanted_tag(f.tags, want) then
      if f.body then
        matches[#matches + 1] = {
          name = f.name,
          content = f.body,
          size = "command",
          tags = f.tags,
          description = f.description,
        }
      else
        local content, read_err = maki.fs.read(maki.fs.joinpath(root, f.name))
        if content then
          matches[#matches + 1] = {
            name = f.name,
            content = content,
            size = f.size,
            tags = f.tags,
            description = f.description,
          }
        else
          warnings[#warnings + 1] = f.name .. ": " .. tostring(read_err)
        end
      end
    end
  end
  return matches, warnings
end

-- Name-sorted, tags in registry order; unreadable metadata lands in warnings
-- without hiding the tool (the stem tag still identifies it), and a tool whose
-- file vanished from the project warns and drops out of the index.
function M.files_with_tags(meta_dir, root)
  local files, warnings = {}, {}
  for _, reg in ipairs(registry_entries(meta_dir)) do
    local name = reg.name
    local project_meta = maki.fs.metadata(maki.fs.joinpath(root, name))
    if reg.legacy and not project_meta then
      warnings[#warnings + 1] = name .. ": " .. LEGACY_MISSING_WARN
    else
      if reg.legacy then
        local _, migrate_err = M.migrate_legacy(meta_dir, name)
        if migrate_err then
          warnings[#warnings + 1] = name .. ": " .. migrate_err
        end
      end
      local tags, description, body, warn = cached_metadata(meta_dir, name)
      if warn then
        warnings[#warnings + 1] = name .. ": " .. warn
      end
      if body then
        files[#files + 1] = {
          name = name,
          size = #body,
          tags = tags,
          description = description,
          body = body,
        }
      elseif not project_meta then
        warnings[#warnings + 1] = name .. ": " .. MISSING_WARN
      else
        files[#files + 1] = { name = name, size = project_meta.size, tags = tags, description = description }
      end
    end
  end
  return files, warnings
end

-- Groups files under their tags, most-used tag first (name breaks ties).
function M.group_by_tag(files)
  local by_tag, groups = {}, {}
  for _, f in ipairs(files) do
    for _, t in ipairs(f.tags) do
      local g = by_tag[t]
      if not g then
        g = { tag = t, files = {} }
        by_tag[t] = g
        groups[#groups + 1] = g
      end
      g.files[#g.files + 1] = { name = f.name, size = f.size, description = f.description, body = f.body }
    end
  end
  table.sort(groups, function(a, b)
    if #a.files == #b.files then
      return a.tag < b.tag
    end
    return #a.files > #b.files
  end)
  return groups
end

function M.grouped_tags(meta_dir, root)
  local files, warnings = M.files_with_tags(meta_dir, root)
  return M.group_by_tag(files), warnings
end

-- For the picker only. format_list keeps the exact "(N bytes)" the model reads.
function M.format_size(bytes)
  if type(bytes) ~= "number" then
    return bytes
  end
  return bytes < SIZE_UNIT and bytes .. "B" or string.format("%.1fK", bytes / SIZE_UNIT)
end

-- A detail is right-aligned, so the size goes last and lands against the border
-- on every row. The description is elastic, so a narrow row eats it before the
-- tags or the size.
function M.detail_parts(size, description, tags)
  local size_text = M.format_size(size)
  local parts = {}
  if description then
    parts[#parts + 1] = { description, DESCRIPTION_STYLE, elastic = true }
    if tags and #tags > 0 then
      parts[#parts + 1] = { " " .. DETAIL_SEP .. " " .. table.concat(tags, ", "), TAG_STYLE }
    end
    parts[#parts + 1] = { " " .. DETAIL_SEP .. " " .. size_text, SIZE_STYLE }
    return parts
  end
  if tags and #tags > 0 then
    parts[#parts + 1] = { table.concat(tags, ", "), TAG_STYLE, elastic = true }
    parts[#parts + 1] = { " " .. DETAIL_SEP .. " " .. size_text, SIZE_STYLE }
    return parts
  end
  return { { size_text, SIZE_STYLE } }
end

function M.format_tag_line(meta_dir, root, max_tags)
  local groups, warnings = M.grouped_tags(meta_dir, root)
  if #groups == 0 and #warnings == 0 then
    return nil
  end
  local tags = {}
  for i, g in ipairs(groups) do
    tags[i] = g.tag
  end
  local line = table.concat(tags, ", ", 1, math.min(#tags, max_tags))
  if #tags > max_tags then
    line = line .. " ... (" .. (#tags - max_tags) .. " tags omitted; use `list` to see all)"
  end
  if #warnings > 0 then
    line = (#line > 0 and line .. " " or "") .. "(unreadable: " .. #warnings .. ")"
  end
  return line
end

function M.cap_read_output(s, hint)
  if #s <= M.MAX_FILE_BYTES then
    return s
  end
  local cut = backoff(s, M.MAX_FILE_BYTES)
  return s:sub(1, cut) .. "\n... (output truncated at " .. M.MAX_FILE_BYTES .. " bytes; " .. hint .. ")"
end

-- Makes a registered tool runnable directly from bash. Best effort: a host
-- without chmod (Windows) keeps the tool usable through its interpreter.
function M.make_executable(path)
  local ok, id = pcall(maki.fn.jobstart, { "chmod", "u+x", path }, { stdout = false, stderr = false })
  if not ok or type(id) ~= "number" then
    return nil, tostring(id)
  end
  local res = maki.fn.jobwait(id, CHMOD_TIMEOUT_MS)
  if not res then
    maki.fn.jobstop(id)
    return nil, "chmod timed out"
  end
  if res.exit_code ~= 0 then
    return nil, "chmod exited " .. tostring(res.exit_code)
  end
  return true
end

-- Unregisters a tool: drops its registry entry, never the project file.
function M.remove_tool(meta_dir, name)
  local path, err = M.safe_resolve(meta_dir, name .. M.META_SUFFIX)
  if not path then
    return nil, err
  end
  if not maki.fs.metadata(path) then
    path, err = M.safe_resolve(meta_dir, LEGACY_META_DIR .. name .. M.META_SUFFIX)
    if not path then
      return nil, err
    end
  end
  if not maki.fs.metadata(path) then
    return nil, "'" .. name .. "' is not registered"
  end
  local ok, rm_err = maki.fs.rm(path)
  if not ok then
    return nil, "unregister error: " .. tostring(rm_err)
  end
  return true
end

function M.format_info_entry(name, size, tags, description, help)
  local label = type(size) == "number" and (size .. " bytes") or size
  local header = name .. " (" .. label .. ")"
  if tags and #tags > 0 then
    header = header .. " [" .. table.concat(tags, ", ") .. "]"
  end
  if description then
    header = header .. "\n" .. description
  end
  return header .. "\n\n" .. help
end

function M.cap_help(s)
  if #s <= M.HELP_MAX_BYTES then
    return s
  end
  local cut = backoff(s, M.HELP_MAX_BYTES)
  return s:sub(1, cut) .. "\n... (help truncated at " .. M.HELP_MAX_BYTES .. " bytes)"
end

function M.format_read_entry(name, size, content, tags, description)
  local label = type(size) == "number" and (size .. " bytes") or size
  local header = name .. " (" .. label .. ")"
  if tags and #tags > 0 then
    header = header .. " [" .. table.concat(tags, ", ") .. "]"
  end
  if description then
    header = header .. "\n" .. description
  end
  return header .. "\n\n" .. content
end

local function format_list_row(f, bullet, show_tags)
  local label = f.body and "command" or (f.size .. " bytes")
  local line = bullet .. f.name .. " (" .. label .. ")"
  if show_tags and #f.tags > 0 then
    line = line .. " [" .. table.concat(f.tags, ", ") .. "]"
  end
  if f.description then
    line = line .. ": " .. f.description
  end
  return line
end

local function normalize_query(raw)
  if type(raw) ~= "string" then
    return nil
  end
  local q = raw:match("^%s*(.-)%s*$"):lower()
  return q ~= "" and q or nil
end

local function matches_query(f, q)
  local haystack = (f.name .. "\n" .. (f.description or "") .. "\n" .. table.concat(f.tags, " ")):lower()
  return haystack:find(q, 1, true) ~= nil
end

function M.filter_files(files, want, raw_query)
  local q = normalize_query(raw_query)
  local out = {}
  for _, f in ipairs(files) do
    if (not want or has_wanted_tag(f.tags, want)) and (not q or matches_query(f, q)) then
      out[#out + 1] = f
    end
  end
  return out
end

function M.format_list(meta_dir, root, raw_tags, query, group_by)
  local want, warning
  if raw_tags and #raw_tags > 0 then
    local werr
    want, warning, werr = normalize_to_want(raw_tags)
    if werr then
      return nil, werr
    end
  end
  local q = normalize_query(query)

  local files, read_warnings = M.files_with_tags(meta_dir, root)
  if #files == 0 and not want and not q then
    return join_parts("\n", combine_unreadable(read_warnings), M.NO_TOOLS_MSG)
  end

  local filtered = M.filter_files(files, want, query)

  local unreadable = combine_unreadable(read_warnings)
  if #filtered == 0 then
    return join_parts("\n", warning, unreadable, NO_MATCH_MSG)
  end

  local lines = {}
  local groups
  if group_by == "tag" then
    groups = M.group_by_tag(filtered)
    for _, g in ipairs(groups) do
      lines[#lines + 1] = g.tag .. " (" .. #g.files .. ")"
      for _, f in ipairs(g.files) do
        lines[#lines + 1] = format_list_row(f, "  - ", false)
      end
      lines[#lines + 1] = ""
    end
  else
    for _, f in ipairs(filtered) do
      lines[#lines + 1] = format_list_row(f, "- ", true)
    end
  end

  local body = M.cap_read_output(table.concat(lines, "\n"), M.CAP_HINT_FILTER)
  if not want and not q then
    groups = groups or M.group_by_tag(filtered)
    if #groups > M.MAX_TAGS then
      body = body .. "\n" .. PRUNE_ADVISORY
    end
  end
  return join_parts("\n", warning, unreadable, body), nil
end

function M.format_read(meta_dir, root, raw_tags)
  local want, warning, err = normalize_to_want(raw_tags)
  if err then
    return nil, err
  end

  local matches, read_warnings = matching_entries(meta_dir, root, want)
  local parts = {}
  for _, m in ipairs(matches) do
    parts[#parts + 1] = M.format_read_entry(m.name, m.size, m.content, m.tags, m.description)
  end

  local hint = #parts <= 1 and M.CAP_HINT_REWRITE or M.CAP_HINT_NARROW
  local body = #parts > 0 and M.cap_read_output(table.concat(parts, "\n\n"), hint) or NO_MATCH_MSG
  return join_parts("\n\n", warning, combine_unreadable(read_warnings), body)
end

M.normalize_want = normalize_to_want
M.has_tag = has_wanted_tag
M.combine_unreadable = combine_unreadable

return M
