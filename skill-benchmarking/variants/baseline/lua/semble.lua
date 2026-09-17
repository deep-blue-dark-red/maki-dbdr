local truncate = require("maki.truncate")
local ToolView = require("maki.tool_view")

local DEFAULT_TOP_K = 5

if maki.fn.executable("semble") == 0 then
  return
end

local function semble_view_opts(ctx)
  local tol = ctx:tool_output_lines()
  return { max_lines = (tol and tol.other) or 5, keep = "head" }
end

local function shell_quote(s)
  return "'" .. s:gsub("'", "'\\''") .. "'"
end

maki.api.register_prompt_hint({
  slot = "tool_usage",
  prompt = { "research", "general" },
  content = '- Use **semble** for "How does X work?" questions or when you don\'t know the right names. Use **grep** for known symbols.',
})

maki.api.register_prompt_hint({
  slot = "efficient_tools",
  content = "semble",
})

maki.api.register_tool({
  name = "semble",
  description = [[Search code semantically using semble.

- Finds code by meaning, not keywords. Best when you don't know the exact names.
- Returns chunks with file:line and relevance scores.
- Use for: "How does X work?", "Where is X implemented?", cross-cutting concerns.
- Use grep instead for known symbols or exhaustive reference searches.]],

  schema = {
    type = "object",
    properties = {
      query = { type = "string", description = "Natural language or symbol query", required = true },
      path = { type = "string", description = "Directory to search" },
      top_k = { type = "integer", description = "Number of results (default " .. DEFAULT_TOP_K .. ")" },
      content = { type = "string", description = "code (default), docs, config, or all" },
      ignore_paths = { type = "array", items = { type = "string" }, description = "Glob patterns to exclude (e.g. ['target/', '*.pb.go'])" },
    },
  },
  header = function(input)
    local buf = maki.ui.buf()
    local spans = { { input.query or "", "tool" } }
    if input.path then
      spans[#spans + 1] = { " in ", "dim" }
      spans[#spans + 1] = { input.path, "path" }
    end
    buf:line(spans)
    return buf
  end,

  restore = function(_input, output, _is_error, ctx)
    return ToolView.restore(output, semble_view_opts(ctx))
  end,

  handler = function(input, ctx)
    local query = input.query
    if not query then
      return "error: query is required"
    end

    local config = ctx:config()
    local max_lines = (config and config.max_output_lines) or 2000
    local max_bytes = (config and config.max_output_bytes) or (50 * 1024)

    local top_k = input.top_k or DEFAULT_TOP_K
    local path = input.path or maki.uv.cwd() or "."
    local ignore_paths = input.ignore_paths
    local sembleignore = path .. "/.sembleignore"

    if ignore_paths and #ignore_paths > 0 then
      local pat_lines = ""
      for _, p in ipairs(ignore_paths) do
        pat_lines = pat_lines .. p .. "\\n"
      end
      local write_id = maki.fn.jobstart(
        "printf '\\n#__maki_semble\\n" .. pat_lines .. "#__maki_semble_end\\n' >> " .. shell_quote(sembleignore)
      )
      maki.fn.jobwait(write_id, 5000)
    end

    local cmd = "semble search " .. shell_quote(query) .. " " .. shell_quote(path)
      .. " --top-k " .. tostring(top_k)

    if input.content then
      cmd = cmd .. " --content " .. shell_quote(input.content)
    end

    local buf, view
    do
      local b = maki.ui.buf()
      local v = ToolView.new(b, semble_view_opts(ctx))
      v:append({ { "Searching...", "dim" } })
      buf, view = b, v
      b:on("click", function()
        v:toggle()
      end)
    end

    local stdout_parts = {}
    local stderr_parts = {}

    maki.fn.jobstart(cmd, {
      on_stdout = function(_, line)
        stdout_parts[#stdout_parts + 1] = line
      end,
      on_stderr = function(_, line)
        stderr_parts[#stderr_parts + 1] = line
      end,
      on_exit = function(_, code)
        if ignore_paths and #ignore_paths > 0 then
          maki.fn.jobstart(
            "sed -i '' '/^#__maki_semble$/,/^#__maki_semble_end$/d' "
              .. shell_quote(sembleignore)
              .. " 2>/dev/null; grep -q '[^[:space:]]' "
              .. shell_quote(sembleignore)
              .. " 2>/dev/null || rm -f "
              .. shell_quote(sembleignore)
          )
        end
        view:clear()
        local is_error = code ~= 0
        local output, llm_output

        if is_error then
          local err = table.concat(stderr_parts, "\n")
          output = err ~= "" and err or ("semble exited with code " .. code)
          view:append({ { output, "dim" } })
          llm_output = output
        else
          local raw = table.concat(stdout_parts, "\n")
          local data, decode_err = maki.json.decode(raw)
          if not data then
            output = "error parsing semble output: " .. tostring(decode_err)
            view:append({ { output, "dim" } })
            llm_output = output
          elseif data.error then
            output = data.error
            view:append({ { output, "dim" } })
            llm_output = output
          else
            local parts = {}
            for _, r in ipairs(data.results or {}) do
              view:append({ { r.file_path, "path" }, { ":" .. r.start_line .. "-" .. r.end_line, "line_nr" } })
              if r.content and r.content ~= "" then
                for line in (r.content .. "\n"):gmatch("([^\n]*)\n") do
                  view:append(line)
                end
                view:append("")
              end
              parts[#parts + 1] = r.file_path .. ":" .. r.start_line .. "-" .. r.end_line
              if r.content and r.content ~= "" then
                parts[#parts + 1] = r.content
              end
              parts[#parts + 1] = ""
            end
            output = table.concat(parts, "\n")
            llm_output = truncate(output, max_lines, max_bytes)
          end
        end

        view:finish()
        ctx:finish({ llm_output = llm_output, is_error = is_error, body = buf })
      end,
    })

    return nil
  end,
})
