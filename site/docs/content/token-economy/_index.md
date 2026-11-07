+++
title = "Token Economy"
weight = 30
[extra]
group = "Concepts"
+++

# Token Economy

Maki's whole design falls out of one fact about agent loops: the conversation is re-sent to the model on every turn.

```
turn 1   [system + prompt]                      ─► model ─► tool call
turn 2   [system + prompt + result 1]           ─► model ─► tool call
turn 3   [system + prompt + result 1 + 2]       ─► model ─► ...
```

A tool result does not cost its tokens once. It costs them again on every turn until the session ends or history is compacted. `cat` a 2000-line file on turn 2 of a 40-turn session and you pay for it 38 more times. Prompt caching softens the price, not the principle: cache reads still cost, and a bloated context also makes models measurably dumber.

So Maki attacks the two multipliers: how much each step adds to context, and how many steps there are.

## Smaller results

**outline instead of read.** The `outline` tool returns a tree-sitter skeleton of a source file: imports, types, signatures, line numbers. Usually 70-90% smaller than the file itself. The agent outlines first, then reads only the ranges it needs.

```
read main.rs                 outline main.rs
────────────                 ─────────────────────────────
1400 lines in context        60 lines of signatures
                             + read offset=812 limit=40
```

**Subagents as garbage collectors.** A `subtask` subagent gets its own throwaway context. It can grep, read, and hit dead ends as much as it wants; only its final summary returns to your conversation. The mess is collected when it exits. Model tiers make this cheap too: delegate a search to a weak model at a fraction of the cost, keep the strong model for judgment.

```
main context                subagent context (discarded)
────────────                ────────────────────────────
subtask("find auth") ────►  glob, grep ×6, read ×9, ...
                     ◄────  "JWT middleware, auth.rs:120"
one line stays              ~20k tokens never seen
```

**Deferred MCP tools.** An MCP server with 100 tools would ship 100 definitions in every request. Maki loads a single `tool_search` tool instead; the model searches when it actually needs something and only the matches load. See [MCP](/docs/mcp/#tool-search).

**Truncation everywhere.** Tool output is capped (`agent.max_output_bytes`, `agent.max_output_lines`), overlong grep lines are skipped, and every builtin tool description nags the model to read only what it needs. The nagging works.

**Interrupted work is not wasted.** Press Esc on a long tool, or let its deadline hit, and whatever it printed so far still reaches the model, tagged as partial: bash keeps its streamed lines, `code_execution` the script output, a `subtask` subagent its half transcript. Otherwise the next turn starts from nothing and you pay to run it all again.

## Fewer round-trips

Every round-trip re-sends the context, so round-trips are the other half of the bill.

**batch** runs independent tool calls in one turn: one request, N results.

**code_execution** goes further: a Python sandbox where tools are async functions. Chained calls, loops, and filtering happen inside the sandbox; only what the script prints enters context.

```
without                          with code_execution
─────────────────────            ─────────────────────────────
glob        → 300 paths          results = gather(read × 300)
read × 300  → 300 files          filter in python
300 turns, every file            print("3 files call foo_v1")
in context forever               1 turn, 1 line in context
```

**Compaction** resets the multiplier when a session runs long: older turns are summarized and dropped. [Context](/docs/context/#when-the-window-fills) has the details.

## Reuse instead of retype

A model that plans a file in its thinking and then calls `write` emits the file twice. Output tokens are the expensive kind, so Maki keeps the model's own output in memory and lets a tool call point back into it.

**Drafts.** A fence labelled with a path is a draft of that file. A `write` that names the path and omits `content` gets the draft as its content. `read` and `edit` accept `draft:src/h.rs`, so the model can fix a few lines before anything reaches the disk. Several fences with the same path are versions, and the newest wins.

```
thinking                          tool call
────────────────────────          ─────────────────────────
```rust src/handler.rs            write(path="src/handler.rs")
fn handle(...) { ... }            content comes from the draft
```                               (0 extra output tokens)
```

**Pastes.** Any string argument may quote the first and last line of something the model wrote earlier, between `⟦` and `⟧`, either as `⟦first…last⟧` or as the two lines themselves. The lines between them are pasted in place, byte for byte, and a single quoted line pastes that line. Literal text can sit around or between quotes, so an edit to a draft costs the changed line rather than the whole block.

```
new_string = "⟦fn handle(…Response {⟧\n    log();\n⟦Response::ok…}⟧"
```

Resolution looks at the current message first (code blocks, then tables, then prose, then earlier tool arguments) and then at the previous eight assistant messages. The start quote must match once in the newest place it appears, and that place may be prose or thinking: a reply that mentions a line shadows the code block that holds it, which is what the source prefixes are for, and the ambiguity error names the prefix that reaches the block. The end quote is the first match after the start that closes its line, else the first match anywhere, so a `}` quoted as the last line is not stopped by the `}` inside `{name}!`. A place that has the start but not the end is skipped. Every tool result after an expanded call lists what was pasted, with line count and source, since the model only ever sees the short form it wrote. A line that itself holds a `⟦ ⟧` marker never matches, so a model that rehearses its tool call in thinking still pastes the real text. A quote that matches twice, or nowhere, fails the call with the candidates listed, and the model retries with a longer quote. One quote pastes at most 10 KiB: a wrong end line that would pull half a session into a file fails instead, and the model pastes in parts. Doubled brackets, `⟦⟦text⟧⟧`, write the markers themselves.

Drafts edited through `edit draft:<path>` live in memory for the session. After a restart the fences are rebuilt from history but those edits are not, so a `write` that relies on an edited draft is refused until the model passes content or fences the file again. A prefix picks a source when two places collide: `⟦thinking|…⟧`, `⟦text|…⟧`, `⟦code|…⟧`, or a tool name such as `⟦edit|…⟧`.

**Search, copy, clips.** The `transcript` tool works on the same buffer. `search` greps every line the model wrote, newest first, and tags each hit with where it came from (`m12 thinking code[src/h.rs]:3`), which matters after compaction when the text itself is gone from context. `copy` resolves a quote and shows the result at input-token cost, so a long paste can be checked before it lands in a file. `copy` with a `name` keeps the text as a clip, and `⟦@name⟧` pastes it from then on; a draft can be pasted the same way by its path.

The history sent back to the model keeps the short form, so the prompt cache stays warm. The buffer only grows: compaction and rewind change what the model is sent, never what it wrote, so a quote keeps resolving for the whole session.

## Watching it work

`/usage` shows the token breakdown of the current session, and `--output-format json` in [Headless Mode](/docs/headless/) reports `total_cost_usd` per run. Cheap is a feature you can measure.

Each turn is priced when it happens and that number is stored with the session. Prices move (DeepSeek, for one, doubles every rate during peak UTC hours), so a total re-priced later would be a guess. What you see is what you were billed.
