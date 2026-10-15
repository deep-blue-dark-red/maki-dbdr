+++
title = "Tools"
weight = 4
[extra]
group = "Reference"
+++

# Tools

Maki ships with 27 built-in tools in this reference (26 on by default, 1 opt-in via plugin options). Tools marked **opt-in** are off until you enable them under `plugins` in [Configuration](/docs/configuration/).

## File Operations

### `bash` {#bash}

Execute a non-interactive bash command. Runs in <cwd> by default; set workdir instead of cd. Join dependent commands with &&. Use tail=N instead of piping to tail, so live output is kept. Output is capped at 2000 lines or 50KB by default. Do not use shell output to talk to the user.
Output above 398B is compressed before it reaches you: `line [xN]` stands for N lines that differed only in volatile values (timestamps, uuids, epoch runs, temp paths); a run of uniform JSON objects becomes a `<rec cols>` header and TSV rows; a random-looking run of 48+ chars (base64/hex tokens, PEM bodies, digests) becomes `[masked r=<hash> n=<len>]`; spans repeated across lines (a shared path or log prefix) move into a lead legend referenced as `<n>`. Paths, versions, error codes, short numbers and code-looking lines (indented, or ending `;`/`{`/`}`) stay verbatim.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `command` | string | yes |  | The bash command to execute |
| `description` | string | no |  | Short description (3-5 words) of what the command does |
| `tail` | integer | no |  | Return only the last N lines |
| `timeout` | integer | no | 120 | Timeout in seconds |
| `workdir` | string | no | cwd | Working directory |

### `list` {#list}

List one directory: names sorted alphabetically, directories first with a trailing /. Hides AGENTS.md, CLAUDE.md, and COPILOT.md.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `path` | string | yes | Absolute path to the directory |

### `read` {#read}

Read a file range with 1-based line numbers: path, offset (first line), and limit (line count; 0 reads to EOF, capped at 2000 lines by default). Absolute, relative, and ~/ paths are accepted. Follow truncation hints to continue.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `limit` | integer | yes | Max number of lines to read. Use 0 to read until end of file (capped at 2000 lines). |
| `offset` | integer | yes | Line number to start from (1-indexed). Use 1 for the first line. |
| `path` | string | yes | File path: absolute, relative, or ~/ |

### `write` {#write}

Create a new file with content; creates parent directories. Overwrites an existing file unless append=true.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `append` | boolean | no | Add content to the end of the file instead of replacing it |
| `content` | string | yes | Complete content for a new file; when append=true, only the text to add |
| `path` | string | yes | Absolute path to the file |

### `edit` {#edit}

Replace exact text in an existing file. Copy old_string from read output without line numbers, preserving whitespace, with enough context to match once; replace_all=true replaces every occurrence. An empty new_string deletes. If matching fails, re-read the target range and retry once.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `new_string` | string | yes |  | Replacement string |
| `old_string` | string | yes |  | Exact string to find (must match uniquely unless replace_all is true) |
| `path` | string | yes |  | Absolute path to the file |
| `replace_all` | boolean | no | false | Replace all occurrences |

### `multiedit` {#multiedit}

Apply several exact-text edits to one file atomically: they run in order, each against the previous result, and if any fails nothing is written. Copy each old_string from read output without line numbers, preserving whitespace; each must match once unless replace_all=true. On failure, re-read the target range and retry once.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `edits` | array | yes | Array of edit operations to apply sequentially |
| `path` | string | yes | Absolute path to the file |

### `edit_lines` {#edit_lines}

Replace the inclusive 1-based range start..end with new_string; an empty string deletes it. Line numbers must be current: earlier edits shift them. Prefer edit for exact-text changes. Do not call through batch.

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

Find files by name or path pattern, e.g. **/*.rs. Respects .gitignore; returns paths newest first.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `path` | string | no | cwd | Directory to search in |
| `pattern` | string | yes |  | Glob pattern (e.g. **/*.rs, src/**/*.ts) |

### `grep` {#grep}

