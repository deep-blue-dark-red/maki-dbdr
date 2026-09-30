# Merging upstream into this fork

This file documents the *process* for merging `upstream/main` into a fork branch.
`FORK.md` documents *what* the fork adds; this file documents *how* to get through
a merge without silently losing any of it. Read both — `FORK.md` first if you've
never done one of these merges before.

The fork and upstream both touch the same handful of large, central files
(`app/mod.rs`, `event_loop.rs`, `command.rs`, ...) at a high rate. A `git merge`
against a few months of upstream history routinely produces 30-120 conflicted
files. Most of that is mechanical, but a merge this size has two failure modes
that don't show up as conflicts at all and will pass `cargo build` while being
wrong. This file exists because both of them happened, repeatedly, in the merge
that prompted writing it down.

## Quick reference

```bash
git checkout <fork-branch>
git merge upstream/main          # or: git merge <other-branch-with-fork-work>

# resolve every UU file (see "Resolving a conflict" below)

scripts/merge-audit.sh           # catch what git merged silently and wrong
# fix whatever it flags, re-run until clean

cargo build --workspace          # catch what merge-audit.sh's heuristics missed
cargo test --workspace           # catch semantic drift that still compiles

scripts/verify-fork.sh           # confirm the 118 documented fork features survived
git commit                       # only once all four are clean
```

Do not skip straight to `cargo build`. It will not tell you about a file that
compiles fine but silently reverted a fix, because it has no idea what the file
used to do.

## The two failure modes git doesn't tell you about

### 1. Duplicate insert

Both sides add the same (or a near-identical) top-level item — a function, a
const, a Cargo.toml dependency key — in a region that doesn't textually overlap
with the other side's change. Git has nothing to conflict on, so it keeps both.

This merge hit it in `Cargo.toml` (`zstd = "0.13"` twice, one from each side
independently vendoring the same crate), `maki-providers/Cargo.toml` (same),
`maki-ui/src/agent/agent_loop.rs` (`do_checkpoint`/`do_rename` defined twice
after an unrelated nearby edit kept both copies), and `maki-ui/src/chat.rs`
(a stale `ChatEventResult::RenameResult` handler duplicated alongside the
already-correct one).

Signature: `cargo build` fails with `E0428` ("defined multiple times"), `E0592`
("duplicate definitions"), or a TOML "duplicate key" manifest error. Cheap and
mechanical to fix once you see it — the hard part is that a 100+-file merge
buries three or four of these in a wall of otherwise-successful build output,
one `cargo build` retry at a time.

### 2. Stale drift

A file that isn't conflicted at all, because only one side touched it, but the
other side's *other* files moved the ground it stands on: a method got renamed,
a type became non-`Option`, a struct field went private behind an accessor.
The untouched file still compiles against the old shape and either fails to
build (if the shape changed enough) or — worse — builds fine and is just wrong.

This merge hit the "builds fine and is wrong" version twice:

- `maki-ui/src/components/tool_display.rs` auto-merged with the *old* role
  prefixes (`"└ maki ∙ "` instead of `"maki> "`). It compiled. Nothing caught
  it until a message-search test failed on a string mismatch.
- A duplicate `App::checkpoint`-path call to `sync_subagents()` silently wiped
  the subagent list on every frame after a turn ended, because the doc comment
  on `sync_subagents` itself said not to call it there — and nothing enforced
  that at the type level. Caught only by two specific tests failing with an
  empty `Vec` where a populated one was expected.

Neither of these produced a compiler error. Both were only found by running the
full test suite and reading the failure, or (for the ones that don't have test
coverage) by diffing the file against both merge parents by hand.

## Resolving a conflict

For a marked (`UU`) conflict, decide per file — not in bulk — which side is
authoritative:

- **The fork side is authoritative** when the conflict is in fork-owned
  territory (see `FORK.md`'s file list) and upstream didn't independently
  rebuild the same feature.
- **Upstream is authoritative** when upstream did an architectural migration
  the fork's version predates (native-tool-macro → Lua plugins, single-session
  → concurrent sessions are the two examples so far). Taking the fork's side
  here doesn't just lose a feature, it fights the rest of the already-merged
  codebase.
- **Hand-merge** when both sides changed real, still-relevant behavior in the
  same file — most conflicts in shared "spine" files like `app/mod.rs` land
  here. Read both sides' intent, not just the diff.

