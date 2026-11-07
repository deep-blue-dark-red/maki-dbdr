local M = {}

local NO_HITS = "no lines match"
local NOTHING = "no drafts or clips yet"

function M.format_hits(hits)
  if #hits == 0 then
    return NO_HITS
  end
  local lines = {}
  for i, hit in ipairs(hits) do
    lines[i] = string.format("%s:%d: %s", hit.place, hit.line, hit.text)
  end
  return table.concat(lines, "\n")
end

local function plural(n, word)
  return string.format("%d %s%s", n, word, n == 1 and "" or "s")
end

function M.format_list(listing)
  local lines = {}
  for _, draft in ipairs(listing.drafts) do
    lines[#lines + 1] = string.format("draft:%s  %s, v%d", draft.path, plural(draft.lines, "line"), draft.versions)
  end
  for _, clip in ipairs(listing.clips) do
    lines[#lines + 1] = string.format("@%s  %s", clip.name, plural(clip.lines, "line"))
  end
  if #lines == 0 then
    return NOTHING
  end
  return table.concat(lines, "\n")
end

return M
