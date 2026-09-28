local helpers = require("create_plugin_helpers")
local th = require("maki.test_helpers")

local case = th.case
local eq = th.eq
local has = th.has

local BAD_NAME =
  "error: name must start with a lowercase letter and keep only lowercase letters, digits and underscores"
local EXISTS = "error: plugin already exists: "
local MANIFEST = "[permissions]\n"
local PLACEHOLDER = "TODO: one sentence on what this tool does and when the model should reach for it."
local WIRED_MARKER = "BUNDLED_PLUGINS"
local PLAN_MARKER = "DEFAULT_BUILTINS"
local SECTIONS_MARKER = "SECTIONS"

case("rejects_a_name_that_is_not_lower_snake_case", function()
  local result = helpers.handler({ name = "Not-a-plugin", path = "/tmp" })
  eq(result.is_error, true)
  eq(result.llm_output, BAD_NAME)
end)

case("refuses_a_plugin_that_is_already_there", function()
  local tmp = th.mktmpdir("create_plugin_spec")
  local dir = maki.fs.joinpath(tmp, "dupe")
  maki.fs.mkdir(dir)
  local result = helpers.handler({ name = "dupe", path = tmp })
  eq(result.is_error, true)
  eq(result.llm_output, EXISTS .. dir)
  th.rmtree(tmp)
end)

case("scaffolds_init_lua_with_the_given_description", function()
  local tmp = th.mktmpdir("create_plugin_spec")
  local result = helpers.handler({ name = "greet", path = tmp, description = 'Say "hello".' })
  eq(result.is_error, nil)
  local source = maki.fs.read(maki.fs.joinpath(tmp, "greet/init.lua"))
  has(source, 'name = "greet"')
  has(source, 'local DESCRIPTION = "Say \\"hello\\"."')
  th.rmtree(tmp)
end)

case("scaffolds_a_placeholder_description_when_none_is_given", function()
  local tmp = th.mktmpdir("create_plugin_spec")
  helpers.handler({ name = "greet", path = tmp })
  local source = maki.fs.read(maki.fs.joinpath(tmp, "greet/init.lua"))
  has(source, PLACEHOLDER)
  th.rmtree(tmp)
end)

case("scaffolds_the_permission_manifest", function()
  local tmp = th.mktmpdir("create_plugin_spec")
  helpers.handler({ name = "greet", path = tmp })
  eq(maki.fs.read(maki.fs.joinpath(tmp, "greet/plugin.toml")), MANIFEST)
  th.rmtree(tmp)
end)

case("lists_the_wiring_steps", function()
  local tmp = th.mktmpdir("create_plugin_spec")
  local result = helpers.handler({ name = "wired", path = tmp })
  has(result.llm_output, WIRED_MARKER)
  has(result.llm_output, PLAN_MARKER)
  has(result.llm_output, SECTIONS_MARKER)
  th.rmtree(tmp)
end)

th.report()
