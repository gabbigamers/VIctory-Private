# Working in this repository

For engineers and for agents. Short on purpose: it says what to read, the
rules that always hold, and how a change is checked. Details live in the
documents `docs/INDEX.md` points to.

## Read in this order, and no more than the task needs

1. `docs/STATUS.md`: where the project stands now (one page).
2. `docs/INDEX.md`: which document, and which section of it, answers which
   question; which crate holds what.
3. Only the documents and sections the index names for the task at hand.

`docs/progress.md` is the chronological record (over a thousand lines).
Do not read it whole: search it (`grep -n`) for the topic, or read the
entry the status page names. Decisions already made are not reopened;
they are in `docs/STATUS.md` ("Decided") with a pointer to their entry.

## Rules that always hold (the owner's)

- Rendering correctness comes before performance. Nothing is skipped,
  dropped or left undefined silently: what cannot be handled is refused or
  handed to a fallback, and counted or logged. No effect is weakened or
  switched off to reach a frame rate. No frame generation.
- Behaviour follows detected capability and recorded validation, never a
  GPU model or vendor. NVIDIA is the first target; the design stays
  vendor-independent.
- Memory: 16 GB of RAM and a 16 GB card. A new long-lived allocation is
  bounded, has an owner and a lifetime, is counted through
  `crates/victory-gpu/src/account.rs`, and has its case in
  `docs/memory.md`. No copy is removed without that document's seven
  questions answered. A larger pagefile is never the answer.
- Experimental paths stay behind a switch that is off by default; with no
  switch set a change alters no translation, dispatch or allocation unless
  its report says so and the evidence carries it.
- Provenance: write the code here. Nothing is taken from another
  repository without its licence and origin checked and recorded in
  `THIRD_PARTY_NOTICES.md`. No game data, dumps, title shader names or
  hashes, or binary tables of unknown origin (`docs/asset-policy.md`).
- Only `VIctory-Private` is changed. Other repositories are read-only.
- Words for the state of work, used exactly: implemented; tested (host
  tests); accepted by Microsoft's validator on Linux; type-checked for
  Windows only; hardware-validated; speculative. A build that compiles, or
  a synthetic test that passes, is not hardware compatibility. No
  performance statement without a measurement on the owner's machine.

## Checking a change

Each worktree uses a build directory of its own. A build directory shared
between worktrees has served one tree's binaries to another here, and a
run then passed without testing the tree it was meant to test.

```bash
T=/path/to/a/build/dir/used/by/this/worktree/only
CARGO_TARGET_DIR=$T/host scripts/check-linux.sh tests            # tier 1: host tests
VICTORY_DXV=/path/to/dxc/bin/dxv CARGO_TARGET_DIR=$T/host \
  scripts/validate-dxil.sh                                       # tier 1b: Microsoft's validator
VICTORY_WINDOWS_TARGET_DIR=$T/win scripts/check-linux.sh windows # tier 2: Windows type-check, clippy -D warnings
CARGO_TARGET_DIR=$T/host cargo xtask policy                      # asset policy
```

- Confirm from the output that the change's own tests ran.
- `frame::tests::waits_end_on_ticks_and_count_frames` (`victory-gpu`)
  depends on the clock and can fail on a loaded machine: rerun it alone
  before reading it as a regression.
- A new rejection by the validator is a defect, never something to hide.
- Where a change claims to leave translations alone, show it: the
  containers the host tests emit (`VICTORY_DXIL_DUMP=<dir>`) are compared
  by hash before and after, and each hash that appears or disappears is
  explained.
- Tiers 3 and 4 (Windows tests, the card, the title) happen only on the
  owner's machine. Say what a run there must show.

## How work is organised

- A change is implemented in its own git worktree, reviewed by someone who
  did not write it, corrected, then brought onto the branch one change at
  a time and validated again there. Reviews need evidence the reviewer
  produced (a failing input, a command and its output), not opinion.
- Engineers and reviewers do not commit, push or stash, and do not edit
  `docs/progress.md`; they hand the coordinator the entry they propose.
  The coordinator integrates and updates `docs/STATUS.md`.
- A report says: what changed, what is identical to before and how that
  was shown, the four checks' results, what is not done, and what only the
  owner's machine can settle.
- Commit subjects follow the log's style (`git log --oneline -15`): the
  area, a colon, what the change does, in plain words.
