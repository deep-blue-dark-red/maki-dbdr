local help = require("tool_help")

local DESCRIPTION = [[Show a tool's full description and argument schema.

Args:
- name: exact tool name. Unknown names return the list of available tools.]]

maki.api.register_tool({
  name = "tool_help",
  kind = "read",
  description = DESCRIPTION,
  schema = {
    type = "object",
    required = { "name" },
    properties = {
      name = { type = "string", description = "Tool name to look up" },
    },
  },
  header = function(input)
    return "tool_help " .. (input.name or "")
  end,
  handler = function(input)
    return help.handler(input)
  end,
})
