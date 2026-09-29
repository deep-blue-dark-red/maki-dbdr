-- Pure logic behind the external status feed. Every line the feed writes is a
-- complete snapshot of one session, so a consumer replaces its state per
-- session and can join late: whatever it missed is restated by the next line.

local M = {}

local STATUS_IDLE = "idle"
local STATUS_WORKING = "working"
local STATUS_RELOAD = "reload"

function M.new_state(session_id, snap)
  return {
    session_id = session_id,
    status = snap.status or STATUS_IDLE,
    focused = snap.focused or false,
    model = snap.model,
    context_size = snap.context_size or 0,
    context_window = snap.context_window or 0,
    cost = snap.cost,
    task = nil,
    error = nil,
  }
end

local function changed(state, patch)
  local dirty = false
  for key, value in pairs(patch) do
    if state[key] ~= value then
      state[key] = value
      dirty = true
    end
  end
  return dirty
end

function M.set_focused(state, focused)
  return changed(state, { focused = focused })
end

-- The session's first user prompt, the task the session was opened for.
function M.task_from_messages(msgs)
  for _, msg in ipairs(msgs or {}) do
    if msg.role == "user" and msg.kind == "turn" and not msg.hidden then
      local texts = {}
      for _, block in ipairs(msg.content or {}) do
        if block.type == "text" and block.text and block.text ~= "" then
          texts[#texts + 1] = block.text
        end
      end
      if #texts > 0 then
        return table.concat(texts, "\n")
      end
    end
  end
  return nil
end

-- Folds one autocmd payload into state. Returns "status" when the state moved
-- and a line is worth writing, "end" when the session is gone, nil otherwise.
function M.apply(state, ev)
  local data = ev.data or {}
  if ev.event == "TurnStart" then
    local dirty = changed(state, { status = STATUS_WORKING })
    if state.task == nil and data.text then
      state.task = data.text
      dirty = true
    end
    if state.error ~= nil then
      state.error = nil
      dirty = true
    end
    return dirty and "status" or nil
  elseif ev.event == "ToolStart" then
    -- Headless drivers fire no TurnStart, so the first tool call is the
    -- earliest "it is working" signal there.
    return changed(state, { status = STATUS_WORKING }) and "status" or nil
  elseif ev.event == "TurnError" then
    return changed(state, { status = STATUS_IDLE, error = data.message }) and "status" or nil
  elseif ev.event == "TurnEnd" then
    return changed(state, {
      status = STATUS_IDLE,
      context_size = data.context_size,
      context_window = data.context_window,
      cost = data.cost,
    }) and "status" or nil
  elseif ev.event == "CompactionDone" then
    return changed(state, {
      context_size = data.context_size_after,
      context_window = data.context_window,
    }) and "status" or nil
  elseif ev.event == "ModelChanged" then
    local spec = data.model and data.model.spec
    return changed(state, { model = spec }) and "status" or nil
  elseif ev.event == "SessionStatusChanged" then
    return changed(state, { status = data.status, focused = data.focused }) and "status" or nil
  elseif ev.event == "SessionEnd" then
    -- /reload rebuilds the plugin host and the session carries on in the new
    -- one, so only that reason is not a goodbye.
    if data.reason == STATUS_RELOAD then
      return nil
    end
    return "end"
  end
  return nil
end

function M.line(state, ts)
  local line = {
    event = "status",
    ts = ts,
    session_id = state.session_id,
    focused = state.focused,
    status = state.status,
    model = state.model,
    context_size = state.context_size,
    context_window = state.context_window,
    cost = state.cost,
    error = state.error,
    task = state.task,
  }
  return line
end

function M.end_line(session_id, ts)
  return { event = "end", ts = ts, session_id = session_id }
end

return M
