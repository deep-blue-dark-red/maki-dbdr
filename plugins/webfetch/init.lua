local VALID_FORMATS = { markdown = true, text = true, html = true, json = true }
local DEFAULT_FORMAT = "markdown"
local SKIP_TAGS = { script = true, style = true, noscript = true }
local ACCEPT_HEADERS = {
  html = "text/html,*/*;q=0.5",
  text = "text/plain,text/html;q=0.9,*/*;q=0.5",
  markdown = "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.5",
  json = "application/json,*/*;q=0.5",
}

local LT, GT, SLASH, SPACE, TAB, CR, LF = 60, 62, 47, 32, 9, 13, 10
local function is_ws(b)
  return b == SPACE or b == TAB or b == CR or b == LF
end

local function strip_html(html)
  local out = {}
  local in_tag = false
  local tag_start = 0
  local skip_tag = nil
  local last_was_space = true

  for i = 1, #html do
    local b = html:byte(i)
    if b == LT then
      in_tag = true
      tag_start = i + 1
    elseif b == GT then
      in_tag = false
      local tag_str = html:sub(tag_start, i - 1):lower()
      local tag_name = tag_str:match("^%s*(%S+)")

      if skip_tag then
        if tag_name and tag_name:byte(1) == SLASH and tag_name:sub(2) == skip_tag then
          skip_tag = nil
        end
      elseif tag_name and SKIP_TAGS[tag_name] then
        skip_tag = tag_name
      end

      if not skip_tag and #out > 0 and not last_was_space then
        out[#out + 1] = " "
        last_was_space = true
      end
    elseif in_tag then
      -- accumulate nothing; we use sub(tag_start, i-1) on close
    elseif not skip_tag then
      if is_ws(b) then
        if not last_was_space and #out > 0 then
          out[#out + 1] = " "
          last_was_space = true
        end
      else
        out[#out + 1] = html:sub(i, i)
        last_was_space = false
      end
    end
  end

  local result = table.concat(out)
  return result:match("^%s*(.-)%s*$")
end

local function json_pretty(val, depth)
  depth = depth or 0
  local t = type(val)
  if t == "string" then
    local escaped = val:gsub("\\", "\\\\"):gsub('"', '\\"'):gsub("\n", "\\n"):gsub("\r", "\\r"):gsub("\t", "\\t")
    return '"' .. escaped .. '"'
  elseif t == "number" then
    return tostring(val)
  elseif t == "boolean" then
    return val and "true" or "false"
  elseif val == nil then
    return "null"
  elseif t == "table" then
    local pad = string.rep("  ", depth)
    local inner = string.rep("  ", depth + 1)
    local n = #val
    local is_array = false
    if n > 0 then
      local count = 0
      for _ in pairs(val) do
        count = count + 1
      end
      is_array = (count == n)
    end
    if is_array then
      local parts = {}
      for i = 1, n do
        parts[i] = inner .. json_pretty(val[i], depth + 1)
      end
      return "[\n" .. table.concat(parts, ",\n") .. "\n" .. pad .. "]"
    else
      local parts = {}
      local keys = {}
      for k in pairs(val) do
        keys[#keys + 1] = k
      end
      table.sort(keys, function(a, b)
        return tostring(a) < tostring(b)
      end)
      for _, k in ipairs(keys) do
        local v = val[k]
        if v ~= nil then
          parts[#parts + 1] = inner .. '"' .. tostring(k) .. '": ' .. json_pretty(v, depth + 1)
        end
      end
      if #parts == 0 then
        return "{}"
      end
      return "{\n" .. table.concat(parts, ",\n") .. "\n" .. pad .. "}"
    end
  end
  return tostring(val)
end

local truncate = require("maki.truncate")
local ToolView = require("maki.tool_view")
local output_limits = require("maki.output_limits")

local opts = maki.api.register_options(output_limits.extend({
  max_response_bytes = {
    default = 5 * 1024 * 1024,
    min = 1024,
    desc = "Stop reading a response after this many bytes.",
  },
}))

local function web_view_opts(ctx)
  local tol = ctx:tool_output_lines()
  return { max_lines = (tol and tol.web) or 3, keep = "head" }
end

maki.api.register_tool({
  name = "webfetch",
  kind = "fetch",
  description = [[Fetch a URL and return its contents.

- Supports markdown (default), text, html, or json output formats.
- HTTP URLs are auto-upgraded to HTTPS.
- Max response size is 5MB, max timeout is 120s.
- Best used inside code_execution with some truncation / filter to avoid context bloat.]],

  schema = {
    type = "object",
    properties = {
      url = { type = "string", description = "URL to fetch (http:// or https://)", required = true },
      format = { type = "string", description = "Output format: markdown (default), text, html, or json" },
      timeout = { type = "integer", description = "Timeout in seconds (default 30, max 120)" },
    },
  },
  permission = "net",
  permission_scopes = "url",

  header = function(input)
    local fmt = input.format
    if fmt and fmt ~= DEFAULT_FORMAT then
      return input.url .. " [" .. fmt .. "]"
    end
    return input.url
  end,

  restore = function(_input, output, _is_error, ctx)
    return ToolView.restore(output, web_view_opts(ctx))
  end,

  handler = function(input, ctx)
    local url = input.url
    if not url then
      return { llm_output = "error: url is required", is_error = true }
    end

    local fmt = input.format or DEFAULT_FORMAT
    if not VALID_FORMATS[fmt] then
      return { llm_output = "error: unknown format: " .. tostring(fmt), is_error = true }
    end

    local max_lines, max_bytes = output_limits.resolve(opts, ctx)

    local resp, err = maki.net.request(url, {
      timeout = input.timeout or 30,
      max_bytes = opts.max_response_bytes,
      headers = {
        ["Accept"] = ACCEPT_HEADERS[fmt],
      },
    })
    if not resp then
      return { llm_output = "error: " .. tostring(err), is_error = true }
    end

    if resp.status < 200 or resp.status >= 300 then
      return { llm_output = "error: HTTP " .. tostring(resp.status), is_error = true }
    end

    local ct = resp.content_type or ""
    if ct:find("^image/") and not ct:find("svg") then
      return { llm_output = "error: image content cannot be displayed as text", is_error = true }
    end

    local body = resp.body
    local is_html = ct:find("text/html") ~= nil
    local is_json = ct:find("application/json") ~= nil or fmt == "json"

    if fmt == "markdown" and is_html then
      local converted = maki.text.html_to_markdown(body)
      body = converted or body
    elseif fmt == "text" and is_html then
      body = strip_html(body)
    end

    local llm_output = truncate(body, max_lines, max_bytes)

    local display_body = body
    if is_json then
      local parsed, _ = maki.json.decode(body)
      if parsed then
        display_body = json_pretty(parsed)
      end
    end

    return {
      llm_output = llm_output,
      body = ToolView.restore(display_body, web_view_opts(ctx)),
    }
  end,
})
