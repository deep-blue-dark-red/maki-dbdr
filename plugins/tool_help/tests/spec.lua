local help = require("tool_help")
local th = require("maki.test_helpers")

local case = th.case
local eq = th.eq

local ENTRY = {
  name = "spec_tool",
  audiences = { "main", "sub" },
  description = "Spec tool for tests.",
  schema = {
    type = "object",
    required = { "path" },
    properties = {
      path = { type = "string", description = "A path" },
      mode = { type = "string", enum = { "fast", "slow" }, description = "Run mode" },
      paths = { type = "array", items = { type = "string" } },
    },
  },
}

local ARGS = "- mode (string): Run mode One of: fast, slow\n"
  .. "- path (string, required): A path\n"
  .. "- paths (array of string)"

case("format_arguments_sorts_and_annotates", function()
  eq(help.format_arguments(ENTRY.schema), ARGS)
end)

case("format_arguments_none", function()
  eq(help.format_arguments({ type = "object" }), nil)
end)

case("format_help_sections", function()
  local expected = "spec_tool\n\nAudiences: main, sub\n\nSpec tool for tests.\n\nArguments:\n" .. ARGS
  eq(help.format_help(ENTRY), expected)
end)

case("format_help_without_audiences_or_arguments", function()
  local entry = { name = "bare", description = "d", schema = { type = "object" } }
  eq(help.format_help(entry), "bare\n\nd")
end)

local CALLABLE = {
  { name = "read", source = "native" },
  { name = "rose__Query", source = "mcp" },
}

case("list_tools_sorts_callable_names", function()
  eq(help.list_tools(CALLABLE), "Tools: read, rose__Query")
end)

case("unknown_name_errors_with_callable_tools", function()
  local result = help.unknown("no_such_tool", CALLABLE)
  eq(result.is_error, true)
  eq(result.llm_output, "error: no tool named 'no_such_tool'. Tools: read, rose__Query")
end)

case("unknown_mcp_tool_points_at_its_definition", function()
  eq(help.unknown("rose__Query", CALLABLE), string.format(help.NOT_BUILT_IN, "rose__Query", "an MCP tool"))
end)
