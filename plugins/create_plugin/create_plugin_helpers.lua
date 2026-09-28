local M = {}

local NAME_PATTERN = "^%l[%l%d_]*$"
local MANIFEST = "[permissions]\n"
local DEFAULT_DESCRIPTION = "TODO: one sentence on what this tool does and when the model should reach for it."

local ERR_NAME =
  "error: name must start with a lowercase letter and keep only lowercase letters, digits and underscores"
local ERR_EXISTS = "error: plugin already exists: "
local ERR_MKDIR = "error: cannot create "
local ERR_WRITE = "error: cannot write "

-- The skeleton loads and answers, so the wiring steps are all that stand
-- between a scaffold and a working tool. %q keeps whatever the caller said
-- about the tool inside one Lua literal.
local INIT_SOURCE = [[
local DESCRIPTION = %q

maki.api.register_tool({
  name = "%s",
  kind = "read",
  description = DESCRIPTION,

  schema = {
    type = "object",
    properties = {
      query = {
        type = "string",
        description = "TODO: what the model has to pass here",
        required = true,
      },
    },
  },

  header = function(input)
    return input.query or "%s"
  end,

  handler = function(input)
    -- TODO: the work this tool exists to do.
    return { llm_output = input.query }
  end,
})
]]

local function wiring(name, dir)
  return string.format(
    [[created %s/init.lua and %s/plugin.toml

plugins/ is compiled into maki, so wire the new plugin in before it loads:
1. maki-lua/src/loader.rs - a BundledPlugin entry in BUNDLED_PLUGINS, above `memory` when its tool
   writes files:
   BundledPlugin { name: "%s", dir: include_dir!("$CARGO_MANIFEST_DIR/../plugins/%s") },
2. maki-config/src/lib.rs - "%s" into DEFAULT_BUILTINS (alphabetical) to load it by default, or
   OPTIONAL_BUILTINS to ship it off; a tool declaring permission = "fs_write" joins FILE_WRITE_TOOLS
   and memory's WRITE_TOOLS.
3. maki-docgen/src/gen_tools.rs - every tool it registers goes into SECTIONS, then `just gen-docs`.
4. `just lint && just test`.

a personal plugin does not belong here: ~/.config/maki/lua/%s.lua, required from init.lua, loads on
/reload without a rebuild.]],
    dir,
    dir,
    name,
    name,
    name,
    name
  )
end

function M.plugins_dir(path)
  return maki.fs.abspath(path)
end

function M.handler(input)
  local name = input.name
  if type(name) ~= "string" or not name:match(NAME_PATTERN) then
    return { llm_output = ERR_NAME, is_error = true }
  end

  local dir = maki.fs.joinpath(M.plugins_dir(input.path), name)
  if maki.fs.metadata(dir) then
    return { llm_output = ERR_EXISTS .. dir, is_error = true }
  end

  local made, mkdir_err = maki.fs.mkdir(dir, { parents = true })
  if not made then
    return { llm_output = ERR_MKDIR .. dir .. ": " .. tostring(mkdir_err), is_error = true }
  end

  local description = input.description or DEFAULT_DESCRIPTION
  local files = {
    { path = maki.fs.joinpath(dir, "init.lua"), content = string.format(INIT_SOURCE, description, name, name) },
    { path = maki.fs.joinpath(dir, "plugin.toml"), content = MANIFEST },
  }
  for _, file in ipairs(files) do
    local written, write_err = maki.fs.write(file.path, file.content)
    if not written then
      return { llm_output = ERR_WRITE .. file.path .. ": " .. tostring(write_err), is_error = true }
    end
  end

  return { llm_output = wiring(name, dir) }
end

return M
