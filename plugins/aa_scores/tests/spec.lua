local h = require("aa_scores_helpers")
local th = require("maki.test_helpers")

local case = th.case
local eq = th.eq

-- Rows as the leaderboard ships them: escaped JSON with the index and its
-- estimated flag at the end of a flat object.
local NAV_ROW = [[{\"slug\":\"glm-5-3\",\"name\":\"GLM-5.3 (max)\"}]]
local MEASURED_ROW =
  [[{\"slug\":\"glm-5-3\",\"name\":\"GLM-5.3 (max)\",\"contextWindowTokens\":1000000,\"intelligenceIndex\":44.777392385614,\"intelligenceIndexIsEstimated\":false]]
local ESTIMATED_ROW =
  [[{\"slug\":\"glm-5\",\"name\":\"GLM-5\",\"intelligenceIndex\":27.9111550817227,\"intelligenceIndexIsEstimated\":true]]
local UNSCORED_ROW = [[{\"slug\":\"mimo-v2-5\",\"name\":\"Mimo 2.5\"}]]

case("parses_measured_and_estimated_scores", function()
  local scores, estimated, count = h.parse(NAV_ROW .. MEASURED_ROW .. ESTIMATED_ROW .. UNSCORED_ROW)
  eq(count, 2)
  eq(scores["glm-5-3"], 44.777392385614)
  eq(scores["glm-5"], 27.9111550817227)
  eq(#estimated, 1)
  eq(estimated[1], "glm-5")
  eq(scores["mimo-v2-5"], nil)
end)

case("a_navigation_row_never_borrows_a_score", function()
  local scores, _, count = h.parse(NAV_ROW)
  eq(count, 0)
  eq(scores["glm-5-3"], nil)
end)

case("a_cache_expires_after_a_day", function()
  local mtime = 1000
  eq(h.is_stale(mtime, mtime + h.REFRESH_SECS - 1), false)
  eq(h.is_stale(mtime, mtime + h.REFRESH_SECS), true)
  eq(h.is_stale(nil, mtime), true)
end)

th.report()
