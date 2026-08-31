local M = {}

M.MARKER = "maki-cron:"
M.NAME_PATTERN = "^%a[%w%-_]*$"
M.ALIAS_SCHEDULE_PATTERN = "^@%w+$"
M.SCHEDULE_FIELD_PATTERN = "^[%d%a%*%-%,/]+$"
M.LOG_SUBDIR = "cron/logs"
M.MAX_NAME_LEN = 64

local EDITABLE_FIELDS = {
  "schedule",
  "cwd",
  "model",
  "skill",
  "prompt",
  "yolo",
  "allowed_tools",
  "log",
  "env_file",
}

local function blank(s)
  return s == nil or s == ""
end

function M.quote(s)
  return "'" .. s:gsub("'", "'\\''") .. "'"
end

function M.validate_name(name)
  if blank(name) then
    return "name is required"
  end
  if #name > M.MAX_NAME_LEN then
    return "name must be at most " .. M.MAX_NAME_LEN .. " characters"
  end
  if not name:match(M.NAME_PATTERN) then
    return "name must start with a letter and use only letters, digits, '-' or '_'"
  end
  return nil
end

function M.validate_schedule(schedule)
  if blank(schedule) then
    return "schedule is required"
  end
  if schedule:match(M.ALIAS_SCHEDULE_PATTERN) then
    return nil
  end
  local fields = {}
  for field in schedule:gmatch("%S+") do
    fields[#fields + 1] = field
  end
  if #fields ~= 5 then
    return "schedule must be 5 whitespace-separated cron fields or an @alias like @daily"
  end
  for _, field in ipairs(fields) do
    if not field:match(M.SCHEDULE_FIELD_PATTERN) then
      return "invalid cron field '" .. field .. "'"
    end
  end
  return nil
end

-- Prompt the scheduled `maki -p` run receives: skills are not slash
-- commands, so the prompt has to name the skill for the skill tool to load it.
function M.prompt_text(job)
  local text = job.prompt or ""
  if job.skill and job.skill ~= "" then
    text = "Use the " .. job.skill .. " skill" .. (text ~= "" and (". " .. text) or ".")
  end
  return text
end

function M.build_command(job, exe)
  local steps, flags = {}, {}
  if not blank(job.env_file) then
    steps[#steps + 1] = ". " .. M.quote(job.env_file)
  end
  steps[#steps + 1] = "cd " .. M.quote(job.cwd)
  flags[#flags + 1] = M.quote(exe) .. " -p"
  if job.yolo then
    flags[#flags + 1] = "--yolo"
  end
  if not blank(job.model) then
    flags[#flags + 1] = "-m " .. M.quote(job.model)
  end
  if not blank(job.allowed_tools) then
    flags[#flags + 1] = "--allowed-tools " .. M.quote(job.allowed_tools)
  end
  flags[#flags + 1] = M.quote(M.prompt_text(job))
  if not blank(job.log) then
    flags[#flags + 1] = ">> " .. M.quote(job.log) .. " 2>&1"
  end
  steps[#steps + 1] = table.concat(flags, " ")
  return table.concat(steps, " && ")
end

function M.encode_job(job)
  local payload = { name = job.name }
  for _, field in ipairs(EDITABLE_FIELDS) do
    payload[field] = job[field]
  end
  return payload
end

function M.build_line(job, exe)
  local encoded, err = maki.json.encode(M.encode_job(job))
  if err then
    return nil, "encode job: " .. err
  end
  return job.schedule .. " " .. M.build_command(job, exe) .. " # " .. M.MARKER .. job.name .. " " .. encoded
end

-- Marker payload after ` # maki-cron:`: the job name, then the JSON spec.
-- A prompt embedding the marker text itself makes the line unparseable
-- (reported as broken), so the marker is reserved.
local function marker_parts(line)
  local marker_at = line:find(" # " .. M.MARKER, 1, true)
  if not marker_at then
    return nil
  end
  local rest = line:sub(marker_at + 3 + #M.MARKER)
  local name = rest:match("^(%S+)")
  local json_at = rest:find("{", 1, true)
  return name, json_at and rest:sub(json_at)
end

-- Returns an array of { name = string, job = table, line = string } plus an
-- array of unparseable managed lines. Unmanaged lines are ignored here.
function M.parse_crontab(content)
  local jobs, broken = {}, {}
  for line in content:gmatch("[^\n]+") do
    local name, json_part = marker_parts(line)
    local decoded = name and json_part and maki.json.decode(json_part)
    if not decoded or decoded.name ~= name then
      if name then
        broken[#broken + 1] = line
      end
    else
      jobs[#jobs + 1] = { name = name, job = decoded, line = line }
    end
  end
  return jobs, broken
end

-- Drop only the managed line of {name}; every other line (managed or not)
-- survives untouched.
function M.strip_job(content, name)
  local kept = {}
  for line in content:gmatch("[^\n]+") do
    local line_name = marker_parts(line)
    if line_name ~= name then
      kept[#kept + 1] = line
    end
  end
  return table.concat(kept, "\n")
end

-- Fields not in `edits` keep their value; empty strings clear optional ones.
function M.merge_edit(existing, edits)
  local merged = M.encode_job(existing)
  merged.name = existing.name
  for _, field in ipairs(EDITABLE_FIELDS) do
    if edits[field] ~= nil then
      merged[field] = edits[field]
    end
  end
  return merged
end

function M.format_jobs(jobs, broken)
  if #jobs == 0 and #broken == 0 then
    return "No maki cronjobs in the crontab. Use cronjob add to create one."
  end
  local out = {}
  for _, entry in ipairs(jobs) do
    local job = entry.job
    out[#out + 1] = table.concat({
      entry.name .. ":",
      "  schedule: " .. (job.schedule or "?"),
      "  cwd:      " .. (job.cwd or "?"),
      "  model:    " .. (blank(job.model) and "(default)" or job.model),
      "  skill:    " .. (blank(job.skill) and "(none)" or job.skill),
      "  prompt:   " .. (blank(job.prompt) and "(none)" or job.prompt),
      "  log:      " .. (blank(job.log) and "(none)" or job.log),
    }, "\n")
  end
  for _, line in ipairs(broken) do
    out[#out + 1] = "unparseable managed line (fix or remove by hand):\n  " .. line
  end
  return table.concat(out, "\n\n")
end

function M.default_log(state_dir, name)
  return state_dir .. "/" .. M.LOG_SUBDIR .. "/" .. name .. ".log"
end

-- Crontab lines have no trailing newline; write_crontab adds the final one.
function M.append_line(content, line)
  if content == "" then
    return line
  end
  return content .. "\n" .. line
end

return M
