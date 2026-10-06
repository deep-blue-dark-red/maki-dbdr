local DirPicker = require("maki.dir_picker")
local known_paths = require("maki.known_paths")

maki.api.register_command({
  name = "/cd-pick",
  description = "Pick a folder and change the working directory",
  nargs = 0,
  handler = function()
    local folders = {}
    local state = maki.env.state_dir()
    if state then
      for _, k in ipairs(known_paths.known(state)) do
        folders[#folders + 1] = { path = k.path }
      end
    end
    local target = DirPicker.open({
      title = " Change directory ",
      folders = folders,
      folder_label = "maki projects",
    })
    if target then
      -- The builtin /cd owns the bookkeeping a cwd change owes: the process
      -- directory, the input history of the new project, the session and the
      -- status bar.
      maki.api.run_command("/cd " .. target)
    end
  end,
})
