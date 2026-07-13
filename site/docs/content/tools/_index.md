+++
title = "Tools"
weight = 3
[extra]
group = "Reference"
+++

# Tools

Maki ships with 18 built-in tools. This is the full reference.

## File Operations

### `bash` *(lua plugin)*

Run git, build, test, and system commands. Not for reading or writing files. Use workdir instead of `cd &&`. Chain dependent commands with &&; use batch for independent ones. Output truncates past ~2000 lines.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `command` | string | yes |  | The bash command to execute |
| `description` | string | no |  | Short description (3-5 words) of what the command does |
| `timeout` | integer | no | 120 | Timeout in seconds |
| `workdir` | string | no | cwd | Working directory |

### `read` *(lua plugin)*

Read a file with line numbers. Give offset and limit — locate them with index or grep first, and read one adequate window rather than repeated small slices. Read multiple files in parallel.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `limit` | integer | no | Max number of lines to read. Omitting the limit reads up to 2000 lines. |
| `offset` | integer | no | Line number to start from (1-indexed) |
| `path` | string | yes | Absolute path to the file or directory |

### `write` *(lua plugin)*

Write a full file, overwriting existing content; creates parent dirs. Prefer edit/multiedit on files that already exist. Don't create README or *.md docs unless asked.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `content` | string | yes | The complete file content to write |
| `path` | string | yes | Absolute path to the file |

### `edit` *(lua plugin)*

Replace an exact string in a file. old_string must be unique (or set replace_all). Read the file first; exclude the line-number prefix from read output when copying. Cheaper than write for targeted changes.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `new_string` | string | yes |  | Replacement string |
| `old_string` | string | yes |  | Exact string to find (must match uniquely unless replace_all is true) |
| `path` | string | yes |  | Absolute path to the file |
| `replace_all` | boolean | no | false | Replace all occurrences |

### `multiedit` *(lua plugin)*

Several exact-string replacements in one file, applied in order, all-or-nothing. Read the file first. Order edits so an earlier one doesn't alter text a later one matches on.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `edits` | array | yes | Array of edit operations to apply sequentially |
| `path` | string | yes | Absolute path to the file |

### `glob` *(lua plugin)*

Find files by glob pattern (respects .gitignore), newest first. Search speculatively in parallel rather than in sequential rounds.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `path` | string | no | cwd | Directory to search in |
| `pattern` | string | no |  | Glob pattern (e.g. **/*.rs, src/**/*.ts) |

### `grep` *(lua plugin)*

Regex search over file contents (respects .gitignore). Don't quote or double-escape the pattern (`\[` not `\\[`). Multi-line auto-enables with \n, (?s), or (?m).

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `context_after` | integer | no |  | Context lines after match |
| `context_before` | integer | no |  | Context lines before match |
| `include` | string | no |  | File glob filter (e.g. *.c) |
| `limit` | integer | no |  | Max match groups to return |
| `path` | string | no | cwd | Directory to search in |
| `pattern` | string | yes |  | Regex pattern |

### `index` *(lua plugin)*

Compact skeleton of a source file — imports, types, signatures with [line numbers]. Use before read to locate the section you need. Source files and markdown only; on failure use read.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `path` | string | yes | Absolute path to the file |

## Execution & Control

### `batch`

Run independent tool calls in parallel (1–25). Not for dependent or output-filtering chains — use code_execution. Don't nest batch in batch.


| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `tool_calls` | array | yes | Array of tool calls to execute in parallel |

### `code_execution`

Run Python to chain dependent tool calls or filter their output. The same tools are async functions here: `r = await read(path='x')`. Tools return strings — parse them yourself. Concurrency via asyncio.gather. Libs: re, asyncio, sys, os, json. No imports, no network. 30s default timeout.


| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `code` | string | yes |  | Python code to execute. Tools are async functions that return strings (not objects). You MUST await every call: `result = await read(path='/file')`. Use `await asyncio.gather(...)` for concurrency. |
| `timeout` | integer | no | 30, max 300 | Timeout in seconds |

### `question` *(lua plugin)*

Ask the user to choose or clarify mid-task. Recommended option first, suffixed "(Recommended)". A "type your own" choice is added automatically — don't add a catch-all option. Set multiSelect for multiple picks.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `questions` | array | yes | List of questions to ask the user |

## Agent & Knowledge

### `task` *(lua plugin)*

Delegate a self-contained subgoal to a subagent; combine with batch to run several at once. subagent_type: research (read-only, for exploration) or general (can edit). Each starts fresh — inline all context. Ask it for a short summary with file:line refs. Its output isn't shown to the user; relay it.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `description` | string | yes | Short (3-5 words) description of the task |
| `model_tier` | string | no | weak/medium/strong — scales cost vs. reasoning depth (omit to inherit current tier) |
| `prompt` | string | yes | Detailed task prompt for the agent |
| `subagent_type` | string | no | Subagent type: "research" (read-only, default) or "general" (can modify files) |

### `todo_write` *(lua plugin)*

Track work of 3+ steps. Send the full list each time (replace-all). Update after each step. Skip for trivial tasks.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `todos` | array | yes | The updated todo list |

### `memory` *(lua plugin)*

Project-scoped scratchpad for decisions and gotchas that persist across sessions. Keep entries short and current.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `command` | string | yes | Command: view, write, delete |
| `content` | string | no | File content for 'write' |
| `path` | string | no | Relative path (e.g. 'architecture.md'). Omit to list all. |

### `skill` *(lua plugin)*

Load a task-specific playbook by name.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `name` | string | no | Skill name; omit to list available skills |

## Web

### `webfetch` *(lua plugin)*

Fetch a URL as markdown (default), text, html, or json. Best called inside code_execution with filtering to avoid dumping the whole page into context.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `format` | string | no |  | Output format: markdown (default), text, html, or json |
| `timeout` | integer | no | 30, max 120 | Timeout in seconds |
| `url` | string | yes |  | URL to fetch (http:// or https://) |

### `websearch` *(lua plugin)*

Web search (Exa) for current info, docs, or anything not in local files. Prefer specific queries.

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `num_results` | integer | no | 8 | Number of results to return |
| `query` | string | yes |  | Search query |