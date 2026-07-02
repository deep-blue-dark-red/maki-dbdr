MODE: COMPACTION — this block REPLACES the conversation; it must stand alone for an agent that sees only this.

Produce the state block now. Match the shape and annotations below, but the values shown are illustrative synthetic filler — replace every one with the real conversation; copy none of them. Output ONLY the block.

<compaction_state>
### MISSION
[USER] one line: the overarching goal
[BLOCK] a hard constraint          (omit the line if none)
1-3 lines: key decisions/pivots and why they were made

### WORKSPACE
files:   (format: path:lines | state | note ; state = new|modified|deleted|read)
  src/foo.rs:120-140 | modified | what changed and why it matters
git_dirty: uncommitted changes relevant to resuming, or "clean"
calls:   (format: exact command | outcome)
  cargo test -p foo | 3 pass, 1 fail (see [ERR])

### GIT
#a1b2c3d telegraphic description of the commit

### CHAIN
(only pivots, discoveries, state changes. [T-N] = transcript turn index if given, else number from [T-1])
[T3] [USER] wants X → built Y → [ERR] Z → fixed by W
[T5] discovered <fact> → switched approach to <plan>

### QUEUE
[ERR] unresolved failure, fenced          (omit if none)
[Q] open question                          (omit if none)
[NEXT] the single immediate next action
further queued steps, one per line
</compaction_state>
