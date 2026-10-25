local M = {}

-- One row per prompt, newest first. The row keeps the whole prompt for the
-- recall; only the label, which the picker draws on a single line, is folded.
function M.rows(entries)
  local rows = {}
  for i = #entries, 1, -1 do
    rows[#rows + 1] = {
      label = (entries[i]:gsub("%s+", " "):gsub("^%s", ""):gsub("%s$", "")),
      text = entries[i],
    }
  end
  return rows
end

-- Rows ranked by `maki.text.fuzzy_list`, which also reports the byte ranges
-- that matched so the picker can mark them.
function M.filter(rows, query)
  local labels = {}
  for i, row in ipairs(rows) do
    labels[i] = row.label
  end
  local matches = maki.text.fuzzy_list(query, labels, { highlights = true })
  local filtered, indices = {}, {}
  for i, match in ipairs(matches) do
    local row = rows[match.index]
    filtered[i] = { label = row.label, text = row.text, highlights = match.highlights }
    indices[i] = match.index
  end
  return filtered, indices
end

return M
