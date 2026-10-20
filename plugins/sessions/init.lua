-- The /sessions picker: one flat list of every session in this directory,
-- live or stored. Live ones get a colored icon, and row order is frozen
-- while the picker is open so rows never jump around under the cursor
-- while background agents keep working.

local TextInput = require("maki.text_input")
local ListPicker = require("maki.list_picker")

local FILTER_PREFIX = "❯ "
local RENAME_PREFIX = "Rename: "
local CONFIRM_HINT = "  Ctrl+D again to delete"
local DELETE_FOCUSED_HINT = "Cannot delete the current session"
local RENAME_USAGE = "Usage: /rename <title>"
local EMPTY_HINT = "  No sessions yet. Press Ctrl+N to start one."
local NO_MATCHES_HINT = "  No matches"
local LOADING_HINT = "  Loading sessions…"
local CURRENT_LABEL = "current"
local TICK_MS = 100
local AGE_TICKS = 10
local GLOBAL_ON_HINT = "Showing sessions from all projects"
local GLOBAL_OFF_HINT = "Showing sessions from this project"
local ELLIPSIS = "…"
local PREVIEW_BYTES = 64 * 1024
local PREVIEW_FALLBACK_BYTES = 1024 * 1024
local PREVIEW_CHARS = 400
local PREVIEW_JOB_TIMEOUT_MS = 2000
local NO_OUTLINE_HINT = "No tool calls to outline yet"
local OUTLINE_DIFF_LINES = 40
local OUTLINE_KEYS = {
  { "←/Esc", "back" },
  { "↑↓", "line" },
  { "j/k", "other outline" },
  { "PgUp/PgDn", "page" },
}
-- Placeholder only: the host swaps "spinner:*"-styled spans for the live
-- animated frame, so working rows spin without this plugin redrawing.
local WORKING_ICON = "· "
local AGE_UNITS = {
  { 31536000, "y" },
  { 2592000, "mo" },
  { 604800, "w" },
  { 86400, "d" },
  { 3600, "h" },
  { 60, "m" },
}
local FILTER_KEYS = {
  { "Enter", "open" },
  { "Ctrl+N", "new" },
  { "Ctrl+R", "rename" },
  { "Ctrl+D", "delete" },
  { "→", "outline" },
  { "Ctrl+Shift+M", "all projects" },
}
local RENAME_KEYS = {
  { "Enter", "save" },
  { "Esc", "cancel" },
}

local board = nil

local function icon_of(s)
  if s.status == "needs_input" then
    return "◆ ", "warning"
  end
  if s.status == "working" then
    return WORKING_ICON, "accent", true
  end
  if s.focused then
    return "● ", "accent"
  end
  if s.live then
    return "○ ", "accent"
  end
  return "  ", "dim"
end

-- Current session first, then most recently opened.
local function by_recency(a, b)
  if a.focused ~= b.focused then
    return a.focused
  end
  return (a.updated_at or 0) > (b.updated_at or 0)
end

-- Rows keep their rank for the picker's lifetime; new ones enter above
-- existing ones so nothing already on screen moves. Within one batch
-- (notably the first full refresh) recency decides.
local function assign_ranks(fresh)
  table.sort(fresh, by_recency)
  local base = board.min_rank - #fresh
  for i, s in ipairs(fresh) do
    board.rank[s.id] = base + i
  end
  board.min_rank = base
end

