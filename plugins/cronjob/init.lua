local lib = require("cron_lib")

local CRONTAB_WAIT_MS = 10000
local ACTIONS = { "add", "edit", "delete", "list" }
local EDITABLE_FIELDS = { "schedule", "cwd", "model", "skill", "prompt", "yolo", "allowed_tools", "log", "env_file" }

local DESCRIPTION = [[Schedule recurring maki runs in the user's crontab. Each job fires
`maki -p` (headless build mode) in a fixed directory with a pinned model and
optional skill, and appends output to a log file.

Actions:
- add:    create a job (name, schedule, and prompt or skill required).
- edit:   change fields of an existing job by name; omitted fields keep their value.
- delete: remove a job by name.
- list:   show all maki-managed jobs.

Notes:
- schedule is a standard 5-field cron expression ("30 7 * * 1-5") or an alias (@daily, @hourly, ...).
- model is a spec like "anthropic/claude-sonnet-4-6"; omit to use the default.
- Jobs run unattended: yolo defaults to true (deny rules still apply), and gate with allowed_tools for a narrower allow list.
- cron provides almost no environment: pass env_file (sourced before the run) when the job needs API keys or PATH entries.
- The job log defaults to <state dir>/cron/logs/<name>.log.

Use list first, then add/edit/delete.]]

local VALID_ACTION = {}
for _, action in ipairs(ACTIONS) do
  VALID_ACTION[action] = true
end

local function blank(s)
  return s == nil or s == ""
end

local function validate_input(input)
  if not VALID_ACTION[input.action] then
    return "action must be one of: add, edit, delete, list"
  end
  if input.action == "list" then
    return nil
  end
  local err = lib.validate_name(input.name)
  if err then
    return err
  end
  if input.action == "add" then
    if blank(input.prompt) and blank(input.skill) then
      return "prompt or skill is required"
    end
    return lib.validate_schedule(input.schedule)
  end
  if input.action == "edit" then
    local touched = false
    for _, field in ipairs(EDITABLE_FIELDS) do
      touched = touched or input[field] ~= nil
    end
    if not touched then
      return "edit needs at least one field to change"
    end
    if input.schedule then
      return lib.validate_schedule(input.schedule)
    end
    return nil
  end
  return nil
end

local function jobwait_ok(id)
  local result = maki.fn.jobwait(id, CRONTAB_WAIT_MS)
  if type(result) ~= "table" then
    return nil, "crontab timed out after " .. (CRONTAB_WAIT_MS / 1000) .. "s"
  end
  if result.exit_code == 0 then
    return result
  end
  return nil, result.stderr or ("exit code " .. result.exit_code)
end

local function read_crontab()
  local result = maki.fn.jobwait(maki.fn.jobstart("crontab -l"), CRONTAB_WAIT_MS)
  if type(result) ~= "table" then
    return nil, "crontab -l timed out after " .. (CRONTAB_WAIT_MS / 1000) .. "s"
  end
  if result.exit_code ~= 0 then
    -- crontab exits 1 with this message when the user has no crontab yet.
    if result.stderr and result.stderr:find("no crontab", 1, true) then
      return ""
    end
    return nil, "crontab -l failed: " .. (result.stderr ~= "" and result.stderr or ("exit code " .. result.exit_code))
  end
  return result.stdout or ""
end

-- Installs {content} via `crontab <file>`; cron requires a trailing newline.
local function write_crontab(content)
  local state = maki.env.state_dir()
  if not state then
    return nil, "cannot resolve state dir"
  end
  local dir = state .. "/cron"
  local _, mkdir_err = maki.fs.mkdir(dir, { parents = true })
  if mkdir_err then
    return nil, "mkdir " .. dir .. ": " .. mkdir_err
  end
  local tmp = dir .. "/crontab.tmp"
  local _, write_err = maki.fs.write(tmp, content:gsub("\n*$", "\n"))
  if write_err then
    return nil, "write " .. tmp .. ": " .. write_err
  end
  local result, err = jobwait_ok(maki.fn.jobstart("crontab " .. lib.quote(tmp)))
  maki.fs.rm(tmp)
  if not result then
    return nil, "crontab install failed: " .. err
  end
  return true
end

-- `>>` fails when the parent directory is missing, so create it up front.
local function prepare_log_dir(log)
  local parent = maki.fs.dirname(log)
  if parent then
    maki.fs.mkdir(parent, { parents = true })
  end
end

local function build_new_job(input)
  local cwd = input.cwd or maki.uv.cwd()
  if not cwd then
    return nil, "cannot determine working directory; pass cwd"
  end
  local state = maki.env.state_dir()
  if not state then
    return nil, "cannot resolve state dir"
  end
  return {
    name = input.name,
    schedule = input.schedule,
    cwd = cwd,
    model = input.model,
    skill = input.skill,
    prompt = input.prompt,
    yolo = input.yolo ~= false,
    allowed_tools = input.allowed_tools,
    log = input.log or lib.default_log(state, input.name),
    env_file = input.env_file,
  },
    nil
end

