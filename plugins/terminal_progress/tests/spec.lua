local p = require("terminal_progress")
local th = require("maki.test_helpers")

local case = th.case
local eq = th.eq

local function ev(event, data)
  return { event = event, data = data or {} }
end

case("turn_events_map_to_the_tab_state", function()
  eq(p.session_state(ev("TurnStart")), "indeterminate")
  eq(p.session_state(ev("ToolStart")), "indeterminate")
  eq(p.session_state(ev("TurnEnd")), "off")
  eq(p.session_state(ev("TurnError")), "error")
end)

case("needing_input_pauses_the_bar", function()
  eq(p.session_state(ev("SessionStatusChanged", { status = "needs_input" })), "paused")
  eq(p.session_state(ev("SessionStatusChanged", { status = "working" })), "indeterminate")
  eq(p.session_state(ev("SessionStatusChanged", { status = "idle" })), "off")
end)

case("reload_is_not_a_goodbye", function()
  eq(p.session_state(ev("SessionEnd", { reason = "reload" })), nil)
  eq(p.session_state(ev("SessionEnd", { reason = "exit" })), "off")
end)

case("unrelated_events_say_nothing", function()
  eq(p.session_state(ev("ModelChanged")), nil)
end)

case("title_wears_the_claude_prefix_for_switchers", function()
  eq(p.title("indeterminate", "/Users/a/git/x", "/Users/a"), "\u{2802} maki - ~/git/x")
  eq(p.title("off", "/Users/a/git/x", "/Users/a"), "\u{2733} maki - ~/git/x")
  eq(p.title("paused", "/Users/a", "/Users/a"), "\u{2733} maki - ~")
  eq(p.title("error", "/Users/ab/x", "/Users/a"), "\u{2733} maki - /Users/ab/x")
  eq(p.title("off", "/srv/x", nil), "\u{2733} maki - /srv/x")
end)

case("the_most_urgent_session_wins", function()
  eq(p.combine({}), "off")
  eq(p.combine({ a = "indeterminate" }), "indeterminate")
  eq(p.combine({ a = "indeterminate", b = "paused" }), "paused")
  eq(p.combine({ a = "paused", b = "error", c = "indeterminate" }), "error")
end)
