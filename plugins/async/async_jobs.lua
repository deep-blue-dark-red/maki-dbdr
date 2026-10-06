-- Auto-showing background-jobs panel (like the todo panel) plus an
-- interactive picker to inspect a job's stdout and kill it. Wired into
-- plugins/async: the panel appears while jobs run and hides when idle.

local lib = require("async_lib")
local ListPicker = require("maki.list_picker")

local TICK_MS = 500
local DETAIL_TICK_MS = 250
-- The stdout panel shows essentially the whole output; the cap only exists
-- so a runaway job cannot balloon the buf without bound.
local DETAIL_MAX_LINES = 5000
local PAGE_LINES = 15
local PANEL_TITLE = " Jobs "
local DEFAULT_TOGGLE_KEY = "<C-g>"
local KEYMAP_DESC = "Inspect background jobs"

local toggle_key = DEFAULT_TOGGLE_KEY

-- Footer labels spell the key the way the docs do: <C-g> -> Ctrl+G.
local function key_label(k)
  local c = k:match("^<C%-(%w+)>")
  if c then
    return "Ctrl+" .. c:upper()
  end
  local a = k:match("^<M%-(%w+)>")
  if a then
    return "Alt+" .. a:upper()
  end
  return k
end

local get_jobs, kill
local buf, win
local ticking = false

