-- Artificial Analysis intelligence scores, cached for the /model picker.
--
-- The picker quotes the index where it used to quote the tier. This plugin
-- fetches the public leaderboard once a day and writes
-- <state_dir>/aa_scores.json:
--
--   {"scores":{"glm-5-3":44.8,...},"estimated":["glm-5",...]}
--
-- Keys are the leaderboard's own slugs, which are lowercase and dashed
-- already, so `zai/glm-5.3` finds `glm-5-3` and an id AA spells differently
-- (`gemini-3.8-flash-high`) still finds its base model. maki-storage reads the
-- file; maki-ui renders one score per row.

local helpers = require("aa_scores_helpers")

local PAGE_URL = "https://artificialanalysis.ai/leaderboards/models"
local CACHE_NAME = "aa_scores.json"
local FETCH_TIMEOUT_SECS = 60

local function cache_path()
  local dir = maki.env.state_dir()
  return dir and maki.fs.joinpath(dir, CACHE_NAME) or nil
end

local function fresh(path)
  local meta = maki.fs.metadata(path)
  return meta ~= nil and not helpers.is_stale(meta.mtime, os.time())
end

--- Refetches the leaderboard and rewrites the cache. Returns how many scores
--- are cached, or nil plus a reason; a forced run over a good response is the
--- only thing that replaces an existing cache.
local function update(force)
  local path = cache_path()
  if not path then
    return nil, "state directory unavailable"
  end
  if not force and fresh(path) then
    return 0, nil
  end
  local resp, err = maki.net.request(PAGE_URL, { timeout = FETCH_TIMEOUT_SECS })
  if not resp then
    return nil, tostring(err)
  end
  if resp.status < 200 or resp.status >= 300 then
    return nil, "HTTP " .. resp.status
  end
  local scores, estimated, count = helpers.parse(resp.body)
  if count == 0 then
    return nil, "no scores found in the response"
  end
  local encoded, encode_err = maki.json.encode({ scores = scores, estimated = estimated })
  if not encoded then
    return nil, tostring(encode_err)
  end
  local ok, write_err = maki.fs.write(path, encoded)
  if not ok then
    return nil, tostring(write_err)
  end
  return count, nil
end

maki.api.register_command({
  name = "/aa_scores",
  description = "Refresh the cached Artificial Analysis intelligence scores",
  handler = function()
    maki.async.run(function()
      local count, err = update(true)
      if not count then
        maki.ui.flash("aa_scores: " .. err)
      else
        maki.ui.flash(("aa_scores: %d scores cached"):format(count))
      end
    end)
  end,
})

-- Startup keeps the cache warm without waiting for a command: a cold cache
-- fills in the background, a fresh one costs one metadata read, and a failed
-- fetch leaves whatever was cached before it.
maki.async.run(function()
  local _, err = update(false)
  if err then
    maki.log.warn("aa_scores: " .. err)
  end
end)