local function known_names(jobs)
  local names = {}
  for _, entry in ipairs(jobs) do
    names[#names + 1] = entry.name
  end
  return #names > 0 and table.concat(names, ", ") or "(none)"
end

local function find_job(jobs, name)
  for _, entry in ipairs(jobs) do
    if entry.name == name then
      return entry
    end
  end
  return nil
end

local function fail(msg)
  return { llm_output = msg, is_error = true }
end

local function handle_add(input)
  local job, err = build_new_job(input)
  if not job then
    return fail(err)
  end
  local content, read_err = read_crontab()
  if not content then
    return fail(read_err)
  end
  local jobs, broken = lib.parse_crontab(content)
  if find_job(jobs, input.name) then
    return fail(
      "cronjob '"
        .. input.name
        .. "' already exists; use action=edit to change it.\nExisting jobs: "
        .. known_names(jobs)
    )
  end
  local line, build_err = lib.build_line(job, maki.uv.exepath())
  if not line then
    return fail(build_err)
  end
  prepare_log_dir(job.log)
  local installed, install_err = write_crontab(lib.append_line(lib.strip_job(content, input.name), line))
  if not installed then
    return fail("failed to install crontab: " .. (install_err or "unknown error"))
  end
  return "scheduled cronjob '"
    .. input.name
    .. "':\n"
    .. line
    .. (#broken > 0 and ("\nwarning: " .. #broken .. " unparseable maki-managed line(s) left untouched") or "")
end

local function handle_edit(input)
  local content, read_err = read_crontab()
  if not content then
    return fail(read_err)
  end
  local jobs = lib.parse_crontab(content)
  local entry = find_job(jobs, input.name)
  if not entry then
    return fail("no cronjob named '" .. input.name .. "'. Existing jobs: " .. known_names(jobs))
  end
  local merged = lib.merge_edit(entry.job, input)
  local err = lib.validate_schedule(merged.schedule)
  if err then
    return fail(err)
  end
  if blank(merged.prompt) and blank(merged.skill) then
    return fail("edit would leave the job without a prompt or skill")
  end
  local line, build_err = lib.build_line(merged, maki.uv.exepath())
  if not line then
    return fail(build_err)
  end
  prepare_log_dir(merged.log)
  local installed, install_err = write_crontab(lib.append_line(lib.strip_job(content, input.name), line))
  if not installed then
    return fail("failed to install crontab: " .. (install_err or "unknown error"))
  end
  return "updated cronjob '" .. input.name .. "':\n" .. line
end

local function handle_delete(input)
  local content, read_err = read_crontab()
  if not content then
    return fail(read_err)
  end
  local jobs = lib.parse_crontab(content)
  if not find_job(jobs, input.name) then
    return fail("no cronjob named '" .. input.name .. "'. Existing jobs: " .. known_names(jobs))
  end
  local installed, install_err = write_crontab(lib.strip_job(content, input.name))
  if not installed then
    return fail("failed to install crontab: " .. (install_err or "unknown error"))
  end
  return "deleted cronjob '" .. input.name .. "'"
end

local function handle_list()
  local content, read_err = read_crontab()
  if not content then
    return fail(read_err)
  end
  local jobs, broken = lib.parse_crontab(content)
  return lib.format_jobs(jobs, broken)
end

maki.api.register_tool({
  name = "cronjob",
  kind = "execute",
  description = DESCRIPTION,
  schema = {
    type = "object",
    required = { "action" },
    properties = {
      action = { type = "string", enum = ACTIONS, description = "add, edit, delete, or list" },
      name = { type = "string", description = "Unique job id (required for add/edit/delete)" },
      schedule = { type = "string", description = "5-field cron expression or @alias (add requires it)" },
      cwd = { type = "string", description = "Working directory the job runs in (add default: current cwd)" },
      model = { type = "string", description = "Model spec provider/model-id (add/edit; empty clears)" },
      skill = { type = "string", description = "Skill name the run should load (add/edit)" },
      prompt = { type = "string", description = "Prompt text for the run (add requires prompt or skill)" },
      yolo = { type = "boolean", description = "Run with --yolo (default true)" },
      allowed_tools = { type = "string", description = "Comma-separated tool allow list (add/edit)" },
      log = { type = "string", description = "Log file path (add/edit; default <state dir>/cron/logs/<name>.log)" },
      env_file = {
        type = "string",
        description = "Shell file sourced before the run, for PATH and API keys (add/edit)",
      },
    },
  },
  permission = "run",
  permission_scopes = function(input)
    if not input.action then
      return nil
    end
    local scope = "crontab " .. input.action
    if input.name then
      scope = scope .. " " .. input.name
    end
    return { scopes = { scope } }
  end,

  header = function(input)
    local s = "cronjob " .. (input.action or "?")
    if input.name then
      s = s .. " " .. input.name
    end
    if input.schedule then
      s = s .. " (" .. input.schedule .. ")"
    end
    return s
  end,

  handler = function(input)
    local err = validate_input(input)
    if err then
      return { llm_output = "error: " .. err, is_error = true }
    end
    if input.action == "list" then
      return handle_list()
    elseif input.action == "add" then
      return handle_add(input)
    elseif input.action == "edit" then
      return handle_edit(input)
    end
    return handle_delete(input)
  end,
})
