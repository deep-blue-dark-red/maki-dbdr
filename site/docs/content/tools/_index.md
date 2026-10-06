+++
title = "Tools"
weight = 4
[extra]
group = "Reference"
+++

# Tools

Maki ships with 26 built-in tools in this reference (25 on by default, 1 opt-in via plugin options). Tools marked **opt-in** are off until you enable them under `plugins` in [Configuration](/docs/configuration/).

## File Operations

### `bash` {#bash}

Execute a bash command.
Commands run in <cwd> by default.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `command` | string | yes |  | The bash command to execute |
| `description` | string | no |  | Short description (3-5 words) of what the command does |
| `tail` | integer | no |  | Return only the last N lines |
| `timeout` | integer | no | 120 | Timeout in seconds |
| `workdir` | string | no | cwd | Working directory |

### `list` {#list}

List one directory: alphabetically sorted names, directories first with a trailing /. Hides AGENTS.md, CLAUDE.md, and COPILOT.md. Use glob for recursive filename searches.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `path` | string | yes | Absolute path to the directory |

### `read` {#read}

Read a file range with 1-based line numbers. Supply path, offset (first line), and limit (line count). limit=0 reads to EOF, capped at 2000 lines by default. Absolute, relative, and ~/ paths are accepted.
Use outline first for unread code, then choose one adequate range. Follow truncation hints to continue. Re-read a target range after a failed edit; otherwise reuse content already shown.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `limit` | integer | yes | Max number of lines to read. Use 0 to read until end of file (capped at 2000 lines). |
| `offset` | integer | yes | Line number to start from (1-indexed). Use 1 for the first line. |
| `path` | string | yes | File path: absolute, relative, or ~/ |

### `write` {#write}

Create a necessary new file with content; creates parent directories. Use edit or multiedit for existing files, including after an edit failure. This tool overwrites existing content by default; append=true adds to the end. Create documentation only when the user requests it.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `append` | boolean | no | Add content to the end of the file instead of replacing it |
| `content` | string | yes | Complete content for a new file; when append=true, only the text to add |
| `path` | string | yes | Absolute path to the file |

### `edit` {#edit}

Replace exact text in an existing file. Copy old_string from read output without line numbers, preserving whitespace and enough context to match once. replace_all=true replaces every occurrence in this file. new_string may be empty to delete text.
For several changes in one file, use multiedit. If matching fails, re-read the target range and retry once with corrected text; do not switch to write.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `new_string` | string | yes |  | Replacement string |
| `old_string` | string | yes |  | Exact string to find (must match uniquely unless replace_all is true) |
| `path` | string | yes |  | Absolute path to the file |
| `replace_all` | boolean | no | false | Replace all occurrences |

### `multiedit` {#multiedit}

Apply several exact-text edits to one file atomically. Read the target ranges first; copy old_string without line numbers, preserving whitespace. Each must match once unless replace_all=true.
Edits run in order against the previous edit's result. If any fails, nothing is written. On failure, re-read the target range and retry once with corrected text; do not switch to write.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `edits` | array | yes | Array of edit operations to apply sequentially |
| `path` | string | yes | Absolute path to the file |

### `edit_lines` {#edit_lines}

Replace the inclusive 1-based range start..end with new_string; an empty string deletes it. Use only with current line numbers from read output; earlier edits can shift them. Prefer edit for exact-text changes. Do not call through batch.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `end` | integer | yes | Last line, inclusive |
| `new_string` | string | yes | Replacement text |
| `path` | string | yes | Absolute path to the file |
| `start` | integer | yes | First line (1-indexed) |

### `insert_lines` <span class="badge badge-optin">opt-in</span> {#insert_lines}

Insert `new_string` after line `line`, or at the top with 0. Only include new lines, never lines already in the file. Do not use with the batch tool.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `line` | integer | yes | Line number to insert after (1-indexed). Use 0 to insert at the top. |
| `new_string` | string | yes | Text to insert |
| `path` | string | yes | Absolute path to the file |

### `glob` {#glob}

Find files when you know a filename or path pattern, e.g. **/*.rs. Respects .gitignore; returns paths newest first. Use grep to search contents.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `path` | string | no | cwd | Directory to search in |
| `pattern` | string | yes |  | Glob pattern (e.g. **/*.rs, src/**/*.ts) |

### `grep` {#grep}

