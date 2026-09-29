local h = require("status_helpers")
local th = require("maki.test_helpers")

local case = th.case
local eq = th.eq

local SESSION = "session-x"
local TS = 1700000000
local TASK = "implement the thing"
local MODEL = "zai/glm-4.7"
local ERROR_MSG = "provider exploded"

local function state(snap)
  return h.new_state(SESSION, snap or {})
end

case("new_state_defaults_are_a_quiet_unpriced_session", function()
  local s = h.new_state(SESSION, {})
  eq(s.session_id, SESSION)
  eq(s.status, "idle")
  eq(s.focused, false)
  eq(s.context_size, 0)
  eq(s.context_window, 0)
  eq(s.model, nil)
  eq(s.cost, nil)
  eq(s.task, nil)
  eq(s.error, nil)
end)

case("new_state_maps_the_snapshot", function()
  local s = h.new_state(SESSION, {
    status = "needs_input",
    focused = true,
    model = MODEL,
    context_size = 12,
    context_window = 200,
    cost = 0.5,
  })
  eq(s.status, "needs_input")
  eq(s.focused, true)
  eq(s.model, MODEL)
  eq(s.context_size, 12)
  eq(s.context_window, 200)
  eq(s.cost, 0.5)
end)

case("task_is_the_first_visible_user_turn", function()
  local msgs = {
    { role = "user", kind = "observation", hidden = false, content = { { type = "text", text = "noise" } } },
    { role = "user", kind = "turn", hidden = true, content = { { type = "text", text = "nudge" } } },
    { role = "user", kind = "turn", hidden = false, content = { { type = "text", text = TASK } } },
    { role = "assistant", kind = "turn", hidden = false, content = { { type = "text", text = "answer" } } },
  }
  eq(h.task_from_messages(msgs), TASK)
end)

case("task_joins_text_blocks_and_skips_other_blocks", function()
  local msgs = {
    {
      role = "user",
      kind = "turn",
      hidden = false,
      content = {
        { type = "text", text = "look at this" },
        { type = "image", media_type = "image/png" },
        { type = "text", text = "now fix it" },
      },
    },
  }
  eq(h.task_from_messages(msgs), "look at this\nnow fix it")
end)

case("task_is_absent_without_a_user_turn", function()
  eq(h.task_from_messages({}), nil)
  eq(h.task_from_messages(nil), nil)
  eq(h.task_from_messages({ { role = "user", kind = "turn", hidden = true, content = {} } }), nil)
end)

case("turn_start_marks_working_and_stamps_the_task", function()
  local s = state()
  eq(h.apply(s, { event = "TurnStart", data = { session_id = SESSION, text = TASK } }), "status")
  eq(s.status, "working")
  eq(s.task, TASK)
  eq(h.apply(s, { event = "TurnStart", data = { session_id = SESSION, text = TASK } }), nil)
end)

case("turn_start_clears_a_stale_error", function()
  local s = state()
  h.apply(s, { event = "TurnError", data = { session_id = SESSION, message = ERROR_MSG } })
  eq(h.apply(s, { event = "TurnStart", data = { session_id = SESSION, text = TASK } }), "status")
  eq(s.error, nil)
end)

case("tool_start_is_the_headless_working_signal", function()
  local s = state()
  eq(h.apply(s, { event = "ToolStart", data = { session_id = SESSION, tool = "bash" } }), "status")
  eq(s.status, "working")
  eq(h.apply(s, { event = "ToolStart", data = { session_id = SESSION, tool = "read" } }), nil)
end)

case("turn_error_sets_the_message_and_ends_the_turn", function()
  local s = state()
  eq(h.apply(s, { event = "TurnError", data = { session_id = SESSION, message = ERROR_MSG } }), "status")
  eq(s.status, "idle")
  eq(s.error, ERROR_MSG)
end)

case("turn_end_settles_status_context_and_cost", function()
  local s = state()
  h.apply(s, { event = "ToolStart", data = { session_id = SESSION } })
  local ev = {
    event = "TurnEnd",
    data = { session_id = SESSION, cost = 1.25, context_size = 3000, context_window = 200000 },
  }
  eq(h.apply(s, ev), "status")
  eq(s.status, "idle")
  eq(s.cost, 1.25)
  eq(s.context_size, 3000)
  eq(s.context_window, 200000)
  eq(h.apply(s, ev), nil)
end)

case("compaction_done_keeps_the_context_reading_current", function()
  local s = state()
  eq(
    h.apply(
      s,
      { event = "CompactionDone", data = { session_id = SESSION, context_size_after = 40, context_window = 100 } }
    ),
    "status"
  )
  eq(s.context_size, 40)
  eq(s.context_window, 100)
end)

case("model_changed_follows_the_model_spec", function()
  local s = state()
  eq(h.apply(s, { event = "ModelChanged", data = { session_id = SESSION, model = { spec = MODEL } } }), "status")
  eq(s.model, MODEL)
end)

case("session_status_changed_is_authoritative", function()
  local s = state()
  eq(
    h.apply(
      s,
      { event = "SessionStatusChanged", data = { session_id = SESSION, status = "needs_input", focused = true } }
    ),
    "status"
  )
  eq(s.status, "needs_input")
  eq(s.focused, true)
end)

case("session_end_says_goodbye_but_not_on_reload", function()
  local s = state()
  eq(h.apply(s, { event = "SessionEnd", data = { session_id = SESSION, reason = "shutdown" } }), "end")
  eq(h.apply(state(), { event = "SessionEnd", data = { session_id = SESSION, reason = "reload" } }), nil)
end)

case("set_focused_only_reports_a_move", function()
  local s = state({ focused = true })
  eq(h.set_focused(s, true), false)
  eq(h.set_focused(s, false), true)
  eq(s.focused, false)
end)

case("line_is_the_full_snapshot_with_absent_unknowns", function()
  local s =
    state({ status = "working", focused = true, model = MODEL, context_size = 12, context_window = 200, cost = 0.5 })
  h.apply(s, { event = "TurnStart", data = { session_id = SESSION, text = TASK } })
  local line = h.line(s, TS)
  eq(line.event, "status")
  eq(line.ts, TS)
  eq(line.session_id, SESSION)
  eq(line.focused, true)
  eq(line.status, "working")
  eq(line.model, MODEL)
  eq(line.context_size, 12)
  eq(line.context_window, 200)
  eq(line.cost, 0.5)
  eq(line.task, TASK)

  local encoded = assert(maki.json.encode(line))
  eq(encoded:find("error", 1, true), nil, "a line without an error carries no error key")
end)

case("end_line_names_only_the_session", function()
  local line = h.end_line(SESSION, TS)
  eq(line.event, "end")
  eq(line.ts, TS)
  eq(line.session_id, SESSION)
  local fields = 0
  for _ in pairs(line) do
    fields = fields + 1
  end
  eq(fields, 3, "an end line carries nothing else")
end)

th.report()
