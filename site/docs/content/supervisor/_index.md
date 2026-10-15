+++
title = "Supervisor"
weight = 32
[extra]
group = "Concepts"
+++

# Supervisor

A cheap model does most of the work well and some of it badly. The supervisor lets a stronger model watch agents running on a weaker one and step in only when they go wrong. You get most of the strong model's judgment for a small share of its price.

Turn it on in `init.lua`:

```lua
maki.setup({ agent = { supervisor = true } })
```

## Who is supervised

A run is supervised when the supervisor model ranks above the run's own model. The supervisor model is `agent.supervisor_model`, or the strong tier of the worker's provider when that is unset. A session on a strong model is left alone, and its weak-tier subagents are watched.

One supervisor serves the whole session. The main agent and every subagent each get a thread of their own. All threads share one system prompt, which holds the supervisor's rules and the project's instruction files (`AGENTS.md` and the like), so the provider caches that prefix once for all of them.

## The dialogue

```
worker (weak model)                         supervisor (strong model)
───────────────────                         ─────────────────────────
reads goal + kickoff note
supervisor tool: plan, questions ────────►  reviews at once
starts working, does not wait               OK (silent)  or  NOTE ...
turn, turn, turn ... ──── batches ───────►  reviews every few turns
<supervisor> note lands between turns ◄───  NOTE: corrections, answers, pointers
next tool call skipped, run ends ◄────────  STOP: rare, for harm or loops
```

At the start of each run the worker is asked to send a short plan of work through the `supervisor` tool, with any questions. The tool returns at once. The worker never waits for a reply, and no reply means carry on.

The supervisor reads batches of the transcript after the fact and answers with one line:

- `OK` says nothing. This is the usual answer.
- `NOTE` lands in the worker's context before its next turn, tagged with the turn it was about. It corrects a misunderstanding, answers a question, or points to an instruction file.
- `STOP` skips any tool calls still pending and ends the run. The reason stays in history, so a later "continue" has the context.

Notes and stops also show in the chat, so you see every intervention.

## What it costs

The supervisor pays for its input, its thinking, and a one-line answer. Several choices keep that small:

- The transcript is rendered compactly before the supervisor reads it. Files the worker read shrink to a line count. Diffs, commands, errors, and the worker's own words stay legible.
- Each thread is append-only, so every review is a cache hit on the ones before it.
- Reviews wait for `agent.supervisor_batch_turns` worker turns or `agent.supervisor_batch_tokens` of new transcript, whichever comes first. A message from the worker triggers a review right away.
- At most two reviews run at once across the session.
- Each run gets `agent.supervisor_max_notes` notes. Past that, only a stop gets through.

Supervisor spend is added to the run's cost. A review that fails is logged and counts as silence. The worker carries on unsupervised until the next batch.

See [configuration](/docs/configuration/#agent) for every key.
