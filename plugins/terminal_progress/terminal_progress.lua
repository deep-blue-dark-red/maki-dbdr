-- Pure logic behind the terminal progress indicator: which OSC 9;4 state a set
-- of sessions adds up to. The sessions share one terminal tab, so the most
-- urgent one wins: an error outranks a wait, a wait outranks work.

local M = {}

M.OFF = "off"
M.WORKING = "indeterminate"
M.WAITING = "paused"
M.FAILED = "error"

local STATUS_WORKING = "working"
local STATUS_NEEDS_INPUT = "needs_input"

local RANK = { [M.OFF] = 0, [M.WORKING] = 1, [M.WAITING] = 2, [M.FAILED] = 3 }

-- Folds one autocmd payload into a session's progress state, or returns nil
-- when the event says nothing about it.
function M.session_state(ev)
  local data = ev.data or {}
  if ev.event == "TurnStart" or ev.event == "ToolStart" then
    return M.WORKING
  elseif ev.event == "TurnEnd" then
    return M.OFF
  elseif ev.event == "TurnError" then
    return M.FAILED
  elseif ev.event == "SessionEnd" then
    -- /reload rebuilds the host and the session carries on in the new one.
    return data.reason ~= "reload" and M.OFF or nil
  elseif ev.event == "SessionStatusChanged" then
    if data.status == STATUS_NEEDS_INPUT then
      return M.WAITING
    elseif data.status == STATUS_WORKING then
      return M.WORKING
    end
    return M.OFF
  end
  return nil
end

-- Claude Code's title prefixes, which tab switchers such as haunt read to tell
-- a working agent (braille spinner) from one waiting on you (star).
M.TITLE_WORKING = "\u{2802}"
M.TITLE_IDLE = "\u{2733}"

-- "<prefix> maki - ~/dir": the shell-integration shape (`process - cwd`) behind
-- a status prefix, so a switcher that strips the prefix still sees the usual
-- row.
function M.title(state, cwd, home)
  if home and home ~= "" and cwd:sub(1, #home) == home and (#cwd == #home or cwd:sub(#home + 1, #home + 1) == "/") then
    cwd = "~" .. cwd:sub(#home + 1)
  end
  local prefix = state == M.WORKING and M.TITLE_WORKING or M.TITLE_IDLE
  return prefix .. " maki - " .. cwd
end

-- The state one tab shows for every live session.
function M.combine(states)
  local best = M.OFF
  for _, state in pairs(states) do
    if RANK[state] > RANK[best] then
      best = state
    end
  end
  return best
end

return M