Search file contents for a known symbol or regex. Returns line-numbered matches grouped by file, newest files first; respects .gitignore. Narrow with path/include and bound output with limit/context_before/context_after.
Pass the regex without shell quotes; use normal JSON escaping (a literal [ is "\\[" in JSON). Multiline matching activates for \n, (?s), or (?m). Use bash with rg for counts, file-only results, or type filters.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `context_after` | integer | no |  | Context lines after match |
| `context_before` | integer | no |  | Context lines before match |
| `include` | string | no |  | File glob filter (e.g. *.c) |
| `limit` | integer | no |  | Max match groups to return |
| `path` | string | no | cwd | Directory to search in |
| `pattern` | string | yes |  | Regex pattern |

### `ast_grep` {#ast_grep}

Search code by syntax structure. Supply exactly one of `pattern` (a single complete AST node with metavariables, e.g. `fn $F() { $BODY }`) or `kind` (a tree-sitter node kind, e.g. `function_item`) — never both; if both are given, pattern wins and kind is ignored. Pattern rules: one node per call (put sequences in a construct, e.g. `if $C { $$$BODY }`); metavariables are `$NAME`/`$$$NAME` only, never rustc `$($A:tt)*`; no leading `.` fragments; parens must exist in the source (`0..$N`, not `(0..$N)`). Returns matched lines and metavariable bindings. Language is inferred from extensions unless lang is set; respects .gitignore. Use grep for text or regex. Requires ast-grep on PATH.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `include` | string | no |  | File glob filter (e.g. *.rs) |
| `kind` | string | no |  | Tree-sitter node kind (snake_case, per language): rust `function_item`, `impl_item`, `match_expression`, `closure_expression`, `macro_invocation`; ts `function_declaration`, `call_expression`. Plain words like `pointer` are rejected. Supply this or pattern, never both. |
| `lang` | string | no |  | Language name (e.g. rust, ts, tsx, python). Inferred from extensions when omitted. |
| `limit` | integer | no |  | Max matches to return |
| `path` | string | no | cwd | Directory or file to search in |
| `pattern` | string | no |  | AST snippet: exactly one node — `fn $F() { $BODY }`, `$X.lock()`, `if $C { $$$BODY }`. Metavariables `$NAME`/`$$$NAME` only; must match source text as written. |
| `strictness` | string | no |  | Pattern strictness: cst \| smart \| ast \| relaxed \| signature \| template |
| `timeout` | integer | no | 60 | Timeout in seconds |

### `ast_grep_replace` {#ast_grep_replace}

Rewrite all AST matches in the absolute file or directory path. Search first with ast_grep using the same scope and pattern/kind (exactly one of them, never both — pattern wins if both are given). Supply rewrite; it may reuse pattern metavariables such as $X.
Every match is changed: limit caps displayed matches, not edits. Returns replacements and the applied count. Requires ast-grep on PATH.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `include` | string | no |  | File glob filter (e.g. *.rs) |
| `kind` | string | no |  | Tree-sitter node kind (snake_case, per language): rust `function_item`, `impl_item`, `match_expression`, `closure_expression`, `macro_invocation`; ts `function_declaration`, `call_expression`. Plain words like `pointer` are rejected. Supply this or pattern, never both. |
| `lang` | string | no |  | Language name (e.g. rust, ts, tsx, python). Inferred from extensions when omitted. |
| `limit` | integer | no |  | Max matches to show |
| `path` | string | yes |  | Absolute path to the file or directory to rewrite |
| `pattern` | string | no |  | AST snippet: exactly one node — `fn $F() { $BODY }`, `$X.lock()`, `if $C { $$$BODY }`. Metavariables `$NAME`/`$$$NAME` only; must match source text as written. |
| `rewrite` | string | yes |  | Replacement snippet, may use the pattern's metavariables |
| `strictness` | string | no |  | Pattern strictness: cst \| smart \| ast \| relaxed \| signature \| template |
| `timeout` | integer | no | 60 | Timeout in seconds |

### `outline` {#outline}

Return a file outline: imports, types, and function signatures with [line numbers]. Call once before reading an unread code file, then use read for the needed range. Supports source code and Markdown; if the language is unsupported, use read.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `path` | string | yes | Absolute path to the file |

### `view_image` {#view_image}

View an image file (png, jpeg, gif, webp) so you can actually see it; it is returned as vision input alongside the tool result. Use instead of `read` for images.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `path` | string | yes | Path to the image file |

### `create_plugin` {#create_plugin}

Scaffold a personal plugin in the maki config directory. Creates lua/<name>.lua (and plugin.toml if missing), then reports the remaining wiring steps; loaded by /reload, no rebuild.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `description` | string | no | What the scaffolded tool should do, in one sentence |
| `name` | string | yes | New plugin name: lowercase letters, digits and underscores, starting with a letter |
| `path` | string | no | Config directory to scaffold into (a .maki directory); defaults to the global config dir (~/.maki) |

## Execution & Control

### `async` {#async}

Queue independent tool calls in the background; spawn returns job ids immediately. Use batch when you want to wait for all calls, task for an autonomous subagent.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `action` | string | no |  | One of "spawn" (default), "status", "wait", "cancel" |
| `job_ids` | array | no | all unfinished jobs | wait/cancel: job ids or names |
| `jobs` | array | no |  | spawn: jobs to queue, each { tool, parameters, name?, timeout_seconds? } or flat { tool, ...params } |
| `timeout_seconds` | integer | no | 300; 0 returns immediately | wait: seconds to block before returning current statuses |
| `workers` | integer | no |  | spawn: concurrent jobs for this spawn call, clamped to the plugin's workers option |

### `batch` {#batch}

Run 1-25 independent tool calls in parallel and wait for all results. Each item: {tool: name, parameters: arguments}. Calls must not depend on each other's results or modify the same file. Use code_execution for dependencies or output filtering, async to keep working while tools run. Do not nest batch.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `tool_calls` | array | yes | Array of tool calls to execute in parallel |

### `code_execution` {#code_execution}

Run sandboxed Python to chain tool calls or filter output before it reaches the conversation. Runs on monty, a restricted interpreter — not full CPython: f-strings are the only string formatting (no `%` operator, no str.format()); generator expressions materialize to lists. Await every call with keyword arguments, e.g. `r = await read(path='/project/file.py', offset=10, limit=40)`. Tools return strings; parse as needed and print only useful results. For concurrency, use `await gather(outline(path='/project/a.py'), grep(pattern='TODO'))` with direct tool calls, not async def wrappers; inspect each result for errors. Bundled stdlib only: re, asyncio, sys, os, json, collections, math, itertools, datetime, pathlib, typing, dataclasses, unicodedata — anything else fails to import; no direct network access. open() supports text files; follow the same read/edit/write rules as direct calls. Default execution budget: 30s, excluding time waiting for tools.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `code` | string | yes |  | Python script. Await tools, use their required arguments, and print the results you need to see. |
| `timeout` | integer | no | 30 | Execution budget in seconds, excluding tool waits |

### `question` {#question}

Ask the user for missing requirements or a decision needed to proceed. Group related questions in one call. Put the recommended option first, with "(Recommended)" in its label. Free-text answers are available by default; omit catch-all options. Set multiSelect=true only for multiple choices. Returns selected labels per question.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `questions` | array | yes | List of questions to ask the user |

## Agent & Knowledge

### `task` {#task}

Delegate a self-contained subgoal to a new agent. Use research (default) for read-only exploration or general for implementation. Include the objective, relevant context, file scope, and expected result in prompt; each call starts fresh.
Use batch for independent subgoals; give implementation agents separate file ownership. Request a concise result with file:line references. The user does not see the result directly; summarize relevant findings. Use async for background tool calls.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `description` | string | yes | Short (3-5 words) description of the task |
| `model_tier` | string | no | Model tier: strong for complex reasoning, medium for implementation, weak for simple search or edits. Omit to inherit; capped at the current tier. |
| `output_schema` | string | no | JSON Schema (object) the subagent's final result must match. When set, the result is returned as a validated JSON string. |
| `prompt` | string | yes | Detailed task prompt for the agent |
| `subagent_type` | string | no | Subagent type: "research" (read-only, default) or "general" (can modify files) |
| `thinking` | string | no | Thinking: off\|adaptive\|minimal\|low\|medium\|high\|xhigh\|max\|int budget. Omit to inherit parent; capped at parent. |

### `todo_write` {#todo_write}

Track work with 3+ steps. Create the list before starting and update after each completed step. Every call replaces the entire list: include all items and their current statuses. Before finishing, mark each completed or cancelled. Skip for trivial tasks.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `todos` | array | yes | The updated todo list |

### `memory` {#memory}

Save and retrieve concise project facts across sessions. Reuse relevant tags from the system prompt. Keep notes current; update or delete stale facts. list/read return the notes directory; use edit on <dir>/<path> for targeted updates.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `command` | string | yes | Action name only: list, read, write, delete, or move. Pass arguments in separate fields. list: optional tags, returns index. read: path or tags, returns bodies. write: path and content, optional tags, creates or overwrites. delete: path. move: path and new_path, renames a note; never overwrites. |
| `content` | string | no | Body for write (frontmatter added automatically). |
| `new_path` | string | no | Target file name for move (notes are flat; must not exist). |
| `path` | string | no | Relative path, e.g. 'architecture.md'. |
| `tags` | array | no | snake_case tags. Filter for list/read; assigned on write (defaults to filename stem). |

### `skill` {#skill}

Load instructions for a task-specific skill. Pass name to load it; omit name to list available skills.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `name` | string | no | Skill name; omit to list available skills |

### `skill_test` {#skill_test}

Test a skill using its SKILL.md tests frontmatter. Runs one headless maki subprocess per case and checks the response against expect_contains / expect_not_contains. Returns pass/fail details and failure output. Each case defaults to 60s; timeout_ms overrides it.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `skill` | string | yes | Skill name to test |

## Web

### `webfetch` {#webfetch}

Fetch a known URL as markdown (default), text, html, or json. HTTP is upgraded to HTTPS. Maximum response: 5MB; timeout up to 120s (default 30s). For large pages, call from code_execution and print only relevant sections. Use websearch to find URLs.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `format` | string | no |  | Output format: markdown (default), text, html, or json |
| `timeout` | integer | no | 30, max 120 | Timeout in seconds |
| `url` | string | yes |  | URL to fetch (http:// or https://) |

### `websearch` {#websearch}

Search the web for current information or external documentation using Exa AI.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `num_results` | integer | no | 8 | Number of results to return |
| `query` | string | yes |  | Search query |