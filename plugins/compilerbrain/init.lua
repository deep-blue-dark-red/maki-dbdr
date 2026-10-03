local shorten_path = require("maki.shorten_path")
local ToolView = require("maki.tool_view")
local FALLBACK_VIEW_LINES = 10
local DIFF_OLD = { style = "diff_old", prefix = "- ", sign = "diff_old_sign", nr = "diff_old_line_nr" }
local DIFF_NEW = { style = "diff_new", prefix = "+ ", sign = "diff_new_sign", nr = "diff_new_line_nr" }
local ESCAPES = { n = "\n", t = "\t", r = "\r", ["\\"] = "\\", ['"'] = '"' }
local TRUNCATED_NOTE = "diff truncated; read the file for the capped lines"
local function split_lines(text)
  local lines = maki.split(text or "", "\n")
  if lines[#lines] == "" then
    lines[#lines] = nil
  end
  return lines
end
local function split_cells(text)
  return maki.split(text, ",")
end
local function ext_of(path)
  return path and path:match("%.([^%.]+)$") or nil
end

local function view_opts(ctx)
  local tol = ctx:tool_output_lines()
  return { max_lines = (tol and tol.other) or FALLBACK_VIEW_LINES, keep = "head" }
end

local function column_index(cols, name)
  for i, col in ipairs(cols) do
    if col == name then
      return i
    end
  end
end
-- One tabular row: quoted cells keep commas and escapes, `null` and "" are
-- null cells, anything else is a bare token left as text.
local function parse_row(line)
  local cells, cell, quoted = {}, {}, false
  local at = 0
  local function flush()
    local raw = table.concat(cell)
    at = at + 1
    if not quoted and (raw == "" or raw == "null") then
      cells[at] = nil
    else
      cells[at] = raw
    end
    cell, quoted = {}, false
  end
  local i, n = 1, #line
  while i <= n do
    local ch = line:sub(i, i)
    if quoted then
      if ch == "\\" and i < n then
        cell[#cell + 1] = ESCAPES[line:sub(i + 1, i + 1)] or line:sub(i + 1, i + 1)
        i = i + 2
      elseif ch == '"' then
        quoted = false
        i = i + 1
      else
        cell[#cell + 1] = ch
        i = i + 1
      end
    elseif ch == '"' then
      quoted = true
      i = i + 1
    elseif ch == "," then
      flush()
      i = i + 1
    else
      cell[#cell + 1] = ch
      i = i + 1
    end
  end
  flush()
  return cells
end
local function scan_blocks(lines)
  local blocks = {}
  local i = 1
  while i <= #lines do
    local count, cols = lines[i]:match("^%s*%[(%d+)%]%{([^}]*)%}:$")
    if count then
      count = tonumber(count)
      local rows = {}
      for k = 1, count do
        local row_line = lines[i + k]
        if not row_line then
          break
        end
        rows[#rows + 1] = parse_row(row_line:match("^%s*(.-)%s*$"))
      end
      local block = { cols = split_cells(cols), rows = rows }
      block.rm = column_index(block.cols, "Removed")
      block.ad = column_index(block.cols, "Added")
      if block.rm and block.ad then
        block.sl = column_index(block.cols, "StartLine")
        block.fp = column_index(block.cols, "FilePath")
      end
      blocks[#blocks + 1] = block
      i = i + count + 1
    else
      i = i + 1
    end
  end
  return blocks
end
local function scalar(lines, key)
  for _, line in ipairs(lines) do
    local v = line:match("^%s*" .. key .. ": (.+)%s*$")
    if v then
      return v
    end
  end
end
local function gutter_width(rows, rm, sl)
  local w = 0
  for _, row in ipairs(rows) do
    if row[rm] and row[sl] and row[sl] ~= "" then
      w = math.max(w, #row[sl])
    end
  end
  return w
end
local function nr_span(fmt, side, nr)
  return { string.format(fmt, nr or ""), side.nr }
end
-- One row can carry multi-line text only when a whole-line break left the
-- file; the number goes on the part that owns it, the rest follow bare. An
-- empty cell is still the line that happened -- a blank one.
local function append_side(view, fmt, side, text, nr, jobs, ext)
  local parts = split_lines(text)
  if #parts == 0 then
    parts = { "" }
  end
  jobs[#jobs + 1] = {
    first = #view.all_lines + 1,
    fmt = fmt,
    text = table.concat(parts, "\n"),
    side = side,
    nr = nr,
    ext = ext,
  }
  for j, line in ipairs(parts) do
    local spans = {}
    if fmt then
      spans[#spans + 1] = nr_span(fmt, side, j == 1 and nr or nil)
    end
    spans[#spans + 1] = { side.prefix, side.sign }
    spans[#spans + 1] = { line, side.style }
    view:append(spans)
  end
end
local function render_diff_rows(view, rows, rm, ad, sl, fp, jobs, default_ext)
  if #rows == 0 then
    return
  end
  local w = gutter_width(rows, rm, sl)
  local fmt = w > 0 and ("%" .. w .. "s ") or nil
  local prev_nr = nil
  local prev_file = nil
  for _, row in ipairs(rows) do
    local removed, added = row[rm], row[ad]
    local nr = sl and row[sl] and row[sl] ~= "" and tonumber(row[sl]) or nil
    local file = fp and row[fp] or nil
    if file and file ~= prev_file then
      view:append({})
      view:append({ { shorten_path(file), "path" } })
      prev_nr = nil
    end
    prev_file = file
    local ext = ext_of(file) or default_ext
    if removed then
      if nr and prev_nr and nr > prev_nr + 1 then
        view:append({ { "...", "tool_dim" } })
      end
      append_side(view, fmt, DIFF_OLD, removed, nr and tostring(nr) or nil, jobs, ext)
      prev_nr = nr or prev_nr
    end
    if added then
      append_side(view, fmt, DIFF_NEW, added, nil, jobs, ext)
    end
  end
end
-- Second pass: syntax colors over the diff backgrounds, gutters rebuilt
-- byte-identically to the plain render. Sides without a diff background in
-- the theme stay plain.
local function apply_highlights(view, jobs)
  maki.async.run(function()
    for _, job in ipairs(jobs) do
      local side = job.ext and maki.ui.theme_style(job.side.style)
      local bg = side and side.bg
      local highlighted = bg and maki.ui.highlight(job.text, job.ext)
      for i, hl_line in ipairs(highlighted or {}) do
        local idx = job.first + i - 1
        if not view.all_lines[idx] then
          break
        end
        local spans = {}
        if job.fmt then
          spans[#spans + 1] = nr_span(job.fmt, job.side, i == 1 and job.nr or nil)
        end
        spans[#spans + 1] = { job.side.prefix, job.side.sign }
        for _, seg in ipairs(hl_line) do
          local s = type(seg[2]) == "table" and seg[2] or {}
          s.bg = bg
          spans[#spans + 1] = { seg[1], s }
        end
        view:update_line(idx, spans)
      end
    end
    view:flush()
  end)
end
local function diff_view(lines, diff_blocks)
  local buf = maki.ui.buf()
  local view = ToolView.new(buf, { max_lines = math.huge, keep = "head" })
  local path = scalar(lines, "FilePath")
  if path then
    view:append({ { shorten_path(path), "path" } })
  end
  local tier = scalar(lines, "Tier")
  if tier then
    view:append({ { "Tier: " .. tier, "tool_dim" } })
  end
  local jobs = {}
  for i, block in ipairs(diff_blocks) do
    if i > 1 or tier then
      view:append({})
    end
    render_diff_rows(view, block.rows, block.rm, block.ad, block.sl, block.fp, jobs, ext_of(path))
  end
  if scalar(lines, "DiffTruncated") == "true" then
    view:append({})
    view:append({ { TRUNCATED_NOTE, "tool_dim" } })
  end
  view:finish()
  if #jobs > 0 then
    apply_highlights(view, jobs)
  end
  return buf
end
local function render_edit_result(input, output, is_error, ctx)
  if is_error or type(output) ~= "string" then
    return ToolView.restore(output, view_opts(ctx))
  end
  local lines = split_lines(output)
  local diff_blocks = {}
  for _, block in ipairs(scan_blocks(lines)) do
    if block.rm and block.ad then
      diff_blocks[#diff_blocks + 1] = block
    end
  end
  if #diff_blocks == 0 then
    return ToolView.restore(output, view_opts(ctx))
  end
  return diff_view(lines, diff_blocks)
end
for _, name in ipairs({
  "compilerbrain.ReplaceMember",
  "compilerbrain.AddMember",
  "compilerbrain.DeleteMember",
  "compilerbrain.AddAttribute",
  "compilerbrain.BatchEdit",
}) do
  maki.api.register_tool_view({ name = name, restore = render_edit_result })
end
