local lib = require("cron_lib")

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

local JOB = {
  name = "nightly-triage",
  schedule = "30 7 * * 1-5",
  cwd = "/home/user/repo",
  model = "anthropic/claude-sonnet-4-6",
  skill = nil,
  prompt = "triage open issues",
  yolo = true,
  allowed_tools = nil,
  log = "/state/cron/logs/nightly-triage.log",
  env_file = nil,
}

case("validate_name", function()
  eq(lib.validate_name("nightly-triage"), nil, "valid name")
  eq(lib.validate_name("a1_b-c"), nil, "valid mixed name")
  assert(lib.validate_name(""), "blank rejected")
  assert(lib.validate_name(nil), "nil rejected")
  assert(lib.validate_name("1abc"), "leading digit rejected")
  assert(lib.validate_name("has space"), "space rejected")
  assert(lib.validate_name("bad/slash"), "slash rejected")
  assert(lib.validate_name(string.rep("a", lib.MAX_NAME_LEN + 1)), "overlong rejected")
end)

case("validate_schedule", function()
  eq(lib.validate_schedule("30 7 * * 1-5"), nil, "five fields")
  eq(lib.validate_schedule("*/15 0,12 1-15 JAN-MON *"), nil, "rich fields")
  eq(lib.validate_schedule("@daily"), nil, "alias")
  eq(lib.validate_schedule("@reboot"), nil, "reboot alias")
  assert(lib.validate_schedule(""), "blank rejected")
  assert(lib.validate_schedule(nil), "nil rejected")
  assert(lib.validate_schedule("* * * *"), "four fields rejected")
  assert(lib.validate_schedule("* * * * * *"), "six fields rejected")
  assert(lib.validate_schedule("* * * * every day"), "natural language rejected")
  assert(lib.validate_schedule("* * * * * ; rm -rf /"), "shell metacharacters rejected")
end)

case("prompt_text", function()
  eq(lib.prompt_text({ prompt = "triage issues" }), "triage issues", "prompt only")
  eq(lib.prompt_text({ skill = "git-release" }), "Use the git-release skill.", "skill only")
  eq(
    lib.prompt_text({ skill = "git-release", prompt = "tag v1.2.3" }),
    "Use the git-release skill. tag v1.2.3",
    "skill and prompt"
  )
  eq(lib.prompt_text({}), "", "nothing")
end)

case("quote", function()
  eq(lib.quote("/home/user"), "'/home/user'", "plain path")
  eq(lib.quote("it's"), "'it'\\''s'", "embedded single quote")
end)

case("build_command", function()
  local cmd = lib.build_command(JOB, "/usr/local/bin/maki")
  contains(cmd, "cd '/home/user/repo' && '/usr/local/bin/maki' -p --yolo", "core command")
  contains(cmd, "-m 'anthropic/claude-sonnet-4-6'", "model flag")
  contains(cmd, "'triage open issues'", "prompt quoted")
  contains(cmd, ">> '/state/cron/logs/nightly-triage.log' 2>&1", "log redirect")

  local minimal = lib.build_command({ name = "j", schedule = "@daily", cwd = "/tmp", yolo = false }, "/maki")
  contains(minimal, "'/maki' -p ", "no yolo when false")
  assert(not minimal:find("--yolo", 1, true), "no yolo flag")
  assert(not minimal:find("-m ", 1, true), "no model flag")
  assert(not minimal:find(">>", 1, true), "no log redirect")

  local with_env = lib.build_command({
    name = "j",
    schedule = "@daily",
    cwd = "/tmp",
    yolo = true,
    env_file = "/home/user/.cronenv",
    allowed_tools = "read,grep",
    log = nil,
  }, "/maki")
  contains(with_env, ". '/home/user/.cronenv' && cd '/tmp'", "env sourced first")
  contains(with_env, "--allowed-tools 'read,grep'", "allowed tools")
end)

