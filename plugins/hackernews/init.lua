local ToolView = require("maki.tool_view")

local BASE_URL = "https://hn.algolia.com/api/v1/search"
local RETRIEVE  = "title,url,author,points,objectID,created_at,num_comments"

local function urlencode(s)
  return (s:gsub("[^%w%-%.%_%~]", function(c)
    return string.format("%%%02X", c:byte())
  end))
end

local function date_short(iso)
  return iso and iso:sub(1, 10) or ""
end

local function make_table(hits)
  local rows = {
    "| Title | Author | Pts | Cmts | Date |",
    "|-------|--------|----:|-----:|------|",
  }
  for _, h in ipairs(hits) do
    local url   = h.url or ("https://news.ycombinator.com/item?id=" .. (h.objectID or ""))
    local title = (h.title or ""):gsub("|", "\\|")
    rows[#rows + 1] = string.format("| [%s](%s) | %s | %d | %d | %s |",
      title, url, h.author or "", h.points or 0, h.num_comments or 0, date_short(h.created_at))
  end
  return table.concat(rows, "\n")
end

local function view_opts(ctx)
  local tol = ctx:tool_output_lines()
  return { max_lines = (tol and tol.web) or 3, keep = "head" }
end

maki.api.register_tool({
  name = "hackernews",
  kind = "fetch",
  description = "Search or browse HackerNews. Returns a compact result table — never dumps raw JSON.",

  schema = {
    type = "object",
    properties = {
      query      = { type = "string",  description = "Search terms (omit for front_page/trending)" },
      count      = { type = "integer", description = "Number of results, default 10, max 30" },
      filter     = { type = "string",  description = "Tag filter: story (default), show_hn, ask_hn, front_page, launch_hn" },
      min_points = { type = "integer", description = "Minimum points threshold" },
    },
  },
  permission_scope = "query",

  header = function(input)
    if input.query and input.query ~= "" then
      return "hackernews: " .. input.query
    end
    return "hackernews: " .. (input.filter or "front_page")
  end,

  handler = function(input, ctx)
    local query  = input.query or ""
    local count  = math.min(input.count or 10, 30)
    local filter = input.filter or (query == "" and "front_page" or "story")

    local url = BASE_URL
      .. "?query="        .. urlencode(query)
      .. "&tags="         .. filter
      .. "&hitsPerPage="  .. tostring(count)
      .. "&attributesToRetrieve=" .. RETRIEVE
      .. "&attributesToHighlight=title"
      .. "&attributesToSnippet=title"

    if input.min_points then
      url = url .. "&numericFilters=points%3E" .. tostring(input.min_points)
    end

    local resp, err = maki.net.request(url, {
      timeout = 15,
      headers = { ["Accept"] = "application/json,*/*;q=0.5" },
    })
    if not resp then
      return "error: " .. tostring(err)
    end
    if resp.status < 200 or resp.status >= 300 then
      return "error: HTTP " .. tostring(resp.status)
    end

    local data, parse_err = maki.json.decode(resp.body)
    if not data then
      return "error: could not parse response: " .. tostring(parse_err)
    end

    local hits = data.hits or {}
    if #hits == 0 then
      return query ~= "" and ("No results for: " .. query) or "No results."
    end

    local total  = data.nbHits or #hits
    local header = string.format("**%d** results (showing %d)\n\n", total, #hits)
    local out    = header .. make_table(hits)

    return {
      llm_output = out,
      body = ToolView.restore(out, view_opts(ctx)),
    }
  end,
})
