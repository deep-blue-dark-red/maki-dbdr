-- External status feed for other programs.
--
-- One NDJSON file per maki process at <state_dir>/status/<pid>.ndjson, one
-- line per change, so anything that can read a file (tail -F, ssh tail, a
-- watcher in any language) sees every session of this maki in real time. The
-- pid in the name lets a consumer check liveness (`kill -0 <pid>`), and the
-- file is removed when the last session ends. It appears on the first session
-- this plugin sees and is created empty before the first line.
--
-- Every line is a complete session snapshot: consumers replace their state
-- for that session_id and treat a missing key as "not set" (no error, no
-- priced cost, task not known yet). Shape:
--
--   {"event":"status","ts":<unix secs>,"session_id":"...","focused":true,
--    "status":"working|needs_input|idle","error":"...","model":"provider/id",
--    "context_size":123,"context_window":200000,"cost":0.42,"task":"first prompt"}
--   {"event":"end","ts":<unix secs>,"session_id":"..."}
--
-- "task" is the session's first user prompt: taken from the transcript when
-- it is already there, else the first prompt sent while this plugin watched.
-- A heartbeat restates every session every HEARTBEAT_MS so the file mtime
-- proves the process is alive and a consumer that joins mid-run gets state
-- without waiting for the next change.
--
-- Turn events cover the TUI, `maki -p` and sdk mode. ACP fires none yet. No
-- session call happens at load: a host without a driver would never answer
-- the roundtrip, so the feed starts on the first event instead.

local helpers = require("status_helpers")

local HEARTBEAT_MS = 30000

local file
local states = {}
local started = false

local function log_err(what, err)
  maki.log.warn("status: " .. what .. " failed: " .. tostring(err))
end

-- Creating the feed file starts it empty: a pid reused after a crash must
-- not append to the dead run's feed, and neither must the next session after
-- the previous file was removed.
local function open_file()
  if file then
    return
  end
  local state_dir = maki.env.state_dir()
  if not state_dir then
    return
  end
  local dir = maki.fs.joinpath(state_dir, "status")
  local ok, err = maki.fs.mkdir(dir, { parents = true })
  if not ok then
    log_err("mkdir", err)
    return
  end
  local path = maki.fs.joinpath(dir, tostring(maki.uv.os_getpid()) .. ".ndjson")
  local wok, werr = maki.fs.write(path, "")
  if not wok then
    log_err("create", werr)
    return
  end
  file = path
end

local function emit(line)
  open_file()
  if not file then
    return
  end
  local encoded, err = maki.json.encode(line)
  if not encoded then
    log_err("encode", err)
    return
  end
  local ok, aerr = maki.fs.append(file, encoded .. "\n")
  if not ok then
    log_err("append", aerr)
  end
end

local function fetch_task(state)
  if state.task then
    return
  end
  local msgs = maki.session.messages({ session = state.session_id })
  state.task = helpers.task_from_messages(msgs)
end

local function ensure(session_id)
  local state = states[session_id]
  if state then
    return state
  end
  local snap = maki.session.read({ session = session_id })
  state = helpers.new_state(session_id, snap or {})
  states[session_id] = state
  fetch_task(state)
  return state
end

local function sweep()
  local live, err = maki.session.live()
  if not live then
    log_err("live", err)
    return
  end
  for _, session in ipairs(live) do
    emit(helpers.line(ensure(session.id), os.time()))
  end
end

local function heartbeat()
  local ts = os.time()
  for _, state in pairs(states) do
    emit(helpers.line(state, ts))
  end
end

local function tick()
  heartbeat()
  maki.defer_fn(tick, HEARTBEAT_MS)
end

local function handle(ev)
  if not started then
    started = true
    sweep()
    maki.defer_fn(tick, HEARTBEAT_MS)
  end
  local data = ev.data or {}
  local sid = data.session_id
  if not sid then
    return
  end
  if ev.event == "SessionFocusChanged" then
    local prev = data.previous_session_id
    if prev and states[prev] and helpers.set_focused(states[prev], false) then
      emit(helpers.line(states[prev], os.time()))
    end
    local state = ensure(sid)
    if helpers.set_focused(state, true) then
      emit(helpers.line(state, os.time()))
    end
    return
  end
  if ev.event == "SessionEnd" then
    local state = states[sid]
    states[sid] = nil
    if not state or helpers.apply(state, ev) ~= "end" then
      return
    end
    emit(helpers.end_line(sid, os.time()))
    if next(states) == nil and file then
      maki.fs.rm(file, { force = true })
      file = nil
    end
    return
  end
  local state = ensure(sid)
  if ev.event == "TurnStart" or ev.event == "TurnEnd" then
    fetch_task(state)
  end
  if helpers.apply(state, ev) then
    emit(helpers.line(state, os.time()))
  end
end

maki.api.create_autocmd({
  "TurnStart",
  "TurnEnd",
  "TurnError",
  "ToolStart",
  "CompactionDone",
  "ModelChanged",
  "SessionStatusChanged",
  "SessionFocusChanged",
  "SessionEnd",
}, { callback = handle })
