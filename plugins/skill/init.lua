local SKILL_FILE = "SKILL.md"
local NOT_FOUND = "skill not found: "
local shorten_path = require("maki.shorten_path")
local ToolView = require("maki.tool_view")
local helpers = require("skill_helpers")
local parse_frontmatter = helpers.parse_frontmatter
local build_skill_list = helpers.build_skill_list

local PROJECT_SKILL_DIRS = {
  ".maki/skills",
  ".claude/skills",
  ".opencode/skills",
  ".agents/skills",
}
local GLOBAL_SKILL_DIRS = {
  ".claude/skills",
  ".config/opencode/skills",
  ".agents/skills",
}

local function scan_skill_dir(dir, skills, excluded)
  local entries = maki.fs.dir(dir)
  if not entries then
    return
  end
  for _, entry in ipairs(entries) do
    if entry[2] == "directory" then
      local folder_name = entry[1]
      if not excluded[folder_name] then
        local skill_path = maki.fs.joinpath(dir, folder_name, SKILL_FILE)
        local content = maki.fs.read(skill_path)
        if content then
          local fm, body = parse_frontmatter(content)
          if body and #body > 0 then
            local name = (fm and fm.name) or folder_name
            if not excluded[name] then
              skills[name] = {
                name = name,
                description = (fm and fm.description) or "",
                content = body,
                location = skill_path,
              }
            end
          end
        end
      end
    end
  end
end

