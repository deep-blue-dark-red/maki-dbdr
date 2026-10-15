-- Terminal tab progress (OSC 9;4) for Ghostty and any terminal that draws it:
-- a spinner while a session works, a paused bar while one waits on you, an
-- error bar after a failed turn, nothing when idle. Maki clears it on exit.
--
-- It also keeps the window title in Claude Code's shape, "<prefix> maki - cwd"
-- with a braille spinner while working and a star otherwise, so switchers that
-- read titles (haunt's claude-status hook) can colour working and finished
-- maki rows the way they do Claude's. Maki hands the title back on exit.

local progress = require("terminal_progress")

local states = {}
local shown
local shown_title

local function render()
  -- Headless drivers have no terminal to draw on and no ui functions.
  if not (maki.ui.set_progress and maki.ui.set_window_title) then
    return
  end
  local want = progress.combine(states)
  if want ~= shown then
    shown = want
    maki.ui.set_progress(want)
  end
  local cwd = maki.uv.cwd()
  if cwd then
    local title = progress.title(want, cwd, maki.uv.os_getenv("HOME"))
    if title ~= shown_title then
      shown_title = title
      maki.ui.set_window_title(title)
    end
  end
end

maki.api.create_autocmd({
  "TurnStart",
  "TurnEnd",
  "TurnError",
  "ToolStart",
  "SessionStatusChanged",
  "SessionEnd",
}, {
  callback = function(ev)
    local sid = (ev.data or {}).session_id
    local state = progress.session_state(ev)
    if not sid or not state then
      return
    end
    -- An error stays on the tab until the next turn clears it; the idle
    -- status change that follows a failed turn must not wipe it.
    if states[sid] == progress.FAILED and ev.event == "SessionStatusChanged" and state == progress.OFF then
      return
    end
    states[sid] = state ~= progress.OFF and state or nil
    render()
  end,
})

-- Show the idle title from the start rather than from the first turn, so a
-- fresh session already reads as a maki row to a title-reading switcher.
render()
