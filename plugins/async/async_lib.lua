-- Pure queue logic for plugins/async. No `maki.*` here, so tests/spec.lua
-- can run this file under plain lua.

local M = {}

M.ASYNC_TOOL = "async"
M.MAX_NAME_LEN = 80
M.NO_OUTPUT = "(no output)"
M.KILL_QUEUED_MSG = "cancelled before start"
M.MAX_INLINE_OUTPUT = 72

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

-- What the job ran: the label captured at spawn (the tool's own header
-- lead, for bash the command), degrading to the header lead or tool name
-- for jobs rebuilt from a snapshot.
function M.job_label(job)
  if job.label and job.label ~= "" then
    return job.label
  end
  if job.header and job.header[1] then
    return job.header[1][1]
  end
  return job.tool or "job"
end

-- Most recent output line, from a running job's live buf or a settled
-- job's captured output, capped so one long line cannot eat a panel row.
function M.last_output_line(job)
  local text
  if job.status == M.STATUS.RUNNING and job.live_buf then
    local lines = M.split_lines(M.live_text(job.live_buf))
    text = lines[#lines]
  elseif job.output and job.output ~= "" then
    local lines = M.split_lines(job.output)
    text = lines[#lines]
  end
  if text == nil or text == "" then
    return nil
  end
  if #text > M.MAX_INLINE_OUTPUT then
    return text:sub(1, M.MAX_INLINE_OUTPUT - 3) .. "…"
  end
  return text
end

function M.name_taken(jobs, name)
  for _, job in ipairs(jobs) do
    if not M.TERMINAL[job.status] and job.name == name then
      return true
    end
  end
  return false
end

-- Validate a whole spawn batch before any job is registered. Returns the
-- normalized specs, or nil plus the offending entry's error. Checks names
-- against both the live queue and duplicates within the batch.
function M.validate_batch(entries, jobs)
  local specs = {}
  local seen = {}
  for _, entry in ipairs(entries) do
    local spec, err = M.normalize_entry(entry)
    if not spec then
      return nil, err
    end
    if spec.name then
      if seen[spec.name] or M.name_taken(jobs, spec.name) then
        return nil, "job name already in use: " .. spec.name
      end
      seen[spec.name] = true
    end
    specs[#specs + 1] = spec
  end
  return specs
end

-- The one kill path, shared by the tool's `cancel` action and the jobs
-- panel. Queued jobs die before they start; running ones get their
-- in-flight call aborted and the late result discarded. A running job
-- always carries a kill handle: run_job sets it before its first yield.
function M.kill_job(job)
  if M.TERMINAL[job.status] then
    return string.format("%s already %s", job.id, job.status)
  end
  if job.status == M.STATUS.RUNNING then
    job.kill_requested = true
    job.kill_handle:kill()
    return string.format("%s killed: the in-flight call aborts and its result is discarded", job.id)
  end
  M.settle(job, M.STATUS.KILLED, M.KILL_QUEUED_MSG)
  return string.format("%s killed (was queued)", job.id)
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

-- Poll deadline for `wait`. Always a number, even for timeout 0 (stop now):
-- a nil deadline would spin until jobs settle, the opposite of "returns now".
function M.wait_deadline(timeout)
  return os.time() + math.floor(timeout)
end

function M.elapsed(job, now)
  if job.started_at == nil then
    return nil
  end
  return (job.settled_at or now or os.time()) - job.started_at
end

function M.status_label(job)
  if job.kill_requested and not M.TERMINAL[job.status] then
    return "killing"
  end
  local secs = M.elapsed(job)
  return secs and string.format("%s (%ds)", job.status, secs) or job.status
end

-- The invoked command as one line, for titles where a job id alone says
-- nothing. Header spans are concatenation-ready (they carry their own
-- spacing), so they join with no separator. Falls back to the tool name
-- when no header spans exist (jobs restored from a snapshot).
function M.job_command(job)
  if job.header and #job.header > 0 then
    local parts = {}
    for _, s in ipairs(job.header) do
      parts[#parts + 1] = s[1]
    end
    return table.concat(parts)
  end
  return job.tool or ""
end

-- Span lines for the spawn's live body buf. Pure over jobs, so restore
-- can rebuild the same view from a state snapshot.
function M.render_queue_lines(jobs)
  local lines = {}
  for _, job in ipairs(jobs) do
    local ind = M.INDICATOR[job.status] or M.INDICATOR[M.STATUS.QUEUED]
    local spans = { { ind[1], ind[2] }, { M.job_label(job), "tool" } }
    if job.name then
      spans[#spans + 1] = { " " .. job.name, "tool_prefix" }
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

-- Kept lines share the shape of a settled tail: indented, oldest dropped
-- first with a counter line. Both views of a job must render the same, so
-- the live path and the settled path cap through this one helper.
function M.cap_tail(lines, max_lines)
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

function M.tail_lines(text, max_lines)
  return M.cap_tail(M.split_lines(text), max_lines)
end

-- Live snapshot lines are span tables ({text, style?}); the tail/cap
-- helpers concatenate, so flatten to plain text first. Plain strings pass
-- through untouched.
function M.plain_lines(lines)
  local out = {}
  for i, line in ipairs(lines) do
    if type(line) == "table" then
      local spans = {}
      for j, span in ipairs(line) do
        spans[j] = span[1]
      end
      out[i] = table.concat(spans)
    else
      out[i] = line
    end
  end
  return out
end

function M.live_text(live_buf)
  return table.concat(M.plain_lines(live_buf:get_lines()), "\n")
end

function M.split_lines(text)
  local lines = {}
  for line in (text or ""):gmatch("([^\n]*)\n?") do
    lines[#lines + 1] = line
  end
  -- gmatch on the trailing newline yields one phantom "" past the end.
  if lines[#lines] == "" then
    lines[#lines] = nil
  end
  return lines
end

-- One status line per job for the auto-showing panel.
function M.build_panel_lines(jobs)
  local lines = {}
  for _, job in ipairs(jobs) do
    local ind = M.INDICATOR[job.status] or M.INDICATOR[M.STATUS.QUEUED]
    local spans = { { ind[1], ind[2] }, { M.job_label(job), "tool" } }
    if job.name then
      spans[#spans + 1] = { " " .. job.name, "tool_prefix" }
    end
    spans[#spans + 1] = { "  " .. M.status_label(job), "dim" }
    if job.annotation then
      spans[#spans + 1] = { "  (" .. job.annotation .. ")", "tool_annotation" }
    end
    local last = M.last_output_line(job)
    if last then
      spans[#spans + 1] = { "  " .. last, "dim" }
    end
    lines[#lines + 1] = spans
  end
  return lines
end

-- Picker rows: one per job, row identity is the id.
function M.build_items(jobs)
  local items = {}
  for _, job in ipairs(jobs) do
    local last = M.last_output_line(job)
    items[#items + 1] = {
      id = job.id,
      label = string.format("%s %s [%s]", M.job_label(job), job.name or "", M.status_label(job)),
      detail = last and { { M.job_command(job), "dim", elastic = true }, { " · " .. last, "dim" } }
        or M.job_command(job),
    }
  end
  return items
end

-- Full console lines for the stdout split: a running job's live snapshot
-- (span tables flattened), otherwise its captured output. Uncapped: the
-- split renders the complete log, appending live lines as they stream.
function M.console_lines(job, live)
  if live and #live > 0 then
    return M.plain_lines(live)
  end
  if job.output and job.output ~= "" then
    return M.split_lines(job.output)
  end
  return { M.NO_OUTPUT }
end

-- Plain-text queue for llm_output and for restore (through ToolView).
-- {results = true} appends each terminal job's output tail; {live = true}
-- appends a running job's tail from its live buffer, so a poll can watch a
-- long-lived job (a dev server) without waiting for it to settle.
function M.render_status(jobs, o)
  o = o or {}
  local now = os.time()
  local tail = o.tail or 10
  local queued, running, terminal = M.counts(jobs)
  local out = { string.format("queue: %d running, %d queued, %d terminal", running, queued, terminal) }
  for _, job in ipairs(jobs) do
    out[#out + 1] = string.format("%s %s [%s]", M.job_label(job), job.name or "", M.status_label(job))
    local showed_tail = false
    if M.TERMINAL[job.status] then
      if o.results and job.output and job.output ~= "" then
        for _, line in ipairs(M.tail_lines(job.output, tail)) do
          out[#out + 1] = line
        end
        showed_tail = true
      end
    elseif o.live and job.live_buf then
      for _, line in ipairs(M.cap_tail(M.split_lines(M.live_text(job.live_buf)), tail)) do
        out[#out + 1] = line
      end
      showed_tail = true
    end
    if not showed_tail then
      local last = M.last_output_line(job)
      if last then
        out[#out + 1] = "    " .. last
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
      label = job.label,
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
      label = sj.label,
      status = M.TERMINAL[sj.status] and sj.status or M.STATUS.ERROR,
      output = M.TERMINAL[sj.status] and sj.output or "interrupted (session ended before completion)",
      annotation = sj.annotation,
      usage = sj.usage,
    }
  end
  return jobs
end

return M