local function find_project_ancestors()
  local cwd = maki.uv.cwd()
  if not cwd then
    return {}
  end
  local dirs = { cwd }
  for _, parent in ipairs(maki.fs.parents(cwd)) do
    dirs[#dirs + 1] = parent
    local git = maki.fs.joinpath(parent, ".git")
    if maki.fs.metadata(git) then
      break
    end
  end
  return dirs
end

local function discover_skills()
  local excluded = {}
  for _, ancestor in ipairs(find_project_ancestors()) do
    local path = maki.fs.joinpath(ancestor, ".agents", "skills.json")
    local content = maki.fs.read(path)
    if content then
      local data, err = maki.json.decode(content)
      if data and data.exclude then
        for _, item in ipairs(data.exclude) do
          excluded[item] = true
        end
      end
    end
  end

  local skills = {}

  local config = maki.env.config_dir()
  if config then
    scan_skill_dir(maki.fs.joinpath(config, "skills"), skills, excluded)
  end

  local home = maki.uv.os_homedir()
  if home then
    for _, rel in ipairs(GLOBAL_SKILL_DIRS) do
      scan_skill_dir(maki.fs.joinpath(home, rel), skills, excluded)
    end
  end

  for _, ancestor in ipairs(find_project_ancestors()) do
    for _, rel in ipairs(PROJECT_SKILL_DIRS) do
      scan_skill_dir(maki.fs.joinpath(ancestor, rel), skills, excluded)
    end
  end

  return skills
end

maki.api.register_tool({
  name = "skill",
  kind = "read",
  description = "Load a task-specific playbook by name.",

  schema = {
    type = "object",
    properties = {
      name = { type = "string", description = "Skill name; omit to list available skills" },
    },
  },

  header = function(input)
    return input.name or "list"
  end,

  handler = function(input, ctx)
    local skills = discover_skills()
    if not input.name or input.name == "" then
      return "Available skills:" .. build_skill_list(skills)
    end

    local skill = skills[input.name]
    if not skill then
      local available = build_skill_list(skills)
      return NOT_FOUND .. input.name .. available
    end

    local lines = {}
    local i = 1
    for line in (skill.content .. "\n"):gmatch("([^\n]*)\n") do
      lines[#lines + 1] = string.format("%4d | %s", i, line)
      i = i + 1
    end
    local formatted = skill.location .. "\n" .. table.concat(lines, "\n")

    local buf = maki.ui.buf()
    local tol = ctx:tool_output_lines()
    local view = ToolView.new(buf, {
      max_lines = (tol and tol.other) or 20,
      keep = "head",
    })
    buf:on("click", function()
      view:toggle()
    end)

    local ext = skill.location:match("%.([^%.]+)$") or "md"
    if not view:set_highlight(skill.content, ext) then
      for line in formatted:gmatch("([^\n]*)\n?") do
        view:append(line)
      end
    end
    view:finish()

    local short = shorten_path(skill.location)
    local header_buf = maki.ui.buf()
    header_buf:line({ { short, "path" } })

    return {
      llm_output = formatted,
      body = buf,
      header = header_buf,
    }
  end,
})

-- ── skill_test ────────────────────────────────────────────────────────────────

local function has_project_skills()
  for _, ancestor in ipairs(find_project_ancestors()) do
    for _, rel in ipairs(PROJECT_SKILL_DIRS) do
      local entries = maki.fs.dir(maki.fs.joinpath(ancestor, rel))
      if entries then
        for _, entry in ipairs(entries) do
          if entry[2] == "directory" then
            return true
          end
        end
      end
    end
  end
  return false
end

local function shell_quote(s)
  return "'" .. s:gsub("'", "'\\''") .. "'"
end

-- Runs one test case by spawning `maki --print` as a subprocess.
-- Uses maki's own credentials and model config — no separate API key needed.
local function run_one_test(maki_bin, skill_body, tc)
  local cmd = maki_bin
    .. " --print --yolo --output-format json"
    .. " --append-system-prompt " .. shell_quote(skill_body)
    .. " --no-plugins"  -- don't load plugins in the subprocess; plain LLM response
    .. " --max-turns 1"
    .. " " .. shell_quote(tc.prompt)

  local stdout_parts = {}
  local done = false
  local exit_code = 0

  local id = maki.fn.jobstart(cmd, {
    on_stdout = function(_, line)
      stdout_parts[#stdout_parts + 1] = line
    end,
    on_exit = function(_, code)
      exit_code = code
      done = true
    end,
  })

  local deadline = 60000  -- 60s per test
  local waited = maki.fn.jobwait(id, deadline)
  if not waited or not done then
    maki.fn.jobstop(id)
    return nil, "timeout after 60s"
  end

  local raw = table.concat(stdout_parts, "\n")
  local data, parse_err = maki.json.decode(raw)
  if not data then
    return nil, "parse: " .. tostring(parse_err) .. " (raw: " .. raw:sub(1, 200) .. ")"
  end

  if data.is_error then
    return nil, "maki error: " .. (data.result or "unknown")
  end

  local text = data.result or ""
  local tl   = text:lower()

  local failures = {}
  for _, pat in ipairs(tc.expect_contains or {}) do
    if not tl:find(pat:lower(), 1, true) then
      failures[#failures + 1] = "missing: " .. pat
    end
  end
  for _, pat in ipairs(tc.expect_not_contains or {}) do
    if tl:find(pat:lower(), 1, true) then
      failures[#failures + 1] = "found forbidden: " .. pat
    end
  end

  return { passed = #failures == 0, failures = failures, response = text }
end

local function find_maki_bin()
  -- Try PATH first, then common build output locations.
  local id = maki.fn.jobstart("command -v maki")
  local result = maki.fn.jobwait(id, 2000)
  if result and result.exit_code == 0 then
    local p = (result.stdout or ""):match("^%s*(.-)%s*$")
    if p ~= "" then return p end
  end
  -- Fall back to the release build next to the repo root.
  local cwd = maki.uv.cwd() or "."
  for _, ancestor in ipairs({ cwd, unpack(maki.fs.parents(cwd)) }) do
    local candidate = maki.fs.joinpath(ancestor, "target/release/maki")
    if maki.fs.metadata(candidate) then return candidate end
    local git = maki.fs.joinpath(ancestor, ".git")
    if maki.fs.metadata(git) then break end
  end
  return "maki"
end

if has_project_skills() then
maki.api.register_tool({
  name        = "skill_test",
  kind        = "fetch",
  description = "Run behavioral smoke tests defined in a skill's SKILL.md `tests:` frontmatter. Spawns a headless maki subprocess per test case, passing the skill body as system context, and checks the LLM response against expect_contains / expect_not_contains strings.",

  schema = {
    type = "object",
    properties = {
      skill = { type = "string", description = "Skill name to test", required = true },
    },
  },
  permission_scope = "skill",

  header = function(input)
    return "skill_test: " .. (input.skill or "?")
  end,

  handler = function(input, ctx)
    local skill_name = input.skill
    if not skill_name then return "error: skill is required" end

    local skills = discover_skills()
    local skill  = skills[skill_name]
    if not skill then
      return NOT_FOUND .. skill_name .. build_skill_list(skills)
    end

    local raw = maki.fs.read(skill.location)
    if not raw then return "error: cannot read " .. skill.location end
    local fm, body = parse_frontmatter(raw)

    local tests = fm and fm.tests
    if not tests or #tests == 0 then
      return "no tests defined in " .. skill.location
        .. "\n\nAdd a `tests:` array to the SKILL.md frontmatter:\n\n"
        .. "```yaml\ntests:\n  - prompt: \"...\"\n    expect_contains:\n      - \"...\"\n```"
    end

    local maki_bin = find_maki_bin()
    local results  = {}
    local n_pass   = 0

    for i, tc in ipairs(tests) do
      local res, err = run_one_test(maki_bin, body, tc)
      if err then
        results[#results + 1] = { i = i, prompt = tc.prompt or "?", err = err }
      elseif res.passed then
        n_pass = n_pass + 1
        results[#results + 1] = { i = i, prompt = tc.prompt, passed = true }
      else
        results[#results + 1] = {
          i = i, prompt = tc.prompt,
          passed = false, failures = res.failures, response = res.response,
        }
      end
    end

    local total   = #tests
    local summary = string.format("## %s: %d/%d passed\n", skill_name, n_pass, total)
    local lines   = { summary }

    for _, r in ipairs(results) do
      if r.err then
        lines[#lines + 1] = string.format("**[%d] ERROR** — %s\n`%s`\n", r.i, r.prompt, r.err)
      elseif r.passed then
        lines[#lines + 1] = string.format("**[%d] PASS** — %s\n", r.i, r.prompt)
      else
        local fl = table.concat(r.failures, "\n- ")
        lines[#lines + 1] = string.format("**[%d] FAIL** — %s\n- %s\n", r.i, r.prompt, fl)
      end
    end

    local out = table.concat(lines, "\n")

    return {
      llm_output = out,
      body       = ToolView.restore(out, { max_lines = 40, keep = "head" }),
      is_error   = (n_pass < total),
    }
  end,
})
end
