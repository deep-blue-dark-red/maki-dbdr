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
  return { status = status or lib.STATUS.QUEUED, tool = "bash" }
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

case("render_status_lists_jobs_and_results", function()
  local jobs = { job(lib.STATUS.DONE), job(lib.STATUS.RUNNING), job() }
  jobs[1].id, jobs[2].id, jobs[3].id = "job-1", "job-2", "job-3"
  jobs[1].name = "build"
  jobs[1].output = "line1\nline2"
  local text = lib.render_status(jobs, { results = true, tail = 10 })
  contains(text, "queue: 1 running, 1 queued, 1 terminal", "counts header")
  contains(text, "job-1 build [done", "terminal job with name")
  contains(text, "    line2", "result tail indented")
  eq(lib.render_status(jobs, {}):find("line2", 1, true), nil, "no results when disabled")
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
  j.output = nil
  local snap = lib.snapshot({ j })
  eq(snap.jobs[1].id, "job-7", "snapshot keeps id")
  local rebuilt = lib.jobs_from_snapshot(snap.jobs)
  eq(rebuilt[1].status, lib.STATUS.ERROR, "non-terminal restores as interrupted error")
  contains(rebuilt[1].output, "interrupted", "non-terminal gets interruption note")
  eq(lib.jobs_from_snapshot(lib.snapshot({ job(lib.STATUS.DONE) }).jobs)[1].status, lib.STATUS.DONE, "terminal kept")
end)

case("render_queue_lines_degrade_headers", function()
  local j = job()
  j.id = "job-1"
  local lines = lib.render_queue_lines({ j })
  eq(#lines, 1, "one line per job")
  eq(lines[1][2][1], "job-1", "id span present")
  eq(lines[1][3][1], "bash", "header degrades to tool name")
  j.header = { { "cargo build", "plain" } }
  eq(lib.render_queue_lines({ j })[1][3][1], "cargo build", "header spans used when present")
end)

if #failures > 0 then
  error(#failures .. " case(s) failed:\n\n" .. table.concat(failures, "\n\n"))
end
