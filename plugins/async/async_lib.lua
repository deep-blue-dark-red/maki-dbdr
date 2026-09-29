-- Pure queue logic for plugins/async. No `maki.*` here, so tests/spec.lua
-- can run this file under plain lua.

local M = {}

M.ASYNC_TOOL = "async"
M.MAX_NAME_LEN = 80

M.STATUS = {
  QUEUED = "queued",
  RUNNING = "running",
  DONE = "done",
  ERROR = "error",
  KILLED = "killed",
  TIMEOUT = "timeout",
}

M.TERMINAL = {
  [M.STATUS.DONE] = true,
  [M.STATUS.ERROR] = true,
  [M.STATUS.KILLED] = true,
  [M.STATUS.TIMEOUT] = true,
}

M.INDICATOR = {
  [M.STATUS.QUEUED] = { "○ ", "dim" },
  [M.STATUS.RUNNING] = { "· ", "spinner" },
  [M.STATUS.DONE] = { "● ", "tool_success" },
  [M.STATUS.ERROR] = { "● ", "tool_error" },
  [M.STATUS.KILLED] = { "● ", "tool_error" },
  [M.STATUS.TIMEOUT] = { "● ", "tool_error" },
}

-- Models send entries in two shapes, { tool, parameters } and flat
-- { tool, ...params }, so accept either, or even both merged, as long
-- as no key appears twice. Mirrors batch's normalize_entry with the
-- async-specific fields and the self-nesting guard folded in.
function M.normalize_entry(entry)
  if type(entry) ~= "table" then
    return nil, "job entry must be an object"
  end
  local tool = entry.tool
  if type(tool) ~= "string" then
    return nil, "job entry missing 'tool'"
  end
  -- Lua twin of `canonical_tool_name` (streaming.rs): strip GPT's
  -- `functions.` prefix.
  tool = tool:match("^functions%.(.+)") or tool
  if tool == M.ASYNC_TOOL then
    return nil, "cannot queue async inside async"
  end
  local name = entry.name
  if name ~= nil then
    if type(name) ~= "string" or #name == 0 or #name > M.MAX_NAME_LEN then
      return nil, "job 'name' must be a non-empty string of at most " .. M.MAX_NAME_LEN .. " bytes"
    end
  end
  local timeout = entry.timeout_seconds
  if timeout ~= nil then
    if type(timeout) ~= "number" or timeout < 1 then
      return nil, "job 'timeout_seconds' must be a number >= 1"
    end
    timeout = math.floor(timeout)
  end
  local rest = {}
  local has_rest = false
  for k, v in pairs(entry) do
    if k ~= "tool" and k ~= "parameters" and k ~= "name" and k ~= "timeout_seconds" then
      rest[k] = v
      has_rest = true
    end
  end
  local nested = entry.parameters
  local params
  if nested == nil then
    if not has_rest then
      return nil, "job entry missing 'parameters'"
    end
    params = rest
  elseif not has_rest then
    params = nested
  elseif type(nested) ~= "table" then
    return nil, "'parameters' must be an object when flat fields are also present"
  else
    params = rest
    for k, v in pairs(nested) do
      if params[k] ~= nil then
        return nil, "duplicate parameter '" .. k .. "' in both 'parameters' and flat fields"
      end
      params[k] = v
    end
  end
  return { tool = tool, params = params, name = name, timeout = timeout }
end

function M.name_taken(jobs, name)
  for _, job in ipairs(jobs) do
    if not M.TERMINAL[job.status] and job.name == name then
      return true
    end
  end
  return false
end

-- First queued job in spawn order, marked running synchronously. The
-- caller must not yield between this and whatever it stores from the
-- return: on a cooperative executor that absence of a yield is the
-- whole critical section.
function M.claim(jobs)
  for _, job in ipairs(jobs) do
    if job.status == M.STATUS.QUEUED then
      job.status = M.STATUS.RUNNING
      job.started_at = os.time()
      return job
    end
  end
  return nil
end

function M.next_queued(jobs)
  for _, job in ipairs(jobs) do
    if job.status == M.STATUS.QUEUED then
      return job
    end
  end
  return nil
end

-- Refuses to run twice, so status, output, and elapsed never disagree.
function M.settle(job, status, output)
  if M.TERMINAL[job.status] then
    return false
  end
  job.status = status
  job.output = output
  job.settled_at = os.time()
  return true
