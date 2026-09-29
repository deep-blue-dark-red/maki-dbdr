-- Pure logic behind the /aa_scores cache: what the leaderboard page carries
-- and how long a cache file stays usable.

local M = {}

M.REFRESH_SECS = 24 * 60 * 60

-- A leaderboard row arrives as escaped JSON inside the page, with the index
-- and its estimated flag at the end of one flat object. `[^{}]-` stops a row
-- without an index (or a row from the page's own navigation lists) at its
-- closing brace instead of borrowing the next row's score.
local ROW =
  [[{\"slug\":\"([%w%._%-]+)\"[^{}]-\"intelligenceIndex\":([%-]?[%d%.]+),\"intelligenceIndexIsEstimated\":(%a+)]]

--- Scores from {html}, keyed by slug, plus the slugs Artificial Analysis
--- estimated rather than measured. Returns scores, estimated, count.
function M.parse(html)
  local scores, estimated, count = {}, {}, 0
  for slug, value, est in html:gmatch(ROW) do
    scores[slug] = tonumber(value)
    count = count + 1
    if est == "true" then
      estimated[#estimated + 1] = slug
    end
  end
  return scores, estimated, count
end

--- A cache written at {mtime} still answers for a day; anything older (or
--- missing) is refetched.
function M.is_stale(mtime, now)
  return mtime == nil or now - mtime >= M.REFRESH_SECS
end

return M
