-- Row building for the /subtasks picker in picker.lua: filtering, ordering and
-- sections, plus the pure row formatting helpers, with no
-- host calls and no globals. Every refresh throws the old rows away and builds
-- them again, so nothing here can drift from the host list.

local ListPicker = require("maki.list_picker")

local RUNNING_SECTION = "Running"
local FINISHED_SECTION = "Finished"
local CLOCK_FORMAT = "%H:%M"

local M = {}

function M.human_tokens(tokens)
  if tokens >= 1000000 then
    return string.format("%.1fM", tokens / 1000000)
  elseif tokens >= 1000 then
    return string.format("%.1fK", tokens / 1000)
  end
  return tostring(tokens)
end

function M.human_elapsed(secs)
  secs = math.max(secs, 0)
  if secs < 60 then
    return string.format("%ds", secs)
  elseif secs < 3600 then
    return string.format("%dm %02ds", math.floor(secs / 60), secs % 60)
  end
  return string.format("%dh %02dm", math.floor(secs / 3600), math.floor((secs % 3600) / 60))
end

-- Indirect so the specs can pin the clock.
function M.now()
  return os.time()
end

-- How long a task has run: live while working, frozen at its end otherwise.
-- Tasks restored from disk have no spawn time and show nothing.
function M.elapsed_of(task)
  if not task.started_at then
    return nil
  end
  local now = task.status == "working" and M.now() or (task.ended_at or M.now())
  return M.human_elapsed(now - task.started_at)
end

-- The row's second line: context used and, when known, the start time.
function M.meta_of(task)
  local parts = {}
  if (task.context or 0) > 0 then
    parts[#parts + 1] = M.human_tokens(task.context)
  end
  if task.started_at then
    parts[#parts + 1] = os.date(CLOCK_FORMAT, task.started_at)
  end
  return #parts > 0 and table.concat(parts, " · ") or nil
end

-- The main chat comes first and has no status. The subagents follow, running
-- ones above finished ones so a long job never gets buried under the ones that
-- already returned. Within a section, chat order.
--
-- Returns { rows, sections }. A row carries a section header only when it opens
-- one, and `sections` counts what survived the filter.
function M.build(tasks, query)
  local words = ListPicker.split_words(query)
  local main, running, finished = nil, {}, {}
  for _, task in ipairs(tasks) do
    if ListPicker.matches(task.name, words) then
      if not task.status then
        main = task
      elseif task.status == "working" then
        running[#running + 1] = task
      else
        finished[#finished + 1] = task
      end
    end
  end

  local rows = {}
  if main then
    rows[#rows + 1] = { task = main }
  end
  for _, group in ipairs({ { RUNNING_SECTION, running }, { FINISHED_SECTION, finished } }) do
    for i, task in ipairs(group[2]) do
      rows[#rows + 1] = { task = task, section = i == 1 and group[1] or nil }
    end
  end
  return { rows = rows, sections = { running = #running, finished = #finished } }
end

-- Position of {id} among {rows}, or nil. The selection is kept as an id and
-- resolved here at render time, so a task moving between sections never drags
-- the cursor with it.
function M.index_of(rows, id)
  for i, row in ipairs(rows) do
    if row.task.id == id then
      return i
    end
  end
  return nil
end

return M
