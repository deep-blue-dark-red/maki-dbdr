-- Background tool-call queue. The spawn action returns job ids at once and
-- hands each job to a deferred runner pass; the model polls with `status` or
-- blocks with `wait`, and cancels what it no longer needs.
--
--   1. `q.jobs` is the single source of truth; renders, state, and lookups
--      are pure functions of it (async_lib.lua, which specs can run bare).
--   2. Lua tasks interleave only at awaits, so claim/settle never yield
--      between check and mark — that discipline is the whole critical
--      section.
--   3. Runners are `maki.defer_fn` passes on purpose: detached scope has no
--      deadline and no cancel token, so a job runs as long as its child's
--      own timeout, across turns. `maki.async.run` would die at its hidden
--      60s deadline.
--   4. Results complete out of order with K runners; every reply says so and
--      everything is keyed by job id, never position.

local ToolView = require("maki.tool_view")
local lib = require("async_lib")
local jobs = require("async_jobs")

local POLL_MS = 250
local DEFAULT_WAIT_TIMEOUT = 300
local MAX_JOBS_PER_SPAWN = 25
local MAX_ACTIVE_JOBS = 64
local MAX_KEEP_TERMINAL = 32
local RESULT_TAIL_LINES = 10

local EMPTY_ERROR = "provide at least one job"
local UNKNOWN_JOB_ERROR = "unknown job id or name"
local SPAWN_CAP_ERROR = string.format("maximum of %d jobs per spawn", MAX_JOBS_PER_SPAWN)
local QUEUE_FULL_ERROR = string.format("queue holds more than %d non-terminal jobs", MAX_ACTIVE_JOBS)
local OUT_OF_ORDER_NOTE =
  "Results arrive out of order as workers free up - always reference jobs by id or name, never by position."

local description = table.concat({
  "Queue independent tool calls in the background; spawn returns job ids immediately. Use batch when you want to wait for all calls, task for an autonomous subagent.",
  "",
  "- spawn (default): supply jobs as {tool, parameters, name?, timeout_seconds?}. Optional workers limits concurrency for this spawn.",
  "- status: inspect jobs; settled jobs show output tails, running jobs a live output tail.",
  "- wait: retrieve results for job_ids (ids or names; default all unfinished). Waits up to timeout_seconds (default 300; 0 returns immediately). A wait timeout does not stop jobs; wait again if needed.",
  "- cancel: cancel job_ids (default all unfinished). Queued jobs never start; running jobs are aborted, child processes killed, results discarded. Cancellation does not undo side effects.",
  "",
  "Match results by name (ids remain accepted), never position. Use returned names to collect needed results before finishing; jobs end with the session. Do not call async inside a job.",
}, "\n")

local opts = maki.api.register_options({
  workers = {
    default = 8,
    min = 1,
    desc = "Max concurrently running jobs. Spawn calls may lower this per call, never raise it.",
  },
  picker_key = {
    default = "<C-g>",
    desc = "Normal-mode key that opens the background jobs picker.",
  },
})

local schema = {
  type = "object",
  properties = {
    action = {
      type = "string",
      description = 'One of "spawn" (default), "status", "wait", "cancel"',
    },
    jobs = {
      type = "array",
      description = "spawn: jobs to queue, each { tool, parameters, name?, timeout_seconds? } or flat { tool, ...params }",
      items = {
        description = "Tool invocation with optional name and timeout_seconds",
      },
    },
    workers = {
      type = "integer",
      description = "spawn: concurrent jobs for this spawn call, clamped to the plugin's workers option",
    },
    job_ids = {
      type = "array",
      items = { type = "string" },
      description = "wait/cancel: job ids or names (default: all unfinished jobs)",
    },
    timeout_seconds = {
      type = "integer",
      description = "wait: seconds to block before returning current statuses (default 300; 0 returns immediately)",
    },
  },
}

local examples = {
  {
    jobs = {
      {
        tool = "bash",
        parameters = { command = "cargo build --release" },
        name = "build",
        timeout_seconds = 900,
      },
      { tool = "bash", parameters = { command = "cargo test" }, name = "test" },
    },
  },
  {
    action = "wait",
    job_ids = { "build" },
    timeout_seconds = 120,
  },
}

--- Queue state -------------------------------------------------------------

local q = { jobs = {}, runners = 0, workers = opts.workers, next_id = 1 }

local pump

local function settle(job, status, output)
  if lib.settle(job, status, output) then
    pump()
    jobs.refresh()
  end
