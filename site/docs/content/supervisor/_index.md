+++
title = "Supervisor"
weight = 32
[extra]
group = "Concepts"
+++

# Supervisor

A cheap model does most of the work well and some of it badly. The supervisor lets a stronger model check agents running on a weaker one and step in only when they go wrong. You get most of the strong model's judgment for a small share of its price.

Turn it on in `init.lua`:

```lua
maki.setup({ agent = { supervisor = true } })
```

## Who is supervised

A run is supervised when the supervisor model ranks above the run's own model. The supervisor model is `agent.supervisor_model`, or the strong tier of the worker's provider when that is unset. A session on a strong model is left alone, and its weak-tier subagents are checked.

One supervisor serves the whole session. The main agent and every subagent each get a thread of their own. All threads share one system prompt, which holds the supervisor's rules and the project's instruction files (`AGENTS.md` and the like), so the provider caches that prefix once for all of them.

## Policies

`agent.supervisor_policy` decides what the supervisor reads. It is a list of parts, and the parts combine:

| Part | What the supervisor reads |
|---|---|
| `transcript` | Compact batches of the worker's turns, every few turns. |
| `actions` | Each action tool asks the worker for a one-line `why`, so every action arrives with its explanation. |
| `report` | At the end of a run: the worker's closing claim, its actions, the diff of the working tree, and the results of `agent.supervisor_checks`. |

The default is `{ "actions", "report" }`. The supervisor never reads the worker's reasoning. It reads what the worker said it would do, what it did and why, what it says it did, and evidence the worker did not write:

```lua
maki.setup({
  agent = {
    supervisor = true,
    supervisor_policy = { "actions", "report" },
    supervisor_checks = { "cargo check -q --tests", "cargo test -q" },
  },
})
```

`{ "transcript" }` reads the worker's turns as they happen instead. It catches a wrong turn earlier and costs more input.

Lookups such as `read`, `grep` and `glob` are not actions and need no `why`. The harness removes the `why` before the tool runs, so tools and MCP servers never see it.

The diff covers the whole working tree since the run started, untracked files included. It is taken through a scratch copy of the git index, so your own staging is left alone. Subagents running at the same time share the tree, so one subagent's diff can hold another's changes. Outside a git repository the report says there is no diff.

Each check runs through `sh` in the working directory with a five-minute limit. Its exit code and the last 20 lines of its output go into the report.

## The dialogue

```
worker (weak model)                         supervisor (strong model)
───────────────────                         ─────────────────────────
reads goal + kickoff note
supervisor tool: plan, questions ────────►  reviews at once
starts working, does not wait               OK (silent)  or  NOTE ...
each action carries a why          (actions)
turn, turn, turn ... ──── batches ───────►  reviews every few turns  (transcript)
<supervisor> note lands between turns ◄───  NOTE: corrections, answers, pointers
next tool call skipped, run ends ◄────────  STOP: rare, for harm or loops
says it is done ───── report + diff ─────►  checks the claim  (report)
waits up to supervisor_exit_wait_secs       NOTE resumes the run, OK ends it
```

At the start of each run the worker is asked to send its plan of work through the `supervisor` tool, with any questions. The tool returns at once. The worker never waits for a reply, and no reply means carry on.

The supervisor answers each review with one line:

- `OK` says nothing. This is the usual answer.
- `NOTE` lands in the worker's context before its next turn, tagged with the turn it was about. It corrects a misunderstanding, answers a question, or points to an instruction file.
- `STOP` skips any tool calls still pending and ends the run. The reason stays in history, so a later "continue" has the context.

Notes and stops also show in the chat, so you see every intervention.

## The end of a run

A run that ends with the worker saying it is done goes to the supervisor at once, with the report or the last turns. This is where a claim of success that nobody checked shows up. The run waits up to `agent.supervisor_exit_wait_secs` for the answer, checks included. A `NOTE` puts the worker back to work with the note. A `STOP` ends the run as failed. Set the wait to `0` to never wait.

An answer that arrives after the run ended lands at the start of the next run of the same agent. A subagent usually runs once, so a late answer about it is lost. Give the wait enough time for your checks if subagents matter to you.

## When the supervisor is unavailable

Workers never depend on the supervisor. If it fails, the worker carries on as if it had said `OK`.

- A review that errors or takes more than two minutes counts as silence. Its input is kept and sent with the next review, so a plan is not lost.
- Three failed reviews in a row take the supervisor offline for five minutes, for the whole session. While it is offline, new runs start without the kickoff note or the `supervisor` tool. Nothing is sent, checks do not run, and a run that ends does not wait. A worker that calls the tool is told nobody is listening.
- After five minutes the next review tries again. One more failure takes it offline for another five minutes.
- A model that cannot be built at start, for example a missing API key, leaves the session unsupervised.

An unreachable supervisor costs a few failed calls and at most one exit wait per run.

## Episodes

Each supervised run appends one line to `episodes.ndjson` in the `supervisor` folder of the maki state directory. The line holds the policy, the worker and supervisor models, every review with its kind, verdict, tokens, cost and time, the diff state and check exit codes of each report, and the run's total tokens and cost.

These lines let you compare policies on real work: how often a policy catches a failing check, what it costs per run, and how often its notes come back as passing checks.

## What it costs

The supervisor pays for its input, its thinking, and a one-line answer. Several choices keep that small:

- Actions and the transcript are rendered compactly. Files the worker read shrink to a line count. Diffs, commands, errors, and the worker's own words stay legible.
- Each thread is append-only, so every review is a cache hit on the ones before it.
- Transcript reviews wait for `agent.supervisor_batch_turns` worker turns or `agent.supervisor_batch_tokens` of new transcript, whichever comes first. A message from the worker triggers a review right away.
- At most two reviews run at once across the session.
- Each run gets `agent.supervisor_max_notes` notes. Past that, only a stop gets through.

Supervisor spend is added to the run's cost. The `why` on each action adds a few output tokens to the worker's calls.

See [configuration](/docs/configuration/#agent) for every key.
