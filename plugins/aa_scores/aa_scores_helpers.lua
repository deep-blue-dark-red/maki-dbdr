-- Pure logic behind the /aa_scores cache: what the leaderboard page carries
-- and how long a cache file stays usable.

local M = {}

M.REFRESH_SECS = 24 * 60 * 60

-- A leaderboard row arrives as escaped JSON inside the page, one object per
-- model, holding nested objects (`scores`). Rows are walked one at a time and
-- only up to the next row's slug, so a row with a null index (or a page
-- navigation row, which has no index at all) cannot borrow the next row's
-- score.
local ROW_START = [[{\"slug\":\"([%w%._%-]+)\"]]
local ROW_BOUNDARY = [[{\"slug\":]]
local INDEX = [[\"intelligenceIndex\":([%-]?[%d%.]+)]]

--- Scores from {html}, keyed by slug, plus the slugs Artificial Analysis
--- estimated rather than measured (the leaderboard dropped that flag, so the
--- list stays empty). Returns scores, estimated, count.
function M.parse(html)
  local scores, estimated, count = {}, {}, 0
  local pos = 1
  while true do
    local _, end_, slug = html:find(ROW_START, pos)
    if not end_ then
      break
    end
    local boundary = html:find(ROW_BOUNDARY, end_ + 1, true)
    local chunk = html:sub(end_ + 1, (boundary or #html + 1) - 1)
    local value = chunk:match(INDEX)
    if value then
      scores[slug] = tonumber(value)
      count = count + 1
    end
    pos = end_ + 1
  end
  return scores, estimated, count
end

--- A cache written at {mtime} still answers for a day; anything older (or
--- missing) is refetched.
function M.is_stale(mtime, now)
  return mtime == nil or now - mtime >= M.REFRESH_SECS
end

return M
