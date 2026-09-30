local backoff = require("maki.utf8").backoff

local M = {}

function M.truncate_bytes(line, max_bytes)
  if #line <= max_bytes then
    return line
  end
  return line:sub(1, backoff(line, max_bytes)) .. "..."
end

function M.split_lines(content)
  local lines = {}
  local pos = 1
  while pos <= #content do
    local nl = content:find("\n", pos, true)
    local raw = nl and content:sub(pos, nl - 1) or content:sub(pos)
    lines[#lines + 1] = raw:find("\r$") and raw:sub(1, -2) or raw
    pos = nl and nl + 1 or #content + 1
  end
  return lines
end

return M
