local h = require("input_history_helpers")
local th = require("maki.test_helpers")

local case = th.case
local eq = th.eq

case("rows_are_newest_first_with_the_full_prompt", function()
  local rows = h.rows({ "old prompt", "new prompt" })
  eq(#rows, 2)
  eq(rows[1].text, "new prompt")
  eq(rows[2].text, "old prompt")
end)

case("multiline_prompt_folds_into_one_label", function()
  local rows = h.rows({ "  first\n\nsecond\tline  " })
  eq(rows[1].label, "first second line")
  eq(rows[1].text, "  first\n\nsecond\tline  ", "the row keeps the prompt verbatim")
end)

case("filter_matches_a_fuzzy_subsequence", function()
  local rows = h.rows({ "make a new branch", "review the parser" })
  local filtered, indices = h.filter(rows, "mkbr")
  eq(#filtered, 1)
  eq(filtered[1].text, "make a new branch")
  eq(indices[1], 2)
end)

case("filter_ranks_the_tighter_match_first", function()
  local rows = { { label = "a_b_c" }, { label = "abc" } }
  local filtered = h.filter(rows, "abc")
  eq(filtered[1].label, "abc")
  eq(filtered[2].label, "a_b_c")
end)

case("filter_empty_query_keeps_the_picker_order", function()
  local rows = h.rows({ "old", "new" })
  local filtered = h.filter(rows, "")
  eq(#filtered, 2)
  eq(filtered[1].text, "new")
  eq(filtered[2].text, "old")
end)

case("filter_reports_the_matched_byte_ranges", function()
  local rows = h.rows({ "review the parser" })
  local filtered = h.filter(rows, "parser")
  eq(#filtered[1].highlights, 1)
  eq(filtered[1].highlights[1][1], 12)
  eq(filtered[1].highlights[1][2], 17)
end)

th.report()