end

local function run_job(job)
  if job.kill_requested then
    settle(job, lib.STATUS.KILLED, lib.KILL_QUEUED_MSG)
    return
  end
  local handle = maki.agent.kill_handle()
  job.kill_handle = handle
  local text, err = maki.agent.call_tool(job.ctx, job.tool, job.params, {
    timeout = job.timeout,
    kill = handle,
    on_live_buf = function(b)
      job.live_buf = b
    end,
    on_annotation = function(a)
      job.annotation = a
    end,
    on_usage = function(u)
      job.usage = u
    end,
  })
  job.kill_handle = nil
  if job.kill_requested then
    -- Keep the cancel reply's promise: the late result is not delivered.
    settle(job, lib.STATUS.KILLED, "killed; late result discarded")
  elseif err then
    local timed_out = job.timeout and job.timeout > 1 and os.time() - job.started_at >= job.timeout - 1
    settle(job, timed_out and lib.STATUS.TIMEOUT or lib.STATUS.ERROR, err)
  else
    settle(job, lib.STATUS.DONE, text)
  end
end

-- One pass runs one job, then hands back its slot. pcall so a runner error
-- can neither strand the job mid-running nor leak the slot count, which
-- would shrink the pool forever.
local function runner_pass()
  local job = lib.claim(q.jobs)
  if job then
    local ok, run_err = pcall(run_job, job)
    if not ok then
      settle(job, lib.STATUS.ERROR, tostring(run_err))
    end
  end
  q.runners = q.runners - 1
  pump()
end

pump = function()
  while q.runners < q.workers and lib.next_queued(q.jobs) do
    q.runners = q.runners + 1
    local ok = pcall(maki.defer_fn, runner_pass, 0)
    if not ok then
      q.runners = q.runners - 1
      return
    end
  end
end

--- Input helpers ------------------------------------------------------------

-- Let the child tool draw its own header; a missing one degrades to the
-- plain tool name, never fails the spawn.
local function header_spans(tool, params)
  local t = maki.api.get_tool(tool)
  local spans = t and t.header and t.header(params)
  return spans or { { tool, "tool" } }
end

local function active_count()
  local _, _, terminal = lib.counts(q.jobs)
  return #q.jobs - terminal
end