local function active(jobs)
  local out = {}
  for _, job in ipairs(jobs) do
    if not lib.TERMINAL[job.status] then
      out[#out + 1] = job
    end
  end
  return out
end

local function ensure_win()
  if buf and win and win:is_open() then
    return
  end
  buf = maki.ui.buf()
  win = maki.ui.open_win(buf, {
    split = "panel",
    height = 3,
    order = 11,
    title = PANEL_TITLE,
    border = "rounded",
    focus = false,
    visible = false,
    footer = { { key_label(toggle_key), "inspect" } },
  })
end

local function render()
  if not get_jobs then
    return
  end
  local jobs = active(get_jobs())
  if #jobs == 0 then
    if win and win:is_open() then
      win:hide()
    end
    maki.ui.set_status_hint(nil)
    return
  end
  ensure_win()
  buf:set_lines(lib.build_panel_lines(jobs))
  win:set_config({ height = #jobs + 2 })
  win:show()
  maki.ui.set_status_hint({
    { string.format(" %d jobs ", #jobs), "foreground" },
    { key_label(toggle_key), "keybind_key" },
    { " ", "" },
  })
end

local function tick()
  if not get_jobs then
    ticking = false
    return
  end
  render()
  if #active(get_jobs()) > 0 then
    maki.defer_fn(tick, TICK_MS)
  else
    ticking = false
  end
end

-- Coalesce: render now for immediate feedback, and keep one tick alive while
-- jobs are active so elapsed time and status keep moving.
local function refresh()
  render()
  if not ticking and get_jobs and #active(get_jobs()) > 0 then
    ticking = true
    maki.defer_fn(tick, TICK_MS)
  end
end

local function find_job(id)
  return get_jobs and lib.find(get_jobs(), id) or nil
end

-- Rows for the picker, rebuilt only when something changed. Returning nil
-- from a refresh callback makes ListPicker skip the swap, so an idle queue
-- costs a string compare per poll instead of a full re-render.
local last_sig, last_items
local function detail_sig(detail)
  if type(detail) ~= "table" then
    return detail or ""
  end
  local parts = {}
  for _, p in ipairs(detail) do
    parts[#parts + 1] = p[1]
  end
  return table.concat(parts, "\1")
end

local function fresh_items()
  local items = lib.build_items(get_jobs())
  local sig = {}
  for i, it in ipairs(items) do
    sig[i] = it.id .. "\1" .. it.label .. "\1" .. detail_sig(it.detail)
  end
  local s = table.concat(sig, "\2")
  if s == last_sig then
    return nil
  end
  last_sig, last_items = s, items
  return items
end

-- The stdout split. It stays open across picker focus switches; the picker
-- owns focus while open, and Tab hands it over either way.
local stdout -- { win, buf, job, last, cursor, line_count }

local function refresh_stdout()
  local st = stdout
  if not st or not st.win:is_open() then
    return
  end
  local job = st.job
  local running = not lib.TERMINAL[job.status]
  local live = running and job.live_buf and job.live_buf:get_lines() or nil
  local lines = lib.job_output_lines(job, live, DETAIL_MAX_LINES)
  local text = table.concat(lines, "\n")
  if text ~= st.last then
    -- Follow the stream while the view already sits at the bottom; a user
    -- who scrolled up keeps their place.
    local pinned = st.cursor >= st.line_count
    st.last = text
    st.line_count = #lines
    st.buf:set_lines(lines)
    if pinned then
      st.cursor = st.line_count
      st.win:set_cursor(st.cursor)
    else
      st.cursor = math.min(st.cursor, st.line_count)
    end
  end
end

local function open_stdout(job)
  if stdout and stdout.win:is_open() then
    if stdout.job.id == job.id then
      refresh_stdout()
      return
    end
    stdout.win:close()
  end
  local b = maki.ui.buf()
  local cmd = lib.job_command(job)
  local w = maki.ui.open_win(b, {
    split = "right",
    width = "33%",
    title = " " .. job.id .. " stdout · " .. cmd .. " ",
    border = "rounded",
    focus = false,
    footer = { { "Tab", "jobs" }, { "Esc", "close" }, { "↑↓", "line" }, { "PgUp/PgDn", "page" } },
  })
  stdout = { win = w, buf = b, job = job, last = nil, cursor = 1, line_count = 0 }
  refresh_stdout()
end

-- Runs while the stdout window holds focus. Returns "picker" when the user
-- Tabs back to the jobs list, nil when the window closed.
local function stdout_focus_loop()
  local st = stdout
  while st.win:is_open() do
    refresh_stdout()
    -- Settled output never changes: block on keys instead of redrawing.
    -- recv's timeout event drives the running redraw; nil means the window
    -- is gone. The cursor rides the scroll position, so arrow keys scroll;
    -- the wheel scrolls the focused window on its own.
    local ev = st.win:recv(not lib.TERMINAL[st.job.status] and DETAIL_TICK_MS or nil)
    if not ev or ev.type == "close" then
      return nil
    elseif ev.type == "key" then
      local key = ev.key
      if key == "<Esc>" or key == "<C-c>" or key == "q" then
        st.win:close()
        return nil
      elseif key == "<Tab>" then
        return "picker"
      elseif key == "<Up>" or key == "<Down>" then
        st.cursor = math.max(1, math.min(st.cursor + (key == "<Down>" and 1 or -1), st.line_count))
        st.win:set_cursor(st.cursor)
      elseif key == "<PageUp>" or key == "<PageDown>" then
        st.cursor =
          math.max(1, math.min(st.cursor + (key == "<PageDown>" and PAGE_LINES or -PAGE_LINES), st.line_count))
        st.win:set_cursor(st.cursor)
      end
    end
  end
  return nil
end

local function open_picker()
  if #get_jobs() == 0 then
    maki.ui.flash("no background jobs")
    return
  end
  -- One flat loop owns both windows: Enter shows a job's stdout without
  -- leaving the picker, Tab hops focus between picker and stdout, and the
  -- stdout split survives every switch until Esc closes it.
  local picker_focused = true
  -- Coming back lands the cursor on the job you were reading.
  local selected_id
  while picker_focused or (stdout and stdout.win:is_open()) do
    if picker_focused then
      local items = fresh_items() or last_items or {}
      local cursor = 1
      if selected_id then
        for i, it in ipairs(items) do
          if it.id == selected_id then
            cursor = i
            break
          end
        end
      end
      local res = ListPicker.open(items, {
        title = PANEL_TITLE,
        footer = { { "Enter", "stdout" }, { "Tab", "stdout" }, { "K", "kill" }, { "Esc", "close" } },
        key = function(item)
          return item.id
        end,
        cursor = cursor,
        submit_swaps = true,
        action_keys = { "<Tab>" },
        -- Statuses, elapsed times, new and killed jobs keep moving while the
        -- picker is open; rows are keyed by id, so the cursor follows the
        -- selected job across swaps.
        refresh = function()
          refresh_stdout()
          return fresh_items()
        end,
        refresh_ms = DETAIL_TICK_MS,
        live_keys = {
          -- Kills in place and swaps the list, so the picker stays open.
          ["K"] = function(item)
            local job = item and find_job(item.id)
            if job then
              maki.ui.flash(kill(job))
            end
            return fresh_items()
          end,
          -- Shows the job in the split and keeps the picker open.
          ["<CR>"] = function(item)
            local job = item and find_job(item.id)
            if job then
              selected_id = job.id
              open_stdout(job)
            end
            return fresh_items()
          end,
        },
      })
      if res.type == "choice" then
        local job = res.item and find_job(res.item.id)
        if job then
          selected_id = job.id
          open_stdout(job)
        end
      elseif res.type == "key" and res.key == "<Tab>" then
        -- Hand focus to the stdout split; the picker window is closed.
        if stdout and stdout.win:is_open() then
          stdout.win:set_config({ focus = true })
          picker_focused = false
        end
      elseif res.type == "close" then
        return
      end
    else
      if stdout_focus_loop() == "picker" then
        picker_focused = true
      end
    end
  end
end

local function setup(get, kill_fn, key)
  get_jobs, kill = get, kill_fn
  toggle_key = key or DEFAULT_TOGGLE_KEY
  maki.keymap.set("n", toggle_key, open_picker, { desc = KEYMAP_DESC })
end

return {
  setup = setup,
  refresh = refresh,
}