local function update_footer()
  if board.rename then
    board.win:set_config({ footer = RENAME_KEYS })
    return
  end
  local footer = {}
  if board.counts.needs_input > 0 then
    footer[#footer + 1] = { "◆ " .. board.counts.needs_input, "needs input" }
  end
  if board.counts.working > 0 then
    footer[#footer + 1] = { "● " .. board.counts.working, "working" }
  end
  for _, f in ipairs(FILTER_KEYS) do
    footer[#footer + 1] = f
  end
  board.win:set_config({ footer = footer })
end

local function filter_words()
  return ListPicker.split_words(board.input:value())
end

-- A stored title is cut at 60 bytes (with "…") when maki derives it from the
-- first prompt. The untruncated prompt is still in the session log, so the
-- picker can spend the row's free columns on it instead of padding.
local function label_of(s)
  return board.previews[s.id] or s.title
end
local function sel_index()
  for i, s in ipairs(board.items) do
    if s.id == board.sel_id then
      return i
    end
  end
  return nil
end

local function find_stored(id)
  for i, st in ipairs(board.stored or {}) do
    if st.id == id then
      return i
    end
  end
  return nil
end

local function apply_filter()
  local prev_pos = sel_index() or 1
  local words = filter_words()
  board.items = {}
  for _, s in ipairs(board.all) do
    if ListPicker.matches(label_of(s), words) then
      board.items[#board.items + 1] = s
    end
  end
  local idx = sel_index() or math.min(prev_pos, math.max(#board.items, 1))
  board.sel_id = board.items[idx] and board.items[idx].id or nil
end

-- Selection restarts from the top on every query change, so clearing the
-- filter never leaves the list scrolled to wherever a match happened to sit.
local function filter_changed()
  board.sel_id = nil
  board.confirm = nil
  apply_filter()
end

local function age(updated_at)
  local secs = math.max(os.time() - (updated_at or 0), 0)
  for _, u in ipairs(AGE_UNITS) do
    if secs >= u[1] then
      return math.floor(secs / u[1]) .. u[2] .. " ago"
    end
  end
  return "just now"
end

local function format_context_size(tokens)
  tokens = tokens or 0
  if tokens >= 1000000 then
    return string.format("%.1fM", tokens / 1000000)
  elseif tokens >= 1000 then
    return string.format("%.1fK", tokens / 1000)
  end
  return tostring(tokens)
end

local function dispw(s)
  return utf8.len(s) or #s
end

local function log_path(id)
  local state = maki.env.state_dir()
  return state and (state .. "/sessions/" .. id .. ".jsonl")
end

-- User text hidden in a record too big to decode: an image-heavy first
-- message puts its text block megabytes behind the base64 payload, so the
-- head chunk ends mid-line. Pull the text blocks out of the raw JSON instead.
local function raw_texts(line)
  local parts = {}
  local pos = 1
  while true do
    local _, stop = line:find('"type":"text","text":"', pos, true)
    if not stop then
      break
    end
    local i, buf = stop + 1, {}
    while i <= #line do
      local c = line:sub(i, i)
      if c == '"' then
        break
      end
      if c == "\\" then
        buf[#buf + 1] = line:sub(i, i + 1)
        i = i + 2
      else
        buf[#buf + 1] = c
        i = i + 1
      end
    end
    local text = table.concat(buf):gsub("\\([nrt])", " "):gsub("\\(.)", "%1")
    text = text:gsub("%s+", " "):match("^%s*(.-)%s*$")
    if text ~= "" then
      parts[#parts + 1] = text
    end
    pos = i + 1
  end
  return parts
end

-- Every user text block in the head chunk, whitespace-normalized. The first
-- message may be an image and hand the title to a later prose message, so the
-- whole chunk is worth reading.
local function user_texts(chunk)
  local parts = {}
  for line in chunk:gmatch("[^\n]+") do
    -- Base64 image payloads make a line megabytes wide; decoding one only to
    -- throw the images away costs more than scanning its text blocks raw.
    local ok, rec = false, nil
    if #line <= PREVIEW_BYTES then
      ok, rec = pcall(maki.json.decode, line)
    end
    if ok and type(rec) == "table" and rec.t == "msg" then
      local d = rec.d
      if type(d) == "table" and d.role == "user" and type(d.content) == "table" then
        for _, block in ipairs(d.content) do
          if type(block) == "table" and block.type == "text" and type(block.text) == "string" then
            local text = block.text:gsub("%s+", " "):match("^%s*(.-)%s*$")
            if text ~= "" then
              parts[#parts + 1] = text
            end
            break
          end
        end
      end
    elseif not ok and line:find('"t":"msg"', 1, true) and line:find('"role":"user"', 1, true) then
      for _, text in ipairs(raw_texts(line)) do
        parts[#parts + 1] = text
      end
    end
  end
  return table.concat(parts, " ")
end

-- Only a title that really came from this prompt grows back: strip the "…"
-- the storage added and look for that prefix. A renamed session keeps its own
-- words even when they happen to end in an ellipsis.
local function extend_title(title, texts)
  local cut = title:match("^(.-)…$")
  if not cut or cut == "" then
    return nil
  end
  local at = texts:find(cut, 1, true)
  if not at then
    return nil
  end
  return maki.ui.truncate_text(texts:sub(at), PREVIEW_CHARS).head
end

-- Reads only the beginning of the log, so a 100 MB session costs one row.
local function read_head(path, bytes)
  local ok, id = pcall(maki.fn.jobstart, { "head", "-c", tostring(bytes), path }, { scope = "plugin" })
  if not ok or not id then
    return nil
  end
  local waited, result = pcall(maki.fn.jobwait, id, PREVIEW_JOB_TIMEOUT_MS)
  if not waited or not result then
    pcall(maki.fn.jobstop, id)
    return nil
  end
  return result.stdout
end

-- Reads previews off the main loop: one head job at a time, then a single
-- dirty tick so every loaded row reflows together.
local function load_previews()
  local this_board = board
  if board.preview_loading then
    return
  end
  local want = {}
  for _, s in ipairs(board.all) do
    if s.title:sub(-3) == ELLIPSIS and not board.previews[s.id] and not board.preview_tried[s.id] then
      want[#want + 1] = { id = s.id, title = s.title }
      board.preview_tried[s.id] = true
    end
  end
  if #want == 0 then
    return
  end
  board.preview_loading = true
  maki.async.run(function()
    for _, entry in ipairs(want) do
      if board ~= this_board then
        return
      end
      local path = log_path(entry.id)
      local chunk = path and read_head(path, PREVIEW_BYTES)
      local texts = chunk and user_texts(chunk)
      -- An image-heavy first message pushes its text past the first chunk;
      -- only those pay for the larger read.
      if texts == "" and path then
        chunk = read_head(path, PREVIEW_FALLBACK_BYTES)
        texts = chunk and user_texts(chunk) or ""
      end
      if texts and texts ~= "" then
        local full = extend_title(entry.title, texts)
        if full then
          board.previews[entry.id] = full
        end
      end
    end
    if board ~= this_board then
      return
    end
    board.preview_loading = false
    -- Sessions found while this batch ran (the stored scan landing late, or a
    -- new live one) need their own pass, so always ask for a refresh.
    board.dirty = true
  end)
end

-- One session, one row: the extra columns go to more of the title, cut right
-- before the right-aligned context column.
local function fit_label(label, width)
  if width <= 0 then
    return ""
  end
  if maki.ui.display_width(label) <= width then
    return label
  end
  return maki.ui.truncate_text(label, math.max(width - 1, 0)).head .. ELLIPSIS
end

local function render()
  local lines = {}
  local inner = board.width - 4
  local input = board.rename and board.rename.input or board.input
  local prefix = board.rename and RENAME_PREFIX or FILTER_PREFIX
  board.reserved = ListPicker.render_header(board.win, lines, input, prefix, inner)
  local cursor_line = board.reserved
  local words = filter_words()
  for i, s in ipairs(board.items) do
    local selected = s.id == board.sel_id
    local icon, icon_style, spinning = icon_of(s)
    local base = selected and "selected" or "item"
    local match_style = selected and "match_selected" or "match"
    local ctx = "ctx: " .. format_context_size(s.context_size) .. " · "
    local right = ctx .. (s.focused and CURRENT_LABEL or age(s.updated_at))
    local right_style = selected and "selected" or (s.focused and "accent" or "dim")
    if selected then
      icon_style = "selected"
    end
    -- Prefix after selection so a working row keeps animating host-side
    -- on the selection background.
    if spinning then
      icon_style = "spinner:" .. icon_style
    end
    local hint = board.confirm == s.id and CONFIRM_HINT or ""
    local icon_w = dispw(icon)
    local padw = 2 + icon_w
    local first_width = inner - padw - dispw(hint) - dispw(right) - 1
    local label = fit_label(label_of(s), first_width)
    local line = { { "  ", base }, { icon, icon_style } }
    for _, sp in ipairs(ListPicker.highlight_spans(label, words, base, match_style)) do
      line[#line + 1] = sp
    end
    local used = padw + maki.ui.display_width(label)
    if hint ~= "" then
      line[#line + 1] = { hint, selected and "match_selected" or "error" }
      used = used + dispw(hint)
    end
    local pad = math.max(inner - used - dispw(right), 1)
    line[#line + 1] = { string.rep(" ", pad), base }
    line[#line + 1] = { right, right_style }
    lines[#lines + 1] = line
    if selected then
      cursor_line = #lines
    end
  end
  if board.loading then
    lines[#lines + 1] = { { LOADING_HINT, "dim" } }
  elseif #board.items == 0 then
    lines[#lines + 1] = { { #board.all == 0 and EMPTY_HINT or NO_MATCHES_HINT, "dim" } }
  end
  board.buf:set_lines(lines)
  board.win:set_cursor(cursor_line)
end

-- Rebuilds the list from live runtimes and the stored snapshot, then
-- renders. Live runtimes win over their stored copies; stored-only sessions
-- are idle. Until the background scan lands (`board.stored`) only live
-- sessions are shown. `live()` suspends this coroutine, and the picker may
-- close or another refresh may finish meanwhile, so bail out unless this
-- board is still current.
local function refresh()
  local this_board = board
  local live, live_err = maki.session.live()
  if board ~= this_board then
    return
  end
  if live_err then
    maki.ui.flash(live_err)
    render()
    return
  end
  local seen, all = {}, {}
  for _, s in ipairs(live) do
    seen[s.id] = true
    s.live = true
    all[#all + 1] = s
  end
  for _, s in ipairs(board.stored or {}) do
    if not seen[s.id] then
      s.status = "idle"
      s.focused = false
      all[#all + 1] = s
    end
  end
  board.counts = { needs_input = 0, working = 0 }
  for _, s in ipairs(all) do
    if board.counts[s.status] then
      board.counts[s.status] = board.counts[s.status] + 1
    end
  end
  if board.loading then
    table.sort(all, by_recency)
  else
    local fresh = {}
    for _, s in ipairs(all) do
      if not board.rank[s.id] then
        fresh[#fresh + 1] = s
      end
    end
    if #fresh > 0 then
      assign_ranks(fresh)
    end
    table.sort(all, function(a, b)
      if a.focused ~= b.focused then
        return a.focused
      end
      return board.rank[a.id] < board.rank[b.id]
    end)
  end
  board.all = all
  apply_filter()
  update_footer()
  render()
  load_previews()
end

local function close()
  if board then
    board.win:close()
    board = nil
  end
end

local function selected()
  local idx = sel_index()
  return idx and board.items[idx] or nil
end

local function set_sel(i)
  board.sel_id = board.items[i] and board.items[i].id or nil
  board.confirm = nil
  render()
end

local function move_sel(delta, wrap)
  local n = #board.items
  if n == 0 then
    return
  end
  local cur = sel_index() or 1
  if wrap then
    set_sel((cur - 1 + delta) % n + 1)
  else
    set_sel(math.min(math.max(cur + delta, 1), n))
  end
end

local function page_size()
  return math.max(board.height - board.reserved - 1, 1)
end

local function open_selected()
  local s = selected()
  if not s then
    return
  end
  if not s.focused then
    local _, err = maki.session.focus(s.id)
    if err then
      maki.ui.flash(err)
      return
    end
  end
  close()
end

local function open_blank()
  local _, err = maki.session.new({ focus = true })
  if err then
    maki.ui.flash(err)
    return
  end
  close()
end

local function delete_selected()
  local s = selected()
  if not s then
    return
  end
  if s.focused then
    maki.ui.flash(DELETE_FOCUSED_HINT)
    return
  end
  if board.confirm ~= s.id then
    board.confirm = s.id
    render()
    return
  end
  board.confirm = nil
  local _, err = maki.session.delete(s.id)
  if err then
    maki.ui.flash(err)
    return
  end
  board.deleted[s.id] = true
  local si = find_stored(s.id)
  if si then
    table.remove(board.stored, si)
  end
  refresh()
end

local function start_rename()
  local s = selected()
  if not s then
    return
  end
  local input = TextInput.new()
  input:insert_text(s.title)
  board.rename = { id = s.id, input = input }
  board.confirm = nil
  update_footer()
  render()
end

local function stop_rename()
  board.rename = nil
  update_footer()
  render()
end

local function commit_rename()
  local title = board.rename.input:value():match("^%s*(.-)%s*$")
  local id = board.rename.id
  stop_rename()
  if title == "" then
    return
  end
  local _, err = maki.session.set_title({ id = id, title = title })
  if err then
    maki.ui.flash(err)
  else
    board.previews[id] = nil
    local si = find_stored(id)
    if si then
      board.stored[si].title = title
    end
  end
  refresh()
end

local function handle_rename_key(key)
  if key == "<Esc>" then
    stop_rename()
  elseif key == "<CR>" then
    commit_rename()
  elseif key ~= "<Up>" and key ~= "<Down>" then
    if board.rename.input:handle_key(key) ~= "ignored" then
      render()
    end
  end
end

-- Reloads the stored-session scan for the board's current `global` scope.
-- Called on open and again whenever the scope is toggled.
local function load_stored()
  local this_board = board
  board.loading = true
  board.stored = nil
  refresh()
  maki.async.run(function()
    local stored, err = maki.session.list({ global = board.global })
    if board ~= this_board then
      return
    end
    if err then
      maki.ui.flash(err)
      stored = {}
    end
    -- A delete may have landed while the scan was in flight; never let the
    -- stale snapshot resurrect that session as a ghost row.
    local kept = {}
    for _, st in ipairs(stored) do
      if not board.deleted[st.id] then
        kept[#kept + 1] = st
      end
    end
    board.stored = kept
    board.loading = false
    refresh()
  end)
end

local function toggle_global()
  board.global = not board.global
  maki.ui.flash(board.global and GLOBAL_ON_HINT or GLOBAL_OFF_HINT)
  load_stored()
end

local function outline_line(text)
  if text == "" then
    return {}
  end
  local style = "item"
  if text:match("^## ") then
    style = "keybind_section"
  elseif text:match("^# ") then
    style = "accent"
  elseif text:match("^%$ ") then
    style = "dim"
  end
  return { { text, style } }
end

-- The same report session_history's "outline" format asks for: model replies
-- interleaved with every command, thinking excluded, diff off. It is Markdown,
-- so it goes through the normal renderer for headings and inline code.
local function report_lines(session, width)
  local ok, report, err = pcall(maki.session.summary, session.id, {
    transcript = true,
    diff = false,
    diff_lines = OUTLINE_DIFF_LINES,
  })
  if not ok then
    report, err = nil, tostring(report)
  end
  if not report then
    local reason = err or "this session has no report yet"
    if reason:find("no tool calls", 1, true) then
      reason = NO_OUTLINE_HINT
    end
    return {
      { { "Outline not available", "error" } },
      { { reason, "dim" } },
    }
  end
  local rendered_ok, rendered = pcall(maki.ui.markdown, report, width)
  if rendered_ok and rendered and #rendered > 0 then
    return rendered
  end
  local lines = {}
  for text in (report .. "\n"):gmatch("(.-)\n") do
    lines[#lines + 1] = outline_line(text)
  end
  return lines
end

local function outline_title(session)
  local label = board and label_of(session) or session.title
  return " Outline · " .. maki.ui.truncate_text(label, 50).head .. " "
end

local function show_outline()
  local s = selected()
  if not s then
    return
  end
  local buf = maki.ui.buf()
  local win = maki.ui.open_win(buf, {
    title = outline_title(s),
    width = "100%",
    height = "90%",
    border = "rounded",
    focus = true,
    footer = OUTLINE_KEYS,
  })
  local width = win.width
  local shown = s
  local lines = {}
  local cursor = 1

  local function show(session)
    shown = session
    lines = report_lines(session, width)
    buf:set_lines(lines)
    win:set_config({ title = outline_title(session) })
    cursor = 1
    win:set_cursor(cursor)
  end

  -- j/k walks the outline views, not the picker: the selection stays where it
  -- was and ← returns to it, but the pane shows the neighbour's report.
  local index = 1
  for i, item in ipairs(board.items) do
    if item.id == s.id then
      index = i
      break
    end
  end
  local function switch(delta)
    local count = board and #board.items or 0
    if count == 0 then
      return
    end
    index = (index - 1 + delta) % count + 1
    show(board.items[index])
  end

  show(s)
  while win:is_open() do
    local ev = win:recv()
    if not ev or ev.type == "close" then
      break
    end
    if ev.type == "resize" then
      width = ev.width
      lines = report_lines(shown, width)
      buf:set_lines(lines)
      cursor = math.min(cursor, math.max(#lines, 1))
      win:set_cursor(cursor)
    elseif ev.type == "key" then
      local key = ev.key
      local page = math.max(win.height - 1, 1)
      if key == "<Esc>" or key == "<Left>" or key == "<C-c>" or key == "q" then
        win:close()
        break
      elseif key == "j" then
        switch(1)
      elseif key == "k" then
        switch(-1)
      elseif key == "<Up>" then
        cursor = math.max(cursor - 1, 1)
      elseif key == "<Down>" then
        cursor = math.min(cursor + 1, math.max(#lines, 1))
      elseif key == "<PageUp>" then
        cursor = math.max(cursor - page, 1)
      elseif key == "<PageDown>" then
        cursor = math.min(cursor + page, math.max(#lines, 1))
      elseif key == "<Home>" then
        cursor = 1
      elseif key == "<End>" then
        cursor = math.max(#lines, 1)
      end
      win:set_cursor(cursor)
    end
  end
end

local function handle_key(key)
  if key == "<C-c>" then
    close()
  elseif board.rename then
    handle_rename_key(key)
  elseif key == "<Esc>" then
    if board.confirm then
      board.confirm = nil
      render()
    elseif not board.input:is_empty() then
      board.input:clear()
      filter_changed()
      render()
    else
      close()
    end
  elseif key == "<Up>" then
    move_sel(-1, true)
  elseif key == "<Down>" then
    move_sel(1, true)
  elseif key == "<PageUp>" then
    move_sel(-page_size())
  elseif key == "<PageDown>" then
    move_sel(page_size())
  elseif key == "<CR>" then
    open_selected()
  elseif key == "<C-n>" then
    open_blank()
  elseif key == "<Right>" then
    show_outline()
  elseif key == "<C-r>" then
    start_rename()
  elseif key == "<C-d>" then
    delete_selected()
  elseif key == "<C-S-m>" then
    toggle_global()
  else
    local r = board.input:handle_key(key)
    if r ~= "ignored" then
      if r == "changed" then
        filter_changed()
      else
        board.confirm = nil
      end
      render()
    end
  end
end

local function open()
  if board then
    return
  end
  local buf = maki.ui.buf()
  local win = maki.ui.open_win(buf, {
    title = " Sessions ",
    width = "70%",
    height = "70%",
    border = "rounded",
    focus = true,
    footer = FILTER_KEYS,
  })
  board = {
    win = win,
    buf = buf,
    width = win.width,
    height = win.height,
    reserved = 0,
    input = TextInput.new(),
    all = {},
    items = {},
    rank = {},
    deleted = {},
    previews = {},
    preview_tried = {},
    preview_loading = false,
    min_rank = 0,
    counts = { needs_input = 0, working = 0 },
    sel_id = nil,
    frame = 0,
    loading = true,
    global = false,
  }
  -- Two-phase load: live sessions are cheap, so they show up and take keys
  -- right away; the stored scan can be slow, so a background task merges it
  -- in once it lands.
  refresh()
  load_stored()
  while board do
    local ev = board.win:recv(TICK_MS)
    if not ev or ev.type == "close" then
      board = nil
    elseif ev.type == "timeout" then
      board.frame = board.frame + 1
      if board.dirty then
        board.dirty = false
        refresh()
      elseif board.frame % AGE_TICKS == 0 then
        render()
      end
    elseif ev.type == "key" then
      handle_key(ev.key)
    elseif ev.type == "paste" then
      local input = board.rename and board.rename.input or board.input
      input:insert_text(ev.text)
      if not board.rename then
        filter_changed()
      else
        board.confirm = nil
      end
      render()
    elseif ev.type == "resize" then
      board.width = ev.width
      board.height = ev.height
      render()
    end
  end
end

-- A background agent flipping state deserves a heads-up even with the picker
-- closed, so the flash names the session and where to find it. Autocmds run
-- synchronously while refresh needs an async roundtrip, so the dirty flag
-- defers it to the next tick of the recv loop.
local last_status = {}
maki.api.create_autocmd("SessionStatusChanged", {
  callback = function(ev)
    local d = ev.data or {}
    local prev = last_status[d.session_id]
    last_status[d.session_id] = d.status
    if not d.focused then
      if d.status == "needs_input" then
        maki.ui.flash("◆ " .. d.title .. " needs input · /sessions")
      elseif d.status == "idle" and prev == "working" then
        maki.ui.flash("✓ " .. d.title .. " finished · /sessions")
      end
    end
    if board then
      board.dirty = true
    end
  end,
})

maki.api.register_command({
  name = "/sessions",
  description = "Browse and switch sessions",
  handler = open,
})

-- Upstream binds session browsing to <C-p>. This fork keeps Ctrl+P on
-- prev_chat and reaches sessions through the configurable `sessions` bind
-- (Alt+S by default). Lua keymaps dispatch ahead of the native binds, so
-- restoring this line silently shadows prev_chat.

maki.api.register_command({
  name = "/rename",
  description = "Rename the current session",
  nargs = "+",
  handler = function(opts)
    local title = opts.args:match("^%s*(.-)%s*$")
    if title == "" then
      maki.ui.flash(RENAME_USAGE)
      return
    end
    local id, err = maki.session.current()
    if err then
      maki.ui.flash(err)
      return
    end
    local _, set_err = maki.session.set_title({ id = id, title = title })
    maki.ui.flash(set_err or ('Renamed to "' .. title .. '"'))
  end,
})
