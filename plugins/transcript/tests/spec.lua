local format = require("format")
local th = require("maki.test_helpers")

local case = th.case
local eq = th.eq

case("format_hits_tags_place_and_line", function()
  local hits = {
    { place = "m2 edit()", line = 1, text = "fn arg" },
    { place = "m1 thinking code[a.rs]", line = 3, text = "fn old() {}" },
  }
  eq(format.format_hits(hits), "m2 edit():1: fn arg\nm1 thinking code[a.rs]:3: fn old() {}")
end)

case("format_hits_empty", function()
  eq(format.format_hits({}), "no lines match")
end)

case("format_list_drafts_then_clips", function()
  local listing = {
    drafts = { { path = "src/h.rs", versions = 2, lines = 1 } },
    clips = { { name = "handler", lines = 3 } },
  }
  eq(format.format_list(listing), "draft:src/h.rs  1 line, v2\n@handler  3 lines")
end)

case("format_list_empty", function()
  eq(format.format_list({ drafts = {}, clips = {} }), "no drafts or clips yet")
end)