-- Drop the oldest terminal jobs past the keep cap so a long session cannot
-- grow the registry without bound. Indices are collected scanning from the
-- end, so removals never shift an index still to be removed.
local function prune()
  local terminal, dropped = 0, {}
  for i = #q.jobs, 1, -1 do
    local job = q.jobs[i]
    if lib.TERMINAL[job.status] then
      terminal = terminal + 1
      if terminal > MAX_KEEP_TERMINAL then
        dropped[#dropped + 1] = i
      end
    end
  end
  for _, i in ipairs(dropped) do
    table.remove(q.jobs, i)
  end
end

-- job_ids nil means every non-terminal job. Unknown ids fail the call so a
-- typo cannot silently wait on nothing.
local function resolve_jobs(input)
  local wanted = input.job_ids
  if wanted == nil then
    local out = {}
    for _, job in ipairs(q.jobs) do
      if not lib.TERMINAL[job.status] then
        out[#out + 1] = job
      end
    end
    return out
  end
  if type(wanted) ~= "table" then
    return nil, "job_ids must be an array of ids or names"
  end
  local out = {}
  for _, id in ipairs(wanted) do
    local job = lib.find(q.jobs, id)
    if not job then
      return nil, UNKNOWN_JOB_ERROR .. ": " .. tostring(id)
    end
    out[#out + 1] = job
  end
  return out
end

local function job_reply(jobs, extra)
  local out = {
    llm_output = lib.render_status(jobs, { results = true, live = true, tail = RESULT_TAIL_LINES }),
    state = lib.snapshot(jobs),
  }
  for k, v in pairs(extra or {}) do
    out[k] = v
  end
  return out
end

--- Actions -------------------------------------------------------------------

local function do_spawn(input, ctx)
  local entries = input.jobs
  if type(entries) ~= "table" or #entries == 0 then
    return { llm_output = EMPTY_ERROR, is_error = true }
  end
  if #entries > MAX_JOBS_PER_SPAWN then
    return { llm_output = SPAWN_CAP_ERROR, is_error = true }
  end
  if active_count() + #entries > MAX_ACTIVE_JOBS then
    return { llm_output = QUEUE_FULL_ERROR, is_error = true }
  end

  if input.workers ~= nil then
    local w = tonumber(input.workers)
    if w == nil then
      return { llm_output = "workers must be an integer", is_error = true }
    end
    q.workers = math.max(1, math.min(math.floor(w), opts.workers))
  end

  local specs, err = lib.validate_batch(entries, q.jobs)
  if not specs then
    return { llm_output = err, is_error = true }
  end

  local spawned = {}
  for _, spec in ipairs(specs) do
    local job = {
      id = "job-" .. q.next_id,
      name = spec.name,
      tool = spec.tool,
      params = spec.params,
      timeout = spec.timeout,
      ctx = ctx,
      header = header_spans(spec.tool, spec.params),
      status = lib.STATUS.QUEUED,
    }
    q.next_id = q.next_id + 1
    q.jobs[#q.jobs + 1] = job
    spawned[#spawned + 1] = job
  end
  prune()
  pump()
  jobs.refresh()

  local buf = maki.ui.buf()
  buf:set_lines(lib.render_queue_lines(spawned))
  ctx:live_buf(buf)
  return {
    llm_output = string.format(
      'Queued %d jobs (%d workers). %s\n%s\nPoll with action="status" or block with action="wait".',
      #spawned,
      q.workers,
      OUT_OF_ORDER_NOTE,
      lib.render_status(spawned, { results = false })
    ),
    body = buf,
    state = lib.snapshot(spawned),
  }
end

local function do_status()
  return job_reply(q.jobs)
end

local function do_wait(input)
  local jobs, err = resolve_jobs(input)
  if not jobs then
    return { llm_output = err, is_error = true }
  end
  -- A wait that lands after everything settled would otherwise answer with
  -- an empty queue; the model wants the results, so widen to the whole
  -- queue (terminal jobs included, tails on).
  if #jobs == 0 then
    jobs = q.jobs
  end
  local timeout = input.timeout_seconds == nil and DEFAULT_WAIT_TIMEOUT or input.timeout_seconds
  if type(timeout) ~= "number" or timeout < 0 then
    return { llm_output = "timeout_seconds must be a number >= 0", is_error = true }
  end
  local deadline = lib.wait_deadline(timeout)
  while not lib.all_terminal(jobs) do
    if os.time() >= deadline then
      break
    end
    maki.async.sleep(POLL_MS)
  end
  local reply = job_reply(jobs)
  if not lib.all_terminal(jobs) then
    reply.llm_output = reply.llm_output
      .. '\nStill running. Re-wait on these jobs, or poll action="status"; results arrive out of order.'
  end
  return reply
end

local function do_cancel(input)
  local jobs, err = resolve_jobs(input)
  if not jobs then
    return { llm_output = err, is_error = true }
  end
  local lines = {}
  if #jobs == 0 then
    lines[1] = "nothing to cancel"
  end
  for _, job in ipairs(jobs) do
    lines[#lines + 1] = lib.kill_job(job)
  end
  return { llm_output = table.concat(lines, "\n"), state = lib.snapshot(jobs) }
end

local function handler(input, ctx)
  local action = input.action or "spawn"
  if action == "spawn" then
    return do_spawn(input, ctx)
  elseif action == "status" then
    return do_status()
  elseif action == "wait" then
    return do_wait(input)
  elseif action == "cancel" then
    return do_cancel(input)
  end
  return { llm_output = 'unknown action "' .. tostring(action) .. '"', is_error = true }
end

local function header(input)
  local action = input.action or "spawn"
  if action == "spawn" then
    return #(input.jobs or {}) .. " jobs"
  end
  return action
end

local function restore(_input, output, _is_error, rctx)
  local st = rctx:state()
  local snap = st and type(st.jobs) == "table" and #st.jobs > 0 and st.jobs or nil
  if not snap then
    local tol = rctx:tool_output_lines()
    return ToolView.restore(output, { max_lines = tol.other, keep = "head" })
  end
  local tol = rctx:tool_output_lines()
  return ToolView.restore(
    lib.render_status(lib.jobs_from_snapshot(snap), { results = true, tail = RESULT_TAIL_LINES }),
    { max_lines = tol.other, keep = "head" }
  )
end

jobs.setup(function()
  return q.jobs
end, lib.kill_job, opts.picker_key)

maki.api.register_tool({
  name = "async",
  description = description,
  kind = "execute",
  audiences = { "main", "workflow", "research_sub", "general_sub" },
  schema = schema,
  examples = examples,
  header = header,
  handler = handler,
  restore = restore,
})
