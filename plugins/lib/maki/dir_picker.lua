-- Folder picker on top of ListPicker: browse the file system from {start}
-- and, when the caller offers shortcut folders, switch to them with Tab.
-- Blocks until a folder is picked and returns its absolute path, or nil when
-- dismissed.
--
-- Keys while open:
--   Enter       descend into the highlighted folder, or take a shortcut row
--   <C-m>/<M-m> take the folder being browsed itself (browse mode only)
--   <Tab>       switch between shortcuts and the file system
--   <Esc>       dismiss
--
-- {opts}:
--   title (string) float title.
--   start (string?) folder to browse first, default ~.
--   folders (table?) shortcut rows, `{ label?, path }` each, offered as the
--     first mode when non-empty.
--   folder_label (string?) section header naming the shortcut rows.

local ListPicker = require("maki.list_picker")
local shorten_path = require("maki.shorten_path")

local DirPicker = {}

-- Rows for one browse step: ".." first, then every subfolder with dot folders
-- sunk. The section header names where you are, and ".." keeps the list never
-- empty so the filter always has something to sit under.
local function browse_items(browse_dir)
  local items = {}
  local here = shorten_path(browse_dir)
  local parent = maki.fs.dirname(browse_dir)
  if parent and parent ~= browse_dir then
    items[#items + 1] = { label = "..", kind = "up", section = here }
  end
  local entries, list_err = maki.fs.dir(browse_dir)
  local dirs = {}
  for _, e in ipairs(entries or {}) do
    if e[2] == "directory" then
      dirs[#dirs + 1] = e[1]
    end
  end
  table.sort(dirs, function(a, b)
    local hidden_a, hidden_b = a:sub(1, 1) == ".", b:sub(1, 1) == "."
    if hidden_a ~= hidden_b then
      return hidden_b
    end
    return a:lower() < b:lower()
  end)
  for _, name in ipairs(dirs) do
    items[#items + 1] = {
      label = name .. "/",
      kind = "dir",
      path = maki.fs.joinpath(browse_dir, name),
      section = here,
    }
  end
  return items, list_err
end

local function folder_items(folders, label)
  local items = {}
  for _, f in ipairs(folders) do
    items[#items + 1] = {
      label = f.label or shorten_path(f.path),
      kind = "folder",
      path = f.path,
      section = label,
    }
  end
  return items
end

function DirPicker.open(opts)
  opts = opts or {}
  local folders = opts.folders or {}
  local folder_label = opts.folder_label or "folders"
  local browse = opts.start or maki.uv.os_homedir() or "/"
  -- The caller's shortcuts are the fast path, so they open first.
  local mode = #folders > 0 and "folders" or "browse"

  local function items_for()
    if mode == "folders" then
      return folder_items(folders, folder_label)
    end
    return browse_items(browse)
  end

  local footer = { { "Enter", "open" } }
  if #folders > 0 then
    footer[#footer + 1] = { "Tab", "switch mode" }
  end
  footer[#footer + 1] = { "Alt+M", "pick folder" }

  while true do
    local items, list_err = items_for()
    if list_err then
      maki.ui.flash("Cannot list " .. shorten_path(browse) .. ": " .. tostring(list_err))
    end
    local event = ListPicker.open(items, {
      title = opts.title,
      key = function(item)
        return item.kind .. ":" .. (item.path or item.label)
      end,
      -- C-m is Enter on terminals without the kitty keyboard protocol; M-m works everywhere.
      action_keys = { "<C-m>", "<M-m>" },
      live_keys = {
        ["<Tab>"] = function()
          if #folders == 0 then
            return nil
          end
          mode = mode == "folders" and "browse" or "folders"
          return items_for()
        end,
      },
      footer = footer,
    })
    if event.type == "close" then
      return nil
    end
    if event.type == "key" then
      -- The pick key means the folder being browsed; from the shortcuts it
      -- first jumps to the browser, so there is something to pick.
      if mode == "browse" then
        return browse
      end
      mode = "browse"
    elseif event.item then
      local item = event.item
      if item.kind == "folder" then
        return item.path
      elseif item.kind == "dir" then
        browse = item.path
      elseif item.kind == "up" then
        browse = maki.fs.dirname(browse) or browse
      end
    else
      return nil
    end
  end
end

DirPicker._browse_items = browse_items
DirPicker._folder_items = folder_items

return DirPicker
