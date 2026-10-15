local help = require("tool_help")

local DESCRIPTION =
  [[Show a built-in tool's full description and argument schema. Without a name, list the callable tools. An unknown name returns the callable tools.]]

maki.api.register_tool({
  name = "tool_help",
  kind = "read",
  description = DESCRIPTION,
  schema = {
    type = "object",
    properties = {
      name = { type = "string", description = "Tool name to look up; omit to list callable tools" },
    },
  },
  header = function(input)
    return "tool_help " .. (input.name or "")
  end,
  handler = function(input, ctx)
    return help.handler(input, ctx)
  end,
})
