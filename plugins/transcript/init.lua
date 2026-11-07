local format = require("format")

local DESCRIPTION =
  [[Your own earlier output: search it, preview what a ⟦first…last⟧ quote pastes, or list drafts and clips. search: regex over everything you wrote, newest first, each hit tagged with where it is. copy: resolve a quote and show the text; with name, save it so ⟦@name⟧ pastes it later. list: drafts (draft:<path>) and clips (@name).]]

local DEFAULT_LIMIT = 20

maki.api.register_tool({
  name = "transcript",
  kind = "read",
  audiences = { "main", "general_sub" },
  description = DESCRIPTION,

  schema = {
    type = "object",
    properties = {
      command = {
        type = "string",
        enum = { "search", "copy", "list" },
        description = "search: pattern (regex), optional limit. copy: quote, optional name. list: no arguments",
        required = true,
      },
      pattern = { type = "string", description = "Regex matched against each line you wrote" },
      limit = { type = "integer", description = "Most hits to return (default 20)" },
      quote = {
        type = "string",
        description = "A ⟦first…last⟧ or ⟦@name⟧ quote, literal text allowed around it",
      },
      name = { type = "string", description = "Save the copied text as a clip pasted with ⟦@name⟧" },
    },
  },

  header = function(input)
    local arg = input.pattern or input.quote or ""
    return "transcript " .. (input.command or "") .. " " .. arg
  end,

  handler = function(input, ctx)
    local cmd = input.command
    local result, err
    if cmd == "search" then
      if not input.pattern then
        return { llm_output = "error: pattern is required", is_error = true }
      end
      local hits
      hits, err = ctx:transcript_search(input.pattern, input.limit or DEFAULT_LIMIT)
      result = hits and format.format_hits(hits)
    elseif cmd == "copy" then
      if not input.quote then
        return { llm_output = "error: quote is required", is_error = true }
      end
      result, err = ctx:transcript_copy(input.quote, input.name)
    elseif cmd == "list" then
      local listing
      listing, err = ctx:transcript_list()
      result = listing and format.format_list(listing)
    else
      err = "unknown command: " .. tostring(cmd)
    end
    if err then
      return { llm_output = "error: " .. err, is_error = true }
    end
    return result
  end,
})