end

function M.find(jobs, id_or_name)
  for _, job in ipairs(jobs) do
    if job.id == id_or_name or (job.name and job.name == id_or_name) then
      return job
    end
  end
  return nil
end

function M.counts(jobs)
  local queued, running, terminal = 0, 0, 0
  for _, job in ipairs(jobs) do
    if job.status == M.STATUS.QUEUED then
      queued = queued + 1
    elseif job.status == M.STATUS.RUNNING then
      running = running + 1
    else
      terminal = terminal + 1
    end
  end
  return queued, running, terminal
end

function M.all_terminal(jobs)
  for _, job in ipairs(jobs) do
    if not M.TERMINAL[job.status] then
      return false
    end
  end
  return true
end

function M.elapsed(job, now)
  if job.started_at == nil then
    return nil
  end
  return (job.settled_at or now or os.time()) - job.started_at
end

function M.status_label(job)
  local secs = M.elapsed(job)
  return secs and string.format("%s (%ds)", job.status, secs) or job.status
end

-- Span lines for the spawn's live body buf. Pure over jobs, so restore
-- can rebuild the same view from a state snapshot.
function M.render_queue_lines(jobs)
  local lines = {}
  for _, job in ipairs(jobs) do
    local ind = M.INDICATOR[job.status] or M.INDICATOR[M.STATUS.QUEUED]
    local spans = { { ind[1], ind[2] }, { job.id, "dim" } }
    if job.name then
      spans[#spans + 1] = { " " .. job.name, "tool_prefix" }
    end
    for _, s in ipairs(job.header or { { job.tool, "tool" } }) do
      spans[#spans + 1] = s
    end
    if job.annotation then
      spans[#spans + 1] = { " (" .. job.annotation .. ")", "tool_annotation" }
    end
    if job.usage then
      spans[#spans + 1] = { "  " .. job.usage, "dim" }
    end
    lines[#lines + 1] = spans
  end
  return lines
end

local TAIL_INDENT = "    "

function M.tail_lines(text, max_lines)
  local lines = {}
  for line in (text or ""):gmatch("([^\n]*)\n?") do
    lines[#lines + 1] = line
  end
  -- gmatch on the trailing newline yields one phantom "" past the end.
  if lines[#lines] == "" then
    lines[#lines] = nil
  end
  local dropped = #lines - max_lines
  local out = {}
  for i = dropped > 0 and dropped + 1 or 1, #lines do
    out[#out + 1] = TAIL_INDENT .. lines[i]
  end
  if dropped > 0 then
    table.insert(out, 1, TAIL_INDENT .. "(" .. dropped .. " earlier lines omitted)")
  end
  return out
end

-- Plain-text queue for llm_output and for restore (through ToolView).
-- {results = true} appends each terminal job's output tail.
function M.render_status(jobs, o)
  o = o or {}
  local now = os.time()
  local queued, running, terminal = M.counts(jobs)
  local out = { string.format("queue: %d running, %d queued, %d terminal", running, queued, terminal) }
  for _, job in ipairs(jobs) do
    out[#out + 1] = string.format("%s %s [%s]", job.id, job.name or "", M.status_label(job))
    if o.results and M.TERMINAL[job.status] and job.output and job.output ~= "" then
      for _, line in ipairs(M.tail_lines(job.output, o.tail or 10)) do
        out[#out + 1] = line
      end
    end
  end
  return table.concat(out, "\n")
end

function M.snapshot(jobs)
  local out = {}
  for i, job in ipairs(jobs) do
    out[i] = {
      id = job.id,
      name = job.name,
      tool = job.tool,
      status = job.status,
      output = job.output,
      annotation = job.annotation,
      usage = job.usage,
    }
  end
  return { jobs = out }
end

-- Rebuild renderable jobs from a state snapshot; headers degrade to the
-- plain tool name, which is all the snapshot keeps.
function M.jobs_from_snapshot(snap)
  local jobs = {}
  for i, sj in ipairs(snap) do
    jobs[i] = {
      id = sj.id,
      name = sj.name,
      tool = sj.tool,
      status = M.TERMINAL[sj.status] and sj.status or M.STATUS.ERROR,
      output = M.TERMINAL[sj.status] and sj.output or "interrupted (session ended before completion)",
      annotation = sj.annotation,
      usage = sj.usage,
    }
  end
  return jobs
end

return M