Search file contents by regex. Returns line-numbered matches grouped by file, newest files first; respects .gitignore. Pass the regex without shell quotes, with normal JSON escaping (a literal [ is "\\[" in JSON). Multiline matching activates for \n, (?s), or (?m).

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `context_after` | integer | no |  | Context lines after match |
| `context_before` | integer | no |  | Context lines before match |
| `include` | string | no |  | File glob filter (e.g. *.c) |
| `limit` | integer | no |  | Max match groups to return |
| `path` | string | no | cwd | Directory to search in |
| `pattern` | string | yes |  | Regex pattern |

### `ast_grep` {#ast_grep}

Search code by syntax structure. Supply exactly one of `pattern` (a single complete AST node with metavariables, e.g. `fn $F() { $BODY }`) or `kind` (a tree-sitter node kind, e.g. `function_item`) — never both; if both are given, pattern wins and kind is ignored. Pattern rules: one complete node per call — a truncated node (e.g. Python `def $F($$$ARGS):` with no body) or a multi-node sequence matches nothing without warning (put sequences in a construct, e.g. `if $C { $$$BODY }`); metavariables are `$NAME`/`$$$NAME` only, never rustc `$($A:tt)*`; no leading `.` fragments; parens match literally — `$A..$B` also matches a parenthesized `(0..n)`, while a parenthesized pattern requires source parens. Returns matched lines and metavariable bindings. Language is inferred from extensions unless lang is set; respects .gitignore. Requires ast-grep on PATH.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `include` | string | no |  | File glob filter (e.g. *.rs) |
| `kind` | string | no |  | Tree-sitter node kind (snake_case, per language): rust `function_item`, `impl_item`, `match_expression`, `closure_expression`, `macro_invocation`; ts `function_declaration`, `call_expression`. A word that is not a real node kind (e.g. `pointer`) just matches nothing. Supply this or pattern, never both. |
| `lang` | string | no |  | Language name (e.g. rust, ts, tsx, python). Inferred from extensions when omitted. |
| `limit` | integer | no |  | Max matches to return |
| `path` | string | no | cwd | Directory or file to search in |
| `pattern` | string | no |  | AST snippet: exactly one complete node — `fn $F() { $BODY }`, `$X.lock()`, `if $C { $$$BODY }`; a truncated node (e.g. Python `def $F($$$ARGS):` with no body) matches nothing. Metavariables `$NAME`/`$$$NAME` only; must match source text as written. |
| `strictness` | string | no |  | Pattern strictness: cst \| smart \| ast \| relaxed \| signature \| template |
| `timeout` | integer | no | 60 | Timeout in seconds |

### `ast_grep_replace` {#ast_grep_replace}

Rewrite every AST match under an absolute file or directory path. Takes exactly one of pattern/kind, as ast_grep does (pattern wins if both are given), and a rewrite that may reuse metavariables such as $X. Run the same scope and pattern with ast_grep first: every match is changed, and limit caps only the matches displayed. Returns the replacements and the applied count. Requires ast-grep on PATH.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `include` | string | no |  | File glob filter (e.g. *.rs) |
| `kind` | string | no |  | Tree-sitter node kind (snake_case, per language): rust `function_item`, `impl_item`, `match_expression`, `closure_expression`, `macro_invocation`; ts `function_declaration`, `call_expression`. A word that is not a real node kind (e.g. `pointer`) just matches nothing. Supply this or pattern, never both. |
| `lang` | string | no |  | Language name (e.g. rust, ts, tsx, python). Inferred from extensions when omitted. |
| `limit` | integer | no |  | Max matches to show |
| `path` | string | yes |  | Absolute path to the file or directory to rewrite |
| `pattern` | string | no |  | AST snippet: exactly one complete node — `fn $F() { $BODY }`, `$X.lock()`, `if $C { $$$BODY }`; a truncated node (e.g. Python `def $F($$$ARGS):` with no body) matches nothing. Metavariables `$NAME`/`$$$NAME` only; must match source text as written. |
| `rewrite` | string | yes |  | Replacement snippet, may use the pattern's metavariables |
| `strictness` | string | no |  | Pattern strictness: cst \| smart \| ast \| relaxed \| signature \| template |
| `timeout` | integer | no | 60 | Timeout in seconds |

### `outline` {#outline}

Return a file outline: imports, types, and function signatures with [line numbers]. Supports source code and Markdown; for other files use read.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `path` | string | yes | Absolute path to the file |

### `view_image` {#view_image}

View an image file (png, jpeg, gif, webp) as vision input; read cannot show images. Accepts absolute, relative, and ~/ paths. Oversized images are downscaled; animated gif/webp keep only the first frame.

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

Run tool calls as background jobs; spawn returns job ids at once.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `action` | string | no |  | One of "spawn" (default), "status", "wait", "cancel" |
| `job_ids` | array | no | all unfinished jobs | wait/cancel: job ids or names |
| `jobs` | array | no |  | spawn: jobs to queue, each { tool, parameters, name?, timeout_seconds? } or flat { tool, ...params } |
| `timeout_seconds` | integer | no | 300; 0 returns immediately | wait: seconds to block before returning current statuses |
| `workers` | integer | no |  | spawn: concurrent jobs for this spawn call, clamped to the plugin's workers option |

### `batch` {#batch}

Run 1-25 independent tool calls in parallel and wait for all results. Each item: {tool: name, parameters: arguments}. Calls must not depend on each other's results or modify the same file. Do not nest batch.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `tool_calls` | array | yes | Array of tool calls to execute in parallel |

### `code_execution` {#code_execution}

Run restricted Python that calls maki tools; only printed output reaches the conversation. The interpreter is monty, not CPython: f-strings are the only string formatting (no `%` operator, no str.format()); generator expressions materialize to lists. Await every call with keyword arguments, e.g. `r = await read(path='/project/file.py', offset=10, limit=40)`. Tools return strings. For concurrency, use `await gather(outline(path='/project/a.py'), grep(pattern='TODO'))` with direct tool calls, not async def wrappers; check each result for errors. Stdlib is limited to re, asyncio, sys, os, json, collections, math, itertools, datetime, pathlib, typing, dataclasses, unicodedata; no network access. open() reads and writes text files under the same rules as the read and write tools. Budget: 30s by default, not counting tool waits.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `code` | string | yes |  | Python script. Await tools, use their required arguments, and print the results you need to see. |
| `timeout` | integer | no | 30 | Execution budget in seconds, excluding tool waits |

### `question` {#question}

Ask the user for missing requirements or a decision needed to proceed. Group related questions in one call. Put the recommended option first, labelled "(Recommended)". Free-text answers are always available, so omit catch-all options. Set multiSelect=true only when several choices can apply. Returns the selected labels per question.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `questions` | array | yes | List of questions to ask the user |

## Agent & Knowledge

### `task` {#task}

Delegate a self-contained subgoal to a new agent that starts with no context: put the objective, relevant context, file scope, and expected result in prompt. research (default) is read-only; general can implement. Run independent subgoals in one batch, giving implementation agents separate files. Ask for a concise result with file:line references; the user does not see it, so summarize what matters.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `description` | string | yes | Short (3-5 words) description of the task |
| `model_tier` | string | no | Model tier: strong for complex reasoning, medium for implementation, weak for simple search or edits. Omit to inherit; capped at the current tier. |
| `output_schema` | string | no | JSON Schema (object) the subagent's final result must match. When set, the result is returned as a validated JSON string. |
| `prompt` | string | yes | Detailed task prompt for the agent |
| `subagent_type` | string | no | Subagent type: "research" (read-only, default) or "general" (can modify files) |
| `thinking` | string | no | Thinking: off\|adaptive\|minimal\|low\|medium\|high\|xhigh\|max\|int budget. Omit to inherit parent; capped at parent. |

### `todo_write` {#todo_write}

Track work of 3+ steps. Every call replaces the whole list, so include every item with its current status. Create the list before starting, update it after each completed step, and mark every item completed or cancelled before finishing.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `todos` | array | yes | The updated todo list |

### `memory` {#memory}

Save and retrieve concise project facts across sessions. Reuse relevant tags from the system prompt. Update or delete stale facts. list/read return the notes directory; use edit on <dir>/<path> for targeted updates.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `command` | string | yes | Action name only: list, read, write, or delete. Pass arguments in separate fields. list: optional tags, returns index. read: path or tags, returns bodies. write: path and content, optional tags, creates or overwrites. delete: path. |
| `content` | string | no | Body for write (frontmatter added automatically). |
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

### `tool_help` {#tool_help}

Show a tool's full description and argument schema. An unknown name returns the list of available tools.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `name` | string | yes | Tool name to look up |

## Web

### `webfetch` {#webfetch}

Fetch a URL as markdown (default), text, html, or json. HTTP is upgraded to HTTPS. Maximum response 5MB; timeout up to 120s (default 30s). For large pages, call it from code_execution and print only the relevant sections.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `format` | string | no |  | Output format: markdown (default), text, html, or json |
| `timeout` | integer | no | 30, max 120 | Timeout in seconds |
| `url` | string | yes |  | URL to fetch (http:// or https://) |

### `websearch` {#websearch}

Search the web using Exa AI. Today's date is YYYY-MM-DD. Returns titles, URLs, and snippets.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `num_results` | integer | no | 8 | Number of results to return |
| `query` | string | yes |  | Search query |