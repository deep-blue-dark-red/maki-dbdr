-- The project folder paths maki knows, read from the state dir maki.env
-- hands out. `known(state_dir)` returns `{ { id, path } }`, one row per
-- project, path-sorted: every `projects/<id>/cwd_latest.json` maps the cwds
-- that ran there to their latest session, and each cwd resolves back to the
-- git root the project is keyed on. Unreadable indexes skip silently.

local M = {}

local fnv1a_64
do
  -- Lua's bit32 is 32-bit only, so we split the 64-bit FNV-1a state into
  -- hi/lo halves and propagate carries by hand during multiplication.
  local p_lo = 0x000001b3
  local p_hi = 0x00000100
  function fnv1a_64(data)
    local lo = 0x84222325
    local hi = 0xcbf29ce4
    for i = 1, #data do
      lo = bit32.bxor(lo, string.byte(data, i))
      local ll = lo * p_lo
      local ll_lo = ll % 0x100000000
      local ll_hi = (ll - ll_lo) / 0x100000000
      local new_hi = (hi * p_lo + lo * p_hi + ll_hi) % 0x100000000
      lo = ll_lo
      hi = new_hi
    end
    return string.format("%08x%08x", hi, lo)
  end
end

local function project_id(path)
  local base = maki.fs.basename(path) or "root"
  return base .. "-" .. fnv1a_64(path)
end

M.project_id = project_id
-- Test seam: the vectors that pin the hash live in the lib spec.
M._fnv1a_64 = fnv1a_64

function M.known(state_dir)
  local projects_root = maki.fs.joinpath(state_dir, "projects")
  local seen, out = {}, {}
  for _, entry in ipairs(maki.fs.dir(projects_root) or {}) do
    if entry[2] == "directory" then
      local raw = maki.fs.read(maki.fs.joinpath(projects_root, entry[1], "cwd_latest.json"))
      local index = raw and maki.json.decode(raw)
      if type(index) == "table" then
        for cwd in pairs(index) do
          local root = maki.fs.root(cwd, ".git") or cwd
          local id = project_id(root)
          if not seen[id] then
            seen[id] = true
            out[#out + 1] = { id = id, path = root }
          end
        end
      end
    end
  end
  table.sort(out, function(a, b)
    return a.path < b.path
  end)
  return out
end

return M
