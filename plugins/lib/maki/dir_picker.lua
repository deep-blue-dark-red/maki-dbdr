-- Folder picker on top of ListPicker: browse the file system from {start}
-- and, with Tab, switch to shortcut rows — the caller's folders plus every
-- known maki project. Blocks until a folder is picked and returns its
-- absolute path, or nil when dismissed.
--
-- Keys while open:
--   Enter       pick the highlighted shortcut, descend into the highlighted
--               folder, or — on ".." — pick the folder being browsed
--   <Right>     enter the highlighted folder or shortcut without picking
--   <Left>      climb to the parent folder
--   <Tab>       switch between shortcuts and the file system
--   <Esc>       dismiss
--
-- {opts}:
--   title (string) float title.
--   start (string?) folder to browse first, default ~.
--   folders (table?) shortcut rows, `{ label?, path }` each, offered as the
--     first mode when non-empty. Known maki projects are always appended.
--   folder_label (string?) section header naming the shortcut rows.

local ListPicker = require("maki.list_picker")
local known_paths = require("maki.known_paths")
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
    items[#items + 1] = { label = "..", detail = "pick this folder", kind = "up", section = here }
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

-- The caller's shortcut rows, with {projects} — known maki projects — that
-- are not already offered appended, so every picker can Tab to the projects
-- you visit.
local function shortcut_folders(folders, projects)
  local merged, seen = {}, {}
  for _, f in ipairs(folders) do
    merged[#merged + 1] = { label = f.label, path = f.path }
    seen[f.path] = true
  end
  for _, p in ipairs(projects or {}) do
    if not seen[p.path] then
      merged[#merged + 1] = { path = p.path }
    end
  end
  return merged
end

function DirPicker.open(opts)
  opts = opts or {}
  local projects = {}
  local state = maki.env.state_dir()
  if state then
    projects = known_paths.known(state)
  end
  local folders = shortcut_folders(opts.folders or {}, projects)
  local folder_label = opts.folder_label or "maki projects"
  local browse = opts.start or maki.uv.os_homedir() or "/"
  -- The caller's shortcuts are the fast path, so they open first.
  local mode = #folders > 0 and "folders" or "browse"

  local function items_for()
    if mode == "folders" then
      return folder_items(folders, folder_label)
    end
    return browse_items(browse)
  end

  local function enter_dir(item)
    if item == nil then
      return nil
    end
    if item.kind == "dir" then
      browse = item.path
    elseif item.kind == "folder" then
      -- A shortcut enters the browser, so Left can climb back out of it.
      mode = "browse"
      browse = item.path
    else
      return nil
    end
    return items_for()
  end

  -- Enter swaps into a directory; a shortcut or ".." falls through to the
  -- submit, which picks.
  local function enter_or_pick(item)
    if item and item.kind == "dir" then
      return enter_dir(item)
    end
    return nil
  end

  local footer = {
    { "Enter", "pick / enter" },
    { "←", "parent" },
    { "→", "enter folder" },
  }
  if #folders > 0 then
    footer[#footer + 1] = { "Tab", "switch mode" }
  end
  footer[#footer + 1] = { "Esc", "close" }

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
      submit_swaps = true,
      live_keys = {
        ["<CR>"] = enter_or_pick,
        ["<Right>"] = enter_dir,
        ["<Left>"] = function()
          if mode ~= "browse" then
            return nil
          end
          local parent = maki.fs.dirname(browse)
          if not parent or parent == browse then
            return nil
          end
          browse = parent
          return items_for()
        end,
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
    if event.item then
      local item = event.item
      if item.kind == "folder" then
        return item.path
      elseif item.kind == "up" then
        return browse
      end
      -- A dir never arrives as a choice: Enter swaps into it instead.
    else
      return nil
    end
  end
end

DirPicker._browse_items = browse_items
DirPicker._folder_items = folder_items
DirPicker._shortcut_folders = shortcut_folders

return DirPicker
