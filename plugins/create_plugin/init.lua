local helpers = require("create_plugin_helpers")
local shorten_path = require("maki.shorten_path")

local DESCRIPTION =
  [[Scaffold a personal plugin in the maki config directory. Creates lua/<name>.lua (and plugin.toml if missing), then reports the remaining wiring steps; loaded by /reload, no rebuild.]]

maki.api.register_tool({
  name = "create_plugin",
  kind = "edit",
  description = DESCRIPTION,
  mutable_path = "path",
  permission = "fs_write",
  permission_scopes = function(input)
    return { scopes = { helpers.config_dir(input.path) } }
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
      description = {
        type = "string",
        description = "What the scaffolded tool should do, in one sentence",
      },
      path = {
        type = "string",
        description = "Config directory to scaffold into (a .maki directory); defaults to the global config dir (~/.maki)",
      },
    },
  },

  header = function(input)
    return shorten_path(helpers.config_dir(input.path) or "~/.maki")
  end,

  handler = function(input)
    return helpers.handler(input)
  end,
})
