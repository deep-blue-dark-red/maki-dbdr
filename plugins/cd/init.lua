local DirPicker = require("maki.dir_picker")

maki.api.register_command({
  name = "/cd-pick",
  description = "Pick a folder and change the working directory",
  nargs = 0,
  handler = function()
    local target = DirPicker.open({ title = " Change directory " })
    if target then
      -- The builtin /cd owns the bookkeeping a cwd change owes: the process
      -- directory, the input history of the new project, the session and the
      -- status bar.
      maki.api.run_command("/cd " .. target)
    end
  end,
})
