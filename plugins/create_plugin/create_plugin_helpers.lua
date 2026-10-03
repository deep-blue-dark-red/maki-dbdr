local M = {}

local NAME_PATTERN = "^%l[%l%d_]*$"
local MANIFEST = "[permissions]\n"
local DEFAULT_DESCRIPTION = "TODO: one sentence on what this tool does and when the model should reach for it."

local ERR_NAME =
  "error: name must start with a lowercase letter and keep only lowercase letters, digits and underscores"
local ERR_CONFIG_DIR = "error: cannot determine the maki config directory"
local ERR_EXISTS = "error: plugin already exists: "
local ERR_MKDIR = "error: cannot create "
local ERR_WRITE = "error: cannot write "

local MODULE_SOURCE = [[
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

local function wiring(name, dir, module, manifest_created)
  local manifest_note = manifest_created and ("created " .. dir .. "/plugin.toml\n")
    or (dir .. "/plugin.toml already exists, extend it if the tool needs permissions\n")
  return string.format(
    [[created %s
%s
finish wiring it in %s:
1. init.lua - add require("%s"), creating the file if missing.
2. plugin.toml - grant the tool's permissions under [permissions]; without the entry every gated call is denied.
3. ask the user to run /reload (or restart maki); no rebuild needed.]],
    module,
    manifest_note,
    dir,
    name
  )
end

function M.config_dir(path)
  if path then
    return maki.fs.abspath(path)
  end
  local dir = maki.env.config_dir()
  return dir and maki.fs.abspath(dir) or nil
end

function M.handler(input)
  local name = input.name
  if type(name) ~= "string" or not name:match(NAME_PATTERN) then
    return { llm_output = ERR_NAME, is_error = true }
  end

  local dir = M.config_dir(input.path)
  if not dir then
    return { llm_output = ERR_CONFIG_DIR, is_error = true }
  end

  local module = maki.fs.joinpath(dir, "lua", name .. ".lua")
  if maki.fs.metadata(module) then
    return { llm_output = ERR_EXISTS .. module, is_error = true }
  end

  local lua_dir = maki.fs.joinpath(dir, "lua")
  local made, mkdir_err = maki.fs.mkdir(lua_dir, { parents = true })
  if not made then
    return { llm_output = ERR_MKDIR .. lua_dir .. ": " .. tostring(mkdir_err), is_error = true }
  end

  local description = input.description or DEFAULT_DESCRIPTION
  local written, write_err = maki.fs.write(module, string.format(MODULE_SOURCE, description, name, name))
  if not written then
    return { llm_output = ERR_WRITE .. module .. ": " .. tostring(write_err), is_error = true }
  end

  local manifest = maki.fs.joinpath(dir, "plugin.toml")
  local manifest_created = false
  if not maki.fs.metadata(manifest) then
    local ok, err = maki.fs.write(manifest, MANIFEST)
    if not ok then
      return { llm_output = ERR_WRITE .. manifest .. ": " .. tostring(err), is_error = true }
    end
    manifest_created = true
  end

  return { llm_output = wiring(name, dir, module, manifest_created) }
end

return M
