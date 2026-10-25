local M = {}

function M.parse_frontmatter(content)
  local rest = content:match("^%s*%-%-%-\n(.*)")
  if not rest then
    return {}, content
  end
  local end_pos = rest:find("\n%-%-%-")
  if not end_pos then
    return {}, content
  end
  local yaml_str = rest:sub(1, end_pos)
  local body = rest:sub(end_pos + 4):match("^%s*(.-)%s*$")
  local fm, _ = maki.yaml.decode(yaml_str)
  if not fm then
    fm = {}
  end
  return fm, body
end

function M.build_skill_list(skills, roots)
  local sorted = {}
  for _, s in pairs(skills) do
    sorted[#sorted + 1] = s
  end
  table.sort(sorted, function(a, b)
    return a.name < b.name
  end)

  local parts = {}
  if roots and #roots > 0 then
    local dirs = { "Skill directories:" }
    for _, r in ipairs(roots) do
      dirs[#dirs + 1] = "- " .. r.label .. ": " .. r.path
    end
    parts[#parts + 1] = table.concat(dirs, "\n")
  end

  if #sorted == 0 then
    parts[#parts + 1] = "No skills available."
    return table.concat(parts, "\n\n")
  end

  for _, s in ipairs(sorted) do
    parts[#parts + 1] = "**" .. s.name .. "**: " .. s.description
  end
  return table.concat(parts, "\n\n")
end

return M