Before bulk-resolving a whole batch of conflicts to one side, verify that side
is actually a superset — diff each file against *both* branch tips and grep the
other side's diff for anything that looks like a real, still-used feature (a
function called from somewhere else, a test asserting real behavior) rather
than a stale API shape. `git checkout --theirs` on a conflict you haven't read
is how a feature gets silently deleted. In the merge that prompted this doc,
that check caught a prompt-cache-miss detector and an MCP handle wire-up that
existed on exactly one side and nowhere else — a blind bulk-resolve would have
dropped both with a clean build and no error.

## Tools

### `scripts/merge-audit.sh`

Run after every `UU` marker is gone, before committing. Diffs every file that
changed on either side against the *current* working tree:

- If the file matches one parent exactly, nothing to check — that side's
  content won cleanly.
- If it matches neither, git manufactured something new for it. Sometimes
  that's a correct 3-way merge or an unrelated fix riding along for free;
  sometimes it's the duplicate-insert or stale-drift bug above. Either way it
  gets listed for a human decision, with the two comparison diffs to run
  printed in the output.

It also runs a cheap static check for exact duplicate top-level Rust symbols
and duplicate Cargo.toml keys, scoped to avoid the two things that look like
duplicates but aren't: same-named methods in different `impl` blocks, and
`#[cfg(unix)]` / `#[cfg(not(unix))]` platform splits.

```bash
scripts/merge-audit.sh                  # mid-merge: auto-reads ORIG_HEAD/MERGE_HEAD
scripts/merge-audit.sh <ours> <theirs>  # explicit refs, e.g. auditing after the fact
```

Exit 0 means nothing needs attention. Exit 1 means read the report — it is not
a substitute for `cargo test`, it's what runs *before* `cargo test` so the
build doesn't need to catch what's mechanically detectable without building.

### `scripts/verify-fork.sh`

The source-of-truth checklist from `FORK.md`: greps for a marker string or
symbol per documented fork feature and reports pass/fail per feature. Run
after the merge is otherwise clean, as the final gate before committing.

A failing check means one of two things:
1. **A real regression** — the feature is genuinely gone or broken. Fix it.
2. **A stale check string** — the feature is present but was reimplemented in
   a way the grep pattern doesn't match (a `.label` field became a `.label()`
   method, a helper got renamed). Confirm the feature actually works (test it,
   or grep for the new shape by hand), then update the check string in
   `scripts/verify-fork.sh` to match reality — don't leave a permanently-red
   check that everyone learns to ignore.

Known-permanent failures, by design (see `FORK.md` for the full rationale):
tool-macro crate and `render_hints.rs` (dropped — superseded by upstream's Lua
tool-rendering migration), `session_picker.rs`-specific checks (dropped —
superseded by upstream's concurrent-sessions Lua plugin; the fork behaviors
that mattered — `context_size`, global-sessions toggle — were ported into
`plugins/sessions/init.lua` instead), and delete-current-session (dropped —
structurally incompatible with upstream's can't-delete-the-focused-session
safety guard).

## Verification order, and why it's in this order

1. **`merge-audit.sh`** — no compiler needed, catches the mechanical
   duplicate-insert bugs before you spend a `cargo build` cycle on them, and
   surfaces every file the merge silently rewrote for review.
2. **`cargo build --workspace`** — catches everything `merge-audit.sh`'s
   heuristics don't reach: type mismatches, renamed APIs, anything that needs
   the compiler's full picture.
3. **`cargo test --workspace`** — catches semantic drift that still compiles.
   Both real bugs in this merge (`tool_display.rs`'s stale prefixes, the
   subagent-wiping `sync_subagents()` call) built cleanly and were only caught
   here.
4. **`verify-fork.sh`** — the fork-specific completeness gate. This is a
   checklist, not a test suite; it catches "this feature's marker string is
   gone" but not "this feature is subtly broken." Run it last, once the code
   is already known to build and pass tests.

Run `cargo test --workspace` per-crate (`cargo test -p maki-ui`, `-p
maki-agent`, ...) rather than all at once if the machine has limited cores —
running every crate's test binary concurrently can produce spurious
minutes-long stalls under thread oversubscription that look like hangs but
are just contention. One pre-existing flaky pair,
`maki-lua/tests/code_execution_policy.rs`'s
`interpreter_calls_advertised_tool_end_to_end` and
`workflow_tool_callable_when_workflow_true`, fails intermittently under full
parallelism and reliably passes with `--test-threads=1`; it is a thread-timing
issue unrelated to any merge, not a regression to chase.
