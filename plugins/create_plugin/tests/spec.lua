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
local REQUIRE_MARKER = 'require("'
local PERMISSIONS_MARKER = "[permissions]"
local RELOAD_MARKER = "/reload"

local function maki_dir(tmp)
  return maki.fs.joinpath(tmp, ".maki")
end

case("rejects_a_name_that_is_not_lower_snake_case", function()
  local result = helpers.handler({ name = "Not-a-plugin", path = "/tmp" })
  eq(result.is_error, true)
  eq(result.llm_output, BAD_NAME)
end)

case("refuses_a_plugin_that_is_already_there", function()
  local tmp = th.mktmpdir("create_plugin_spec")
  local module = maki.fs.joinpath(maki_dir(tmp), "lua", "dupe.lua")
  maki.fs.mkdir(maki.fs.joinpath(maki_dir(tmp), "lua"), { parents = true })
  maki.fs.write(module, "")
  local result = helpers.handler({ name = "dupe", path = maki_dir(tmp) })
  eq(result.is_error, true)
  eq(result.llm_output, EXISTS .. module)
  th.rmtree(tmp)
end)

case("scaffolds_the_module_with_the_given_description", function()
  local tmp = th.mktmpdir("create_plugin_spec")
  local result = helpers.handler({ name = "greet", path = maki_dir(tmp), description = 'Say "hello".' })
  eq(result.is_error, nil)
  local source = maki.fs.read(maki.fs.joinpath(maki_dir(tmp), "lua", "greet.lua"))
  has(source, 'name = "greet"')
  has(source, 'local DESCRIPTION = "Say \\"hello\\"."')
  th.rmtree(tmp)
end)

case("scaffolds_a_placeholder_description_when_none_is_given", function()
  local tmp = th.mktmpdir("create_plugin_spec")
  helpers.handler({ name = "greet", path = maki_dir(tmp) })
  local source = maki.fs.read(maki.fs.joinpath(maki_dir(tmp), "lua", "greet.lua"))
  has(source, PLACEHOLDER)
  th.rmtree(tmp)
end)

case("creates_the_permission_manifest_when_missing", function()
  local tmp = th.mktmpdir("create_plugin_spec")
  helpers.handler({ name = "greet", path = maki_dir(tmp) })
  eq(maki.fs.read(maki.fs.joinpath(maki_dir(tmp), "plugin.toml")), MANIFEST)
  th.rmtree(tmp)
end)

case("keeps_an_existing_permission_manifest", function()
  local tmp = th.mktmpdir("create_plugin_spec")
  maki.fs.mkdir(maki_dir(tmp), { parents = true })
  maki.fs.write(maki.fs.joinpath(maki_dir(tmp), "plugin.toml"), "[permissions]\nwrite = true\n")
  helpers.handler({ name = "greet", path = maki_dir(tmp) })
  has(maki.fs.read(maki.fs.joinpath(maki_dir(tmp), "plugin.toml")), "write = true")
  th.rmtree(tmp)
end)

case("lists_the_wiring_steps", function()
  local tmp = th.mktmpdir("create_plugin_spec")
  local result = helpers.handler({ name = "wired", path = maki_dir(tmp) })
  has(result.llm_output, REQUIRE_MARKER)
  has(result.llm_output, PERMISSIONS_MARKER)
  has(result.llm_output, RELOAD_MARKER)
  th.rmtree(tmp)
end)

th.report()
