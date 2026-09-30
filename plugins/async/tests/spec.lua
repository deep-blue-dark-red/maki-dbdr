local lib = require("async_lib")

local failures = {}

local function case(name, fn)
  local ok, err = pcall(fn)
  if not ok then
    failures[#failures + 1] = name .. ": " .. tostring(err)
  end
end

local function eq(actual, expected, msg)
  if actual ~= expected then
    error((msg or "") .. "\nexpected: " .. tostring(expected) .. "\n  actual: " .. tostring(actual))
  end
end

local function contains(haystack, needle, msg)
  if not haystack:find(needle, 1, true) then
    error((msg or "") .. "\nmissing: " .. needle .. "\nin: " .. haystack)
  end
end

local function job(status)
  return { id = "job-1", status = status or lib.STATUS.QUEUED, tool = "bash" }
end

-- normalize_entry rejects with (nil, err), which plain assert would treat
-- as a failure; demand the pair explicitly.
local function assert_rejects(entry, msg)
  local v, err = lib.normalize_entry(entry)
  if v ~= nil or err == nil then
    error(msg .. " (expected rejection, got: " .. tostring(v) .. ")")
  end
end

case("normalize_entry_shapes", function()
  local nested = lib.normalize_entry({ tool = "bash", parameters = { command = "ls" } })
  eq(nested.tool, "bash", "nested shape")
  eq(nested.params.command, "ls", "nested params")

  local flat = lib.normalize_entry({ tool = "bash", command = "ls" })
  eq(flat.tool, "bash", "flat shape")
  eq(flat.params.command, "ls", "flat params")

  local both = lib.normalize_entry({ tool = "bash", command = "a", parameters = { cwd = "/tmp" } })
  eq(both.params.command, "a", "merged flat")
  eq(both.params.cwd, "/tmp", "merged nested")

  local dup = lib.normalize_entry({ tool = "bash", command = "a", parameters = { command = "b" } })
  assert(dup == nil, "duplicate key rejected")
end)

case("normalize_entry_rejections", function()
  assert_rejects("nope", "non-table rejected")
  assert_rejects({ parameters = {} }, "missing tool rejected")
  assert_rejects({ tool = "bash" }, "missing params rejected")
  assert_rejects({ tool = "async", parameters = {} }, "self nesting rejected")
  assert_rejects({ tool = "bash", parameters = {}, name = "" }, "empty name rejected")
  assert_rejects({ tool = "bash", parameters = {}, timeout_seconds = 0 }, "zero timeout rejected")
  assert(
    lib.normalize_entry({ tool = "bash", parameters = {}, timeout_seconds = 2.9 }) == nil or true,
    "fractional timeout floored, not rejected"
  )
  local floored = lib.normalize_entry({ tool = "bash", parameters = {}, timeout_seconds = 2.9 })
  eq(floored.timeout, 2, "timeout floored to whole seconds")
  local functions_prefixed = lib.normalize_entry({ tool = "functions.bash", parameters = {} })
  eq(functions_prefixed.tool, "bash", "functions. prefix stripped")
end)

case("claim_marks_running_without_skipping", function()
  local jobs = { job(), job() }
  local first = lib.claim(jobs)
  eq(first.status, lib.STATUS.RUNNING, "first queued claimed")
  eq(lib.next_queued(jobs), jobs[2], "second still queued")
  eq(lib.claim(jobs), jobs[2], "second claim takes it")
  eq(lib.claim(jobs), nil, "empty queue claims nothing")
end)

case("claim_is_fifo_on_spawn_order", function()
  local jobs = { job(), job(), job() }
  eq(lib.claim(jobs), jobs[1], "fifo: first spawned claimed first")
end)

case("settle_refuses_double", function()
  local j = job()
  lib.settle(j, lib.STATUS.DONE, "out")
  eq(lib.settle(j, lib.STATUS.ERROR, "late"), false, "second settle refused")
  eq(j.status, lib.STATUS.DONE, "status kept from first settle")
  eq(j.output, "out", "output kept from first settle")
end)

case("find_by_id_and_name", function()
  local jobs = { job(lib.STATUS.DONE), job(lib.STATUS.RUNNING) }
  jobs[1].id, jobs[2].id = "job-1", "job-2"
  jobs[2].name = "build"
  eq(lib.find(jobs, "job-2"), jobs[2], "by id")
  eq(lib.find(jobs, "build"), jobs[2], "by name")
  eq(lib.find(jobs, "nope"), nil, "unknown")
end)

case("name_taken_ignores_terminal", function()
  local jobs = { job(lib.STATUS.DONE) }
  jobs[1].name = "build"
  eq(lib.name_taken(jobs, "build"), false, "terminal name reusable")
  table.insert(jobs, job(lib.STATUS.RUNNING))
  jobs[2].name = "build"
  eq(lib.name_taken(jobs, "build"), true, "active name taken")
end)

case("counts_and_all_terminal", function()
  local jobs = { job(), job(lib.STATUS.RUNNING), job(lib.STATUS.DONE), job(lib.STATUS.KILLED) }
  local queued, running, terminal = lib.counts(jobs)
  eq(queued, 1, "queued count")
  eq(running, 1, "running count")
  eq(terminal, 2, "terminal count")
  eq(lib.all_terminal(jobs), false, "mixed queue not terminal")
  eq(lib.all_terminal({}), true, "empty queue is terminal")
end)

case("wait_deadline_always_set", function()
  local now = os.time()
  local zero = lib.wait_deadline(0)
  eq(type(zero), "number", "zero timeout yields a deadline, never nil")
  eq(zero <= now + 1, true, "zero timeout deadline is immediate")
  eq(lib.wait_deadline(300) >= now + 300, true, "positive timeout deadline is in the future")
end)

case("panel_lines_and_items", function()
  local jobs = { job(lib.STATUS.RUNNING), job(lib.STATUS.DONE) }
  jobs[1].id, jobs[2].id = "job-1", "job-2"
  jobs[1].name = "build"
  jobs[1].label = "cargo build --release"
  jobs[2].output = "a\nlast line"
  local lines = lib.build_panel_lines(jobs)
  eq(#lines, 2, "one panel line per job")
  eq(lines[1][2][1], "cargo build --release", "panel line shows the command label")
  local done_text = lines[2][4][1]
  contains(done_text, "last line", "panel line carries the last output line")
  local items = lib.build_items(jobs)
  eq(#items, 2, "one picker item per job")
  eq(items[1].id, "job-1", "item identity is the job id")
  contains(items[1].label, "cargo build --release", "label carries the command")
  eq(type(items[2].detail), "table", "command and last output are detail parts")
  eq(items[2].detail[1][1], "bash", "command part comes first")
  contains(items[2].detail[2][1], "last line", "last output part comes second")
  eq(items[1].detail, "bash", "detail degrades to tool name")
end)

case("job_command_joins_header_spans", function()
  eq(lib.job_command({ tool = "bash" }), "bash", "no header falls back to tool")
  eq(
    lib.job_command({ tool = "bash", header = { { "cargo build", "plain" }, { " --release", "plain" } } }),
    "cargo build --release",
    "header span texts join as-is"
  )
  eq(lib.job_command({}), "", "empty job yields empty command")
end)

case("last_output_line", function()
  local running = job(lib.STATUS.RUNNING)
  eq(lib.last_output_line(running), nil, "no output yet")
  running.live_buf = setmetatable({ lines = { { { "partial line" } } } }, {
    __index = function(_, k)
      if k == "get_lines" then
        return function(self)
          return self.lines
        end
      end
    end,
  })
  eq(lib.last_output_line(running), "partial line", "running reads the live buf")
  local done = job(lib.STATUS.DONE)
  done.output = "a\nb"
  eq(lib.last_output_line(done), "b", "settled reads the output tail")
  done.output = ""
  eq(lib.last_output_line(done), nil, "empty output has no last line")
  done.output = string.rep("x", lib.MAX_INLINE_OUTPUT + 10)
  eq(#lib.last_output_line(done), lib.MAX_INLINE_OUTPUT, "long line capped")
  eq(lib.last_output_line(done):sub(-3), "\xE2\x80\xA6", "cap ends with ellipsis")
end)

case("job_output_lines", function()
  eq(
    lib.job_output_lines(job(lib.STATUS.RUNNING), { "live line" }, 10)[1],
    "    live line",
    "live lines render as an indented tail"
  )
  local done = job(lib.STATUS.DONE)
  done.output = "a\nb"
  contains(table.concat(lib.job_output_lines(done, nil, 10), "\n"), "b", "settled job renders its output tail")
  eq(lib.job_output_lines(job(lib.STATUS.DONE), nil, 10)[1], lib.NO_OUTPUT, "empty output uses the sentinel")
end)

case("render_status_lists_jobs_and_results", function()
  local jobs = { job(lib.STATUS.DONE), job(lib.STATUS.RUNNING), job() }
  jobs[1].id, jobs[2].id, jobs[3].id = "job-1", "job-2", "job-3"
  jobs[1].name = "build"
  jobs[1].label = "cargo build"
  jobs[1].output = "line1\nline2"
  local text = lib.render_status(jobs, { results = true, tail = 10 })
  contains(text, "queue: 1 running, 1 queued, 1 terminal", "counts header")
  contains(text, "cargo build build [done", "terminal job with name")
  contains(text, "    line2", "result tail indented")
  local plain = lib.render_status(jobs, {})
  eq(plain:find("line1", 1, true), nil, "no full tail when disabled")
  contains(plain, "    line2", "last output line still shown when tails disabled")
end)

case("tail_lines_keeps_last_lines", function()
  local tails = lib.tail_lines("a\nb\nc", 2)
  contains(tails[1], "1 earlier lines omitted", "dropped line counted")
  eq(tails[2], "    b", "kept line indented")
  eq(tails[3], "    c", "last line kept")
  eq(#lib.tail_lines("", 5), 0, "empty output no tails")
end)

case("snapshot_roundtrip", function()
  local j = job(lib.STATUS.RUNNING)
  j.id, j.name, j.tool = "job-7", "build", "bash"
  j.label = "sleep 900"
  j.output = nil
  local snap = lib.snapshot({ j })
  eq(snap.jobs[1].id, "job-7", "snapshot keeps id")
  eq(snap.jobs[1].label, "sleep 900", "snapshot keeps label")
  local rebuilt = lib.jobs_from_snapshot(snap.jobs)
  eq(rebuilt[1].status, lib.STATUS.ERROR, "non-terminal restores as interrupted error")
  contains(rebuilt[1].output, "interrupted", "non-terminal gets interruption note")
  eq(rebuilt[1].label, "sleep 900", "label survives the roundtrip")
  eq(lib.jobs_from_snapshot(lib.snapshot({ job(lib.STATUS.DONE) }).jobs)[1].status, lib.STATUS.DONE, "terminal kept")
end)

case("render_queue_lines_degrade_headers", function()
  local j = job()
  local lines = lib.render_queue_lines({ j })
  eq(#lines, 1, "one line per job")
  eq(lines[1][2][1], "bash", "label degrades to tool name")
  j.label = "cargo build"
  eq(lib.render_queue_lines({ j })[1][2][1], "cargo build", "label shown when present")
end)

case("validate_batch_rejects_whole_batch_before_registration", function()
  local ok_spec = { tool = "bash", params = { command = "echo hi" }, name = "ok-job" }
  local specs, err = lib.validate_batch({ ok_spec, { tool = "bash", timeout_seconds = 0 } }, {})
  eq(specs, nil, "no partial specs on late failure")
  contains(err, "timeout_seconds", "error names the bad entry")

  -- duplicate name within the batch
  specs, err = lib.validate_batch({ ok_spec, { tool = "bash", params = { command = "x" }, name = "ok-job" } }, {})
  eq(specs, nil, "intra-batch duplicate rejected")
  contains(err, "already in use", "duplicate error message")

  -- name colliding with a live queued job
  local queued = job(lib.STATUS.QUEUED)
  queued.name = "taken"
  specs, err = lib.validate_batch({ { tool = "bash", params = {}, name = "taken" } }, { queued })
  eq(specs, nil, "queue name collision rejected")

  -- valid batch passes through normalized
  specs = lib.validate_batch({ ok_spec, { tool = "bash", params = { command = "x" } } }, {})
  eq(#specs, 2, "valid batch returns all specs")
  eq(specs[1].name, "ok-job", "spec name kept")
end)

case("job_output_lines_caps_live_like_settled", function()
  local many = {}
  for i = 1, 12 do
    many[i] = "line" .. i
  end
  local j = { status = lib.STATUS.RUNNING, output = "" }
  local live = lib.job_output_lines(j, many, 5)
  -- Same shape as the settled tail of the same output: dropped-line counter,
  -- indented rows, last line intact.
  local settled = lib.job_output_lines({
    status = lib.STATUS.DONE,
    output = table.concat(many, "\n"),
  }, nil, 5)
  eq(#live, 6, "live capped to max_lines plus counter")
  contains(live[1], "7 earlier lines omitted", "live dropped-line counter")
  eq(live[#live], "    line12", "live keeps last line indented")
  eq(#settled, #live, "settled view same height as live view")
  for i = 1, #live do
    eq(settled[i], live[i], "settled row matches live row " .. i)
  end
end)

case("kill_job_paths", function()
  local done = job(lib.STATUS.DONE)
  contains(lib.kill_job(done), "already", "terminal job reports its status")
  eq(done.kill_requested, nil, "terminal kill sets nothing")

  local running, handle = job(lib.STATUS.RUNNING), { kills = 0 }
  function handle:kill()
    self.kills = self.kills + 1
  end
  running.kill_handle = handle
  contains(lib.kill_job(running), "killed", "running job reports the abort")
  eq(running.kill_requested, true, "running kill marks the job")
  eq(handle.kills, 1, "running kill fires the handle")

  local queued = job(lib.STATUS.QUEUED)
  contains(lib.kill_job(queued), "was queued", "queued job reports the pre-start kill")
  eq(queued.status, lib.STATUS.KILLED, "queued kill settles the job")
  eq(queued.output, lib.KILL_QUEUED_MSG, "queued kill uses the shared message")
  contains(lib.kill_job(queued), "already", "second kill sees terminal")
end)

case("live_text_strips_span_styles", function()
  local buf = {
    get_lines = function()
      return {
        { { "hello " }, { "world", "dim" } },
        { { "plain" } },
      }
    end,
  }
  eq(lib.live_text(buf), "hello world\nplain", "span texts joined per line")
end)

case("render_status_shows_live_tail_for_running_jobs", function()
  local buf = {
    get_lines = function()
      return { { { "line1" } }, { { "line2" } }, { { "line3" } } }
    end,
  }
  local running = job(lib.STATUS.RUNNING)
  running.live_buf = buf
  local text = lib.render_status({ running }, { live = true, tail = 2 })
  contains(text, "line3", "live tail keeps the last line")
  contains(text, "1 earlier lines omitted", "live tail caps with a counter")
  eq(text:find("line1", 1, true), nil, "dropped live lines stay dropped")

  local alone = lib.render_status({ running }, { results = true, tail = 2 })
  eq(alone:find("line1", 1, true), nil, "results flag alone shows no full live tail")
  contains(alone, "    line3", "results flag alone still shows the last live line")
  eq(lib.render_status({ running }, {}):find("line2", 1, true), nil, "no live tail when disabled")
  eq(
    lib.render_status({ job(lib.STATUS.RUNNING) }, { live = true }):find("line", 1, true),
    nil,
    "running job without a buf renders bare"
  )
end)

case("status_label_shows_killing_for_requested_kills", function()
  local running = job(lib.STATUS.RUNNING)
  running.started_at = os.time() - 5
  contains(lib.status_label(running), "running", "plain running label")
  running.kill_requested = true
  eq(lib.status_label(running), "killing", "kill request shows as killing")
  local killed = job(lib.STATUS.KILLED)
  killed.kill_requested = true
  contains(lib.status_label(killed), "killed", "terminal status wins over killing")
end)

if #failures > 0 then
  error(#failures .. " case(s) failed:\n\n" .. table.concat(failures, "\n\n"))
end
