local helpers = require("create_plugin_helpers")
local shorten_path = require("maki.shorten_path")

local DESCRIPTION = [[Scaffold a new bundled plugin in the maki source tree, for developing maki itself.

- Creates <path>/<name>/init.lua and plugin.toml, then lists the wiring steps that make the plugin load.
- Never use it for a personal plugin: those live in ~/.config/maki/lua/<name>.lua and load on /reload, while a file under plugins/ does nothing until maki is rebuilt.]]

maki.api.register_tool({
  name = "create_plugin",
  kind = "edit",
  description = DESCRIPTION,
  mutable_path = "path",
  permission = "fs_write",
  permission_scopes = function(input)
    return { scopes = { helpers.plugins_dir(input.path) } }
  end,
  audiences = { "main", "general_sub" },

  schema = {
    type = "object",
    properties = {
      name = {
        type = "string",
        description = "New plugin name: lowercase letters, digits and underscores, starting with a letter",
        required = true,
      },
      path = {
        type = "string",
        description = "Absolute path to the plugins directory to create it in, the plugins/ of a maki checkout",
        required = true,
      },
      description = {
        type = "string",
        description = "What the scaffolded tool should do, in one sentence",
      },
    },
  },

  header = function(input)
    return shorten_path(helpers.plugins_dir(input.path))
  end,

  handler = function(input)
    return helpers.handler(input)
  end,
})