case("build_line roundtrip", function()
  local line = lib.build_line(JOB, "/maki")
  contains(line, " # maki-cron:nightly-triage {", "marker and json payload")
  assert(line:sub(1, #"30 7 * * 1-5 ") == "30 7 * * 1-5 ", "schedule first")

  local jobs, broken = lib.parse_crontab(line)
  eq(#jobs, 1, "one job parsed")
  eq(#broken, 0, "nothing broken")
  for field, value in pairs(JOB) do
    eq(jobs[1].job[field], value, "roundtrip field " .. field)
  end
  eq(jobs[1].name, "nightly-triage", "name")
end)

case("parse_crontab", function()
  local user_line = "MAILTO=me@example.com"
  local other = lib.build_line(JOB, "/maki")
  local target = lib.build_line({ name = "other-job", schedule = "@hourly", cwd = "/tmp", yolo = true }, "/maki")
  local content = table.concat({ user_line, other, target, "0 0 * * 1 leftover # maki-cron:corrupted {" }, "\n")

  local jobs, broken = lib.parse_crontab(content)
  eq(#jobs, 2, "managed jobs parsed")
  eq(#broken, 1, "corrupted line reported")
  eq(jobs[1].name, "nightly-triage", "first job")
  eq(jobs[2].name, "other-job", "second job")
  eq(jobs[2].job.schedule, "@hourly", "alias survives roundtrip")

  local nothing = lib.parse_crontab(user_line)
  eq(#nothing, 0, "user lines ignored")
  local empty = lib.parse_crontab("")
  eq(#empty, 0, "empty content ignored")
end)

case("strip_job", function()
  local user_line = "MAILTO=me@example.com"
  local other = lib.build_line(JOB, "/maki")
  local target = lib.build_line({ name = "other-job", schedule = "@hourly", cwd = "/tmp", yolo = true }, "/maki")
  local content = table.concat({ user_line, other, target }, "\n")

  local stripped = lib.strip_job(content, "nightly-triage")
  contains(stripped, user_line, "user line kept")
  contains(stripped, "maki-cron:other-job", "other managed line kept")
  assert(not stripped:find("maki-cron:nightly-triage ", 1, true), "target line dropped")

  eq(lib.strip_job("", "nightly-triage"), "", "empty content")
end)

case("merge_edit", function()
  local edits = { schedule = "@daily", model = "" }
  local merged = lib.merge_edit(JOB, edits)
  eq(merged.name, "nightly-triage", "name preserved")
  eq(merged.schedule, "@daily", "schedule replaced")
  eq(merged.model, "", "model cleared with empty string")
  eq(merged.cwd, "/home/user/repo", "untouched field kept")
  eq(merged.prompt, "triage open issues", "untouched prompt kept")
  eq(merged.yolo, true, "untouched yolo kept")
end)

case("format_jobs", function()
  contains(lib.format_jobs({}, {}), "No maki cronjobs", "empty message")

  local out = lib.format_jobs({ { name = "nightly-triage", job = JOB } }, {})
  contains(out, "nightly-triage:", "name header")
  contains(out, "30 7 * * 1-5", "schedule")
  contains(out, "anthropic/claude-sonnet-4-6", "model")
  contains(out, "triage open issues", "prompt")

  local with_defaults = lib.format_jobs({
    { name = "minimal", job = { schedule = "@daily", cwd = "/tmp" } },
  }, {})
  contains(with_defaults, "(default)", "missing model annotated")
  contains(with_defaults, "(none)", "missing skill annotated")

  local with_broken = lib.format_jobs({}, { "0 0 * * 1 leftover # maki-cron:corrupted {" })
  contains(with_broken, "unparseable", "broken lines surfaced")
end)

case("default_log", function()
  eq(lib.default_log("/state", "nightly-triage"), "/state/cron/logs/nightly-triage.log", "log path")
end)

case("append_line", function()
  eq(lib.append_line("", "new line"), "new line", "empty content")
  eq(lib.append_line("existing", "new line"), "existing\nnew line", "separator added")
end)

if #failures > 0 then
  error(#failures .. " case(s) failed:\n\n" .. table.concat(failures, "\n\n"))
end
