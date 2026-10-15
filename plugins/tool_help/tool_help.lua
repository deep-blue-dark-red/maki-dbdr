local M = {}

M.NOT_BUILT_IN =
  "'%s' is %s: tool_help covers built-in tools only. Its definition in your tool list is complete; load a deferred MCP tool with tool_search."
local SOURCE_LABELS = { mcp = "an MCP tool", ["local"] = "a host tool" }

local function sorted_keys(t)
  local keys = {}
  for k in pairs(t) do
    keys[#keys + 1] = k
  end
  table.sort(keys)
  return keys
end

function M.list_tools(callable)
  local names = {}
  for _, t in ipairs(callable) do
    names[#names + 1] = t.name
  end
  table.sort(names)
  return "Tools: " .. table.concat(names, ", ")
end

local function arg_type(p)
  if p.type == "array" and p.items and p.items.type then
    return "array of " .. p.items.type
  end
  return p.type or "any"
end

local function format_argument(name, p, required)
  local line = "- " .. name .. " (" .. arg_type(p)
  if required[name] then
    line = line .. ", required"
  end
  line = line .. ")"
  if p.description then
    line = line .. ": " .. p.description
  end
  if p.enum then
    line = line .. " One of: " .. table.concat(p.enum, ", ")
  end
  return line
end

function M.format_arguments(schema)
  local props = schema and schema.properties
  if not props or not next(props) then
    return nil
  end
  local required = {}
  for _, name in ipairs(schema.required or {}) do
    required[name] = true
  end
  local lines = {}
  for _, name in ipairs(sorted_keys(props)) do
    lines[#lines + 1] = format_argument(name, props[name], required)
  end
  return table.concat(lines, "\n")
end

function M.format_help(entry)
  local parts = { entry.name }
  if entry.audiences and #entry.audiences > 0 then
    parts[#parts + 1] = "Audiences: " .. table.concat(entry.audiences, ", ")
  end
  if entry.description then
    parts[#parts + 1] = entry.description
  end
  local args = M.format_arguments(entry.schema)
  if args then
    parts[#parts + 1] = "Arguments:\n" .. args
  end
  return table.concat(parts, "\n\n")
end

function M.unknown(name, callable)
  for _, t in ipairs(callable) do
    if t.name == name then
      return string.format(M.NOT_BUILT_IN, name, SOURCE_LABELS[t.source] or t.source)
    end
  end
  return {
    llm_output = "error: no tool named '" .. name .. "'. " .. M.list_tools(callable),
    is_error = true,
  }
end

function M.handler(input, ctx)
  if not input.name or input.name == "" then
    local callable, err = maki.agent.callable_tools(ctx)
    if err then
      return { llm_output = "error: " .. err, is_error = true }
    end
    return M.list_tools(callable)
  end
  local entry = maki.api.get_tool(input.name)
  if entry then
    return M.format_help(entry)
  end
  local callable, err = maki.agent.callable_tools(ctx)
  if err then
    return { llm_output = "error: " .. err, is_error = true }
  end
  return M.unknown(input.name, callable)
end

return M
