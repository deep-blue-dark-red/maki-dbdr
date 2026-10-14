local help = require("tool_help")
local th = require("maki.test_helpers")

local case = th.case
local eq = th.eq
local has = th.has

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

case("handler_unknown_name_errors", function()
  local result = help.handler({ name = "no_such_tool" })
  eq(result.is_error, true)
  has(result.llm_output, "error: no tool named 'no_such_tool'")
end)
