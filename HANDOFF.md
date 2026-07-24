# Session Handoff — Beelingua Bot

Date: 2026-07-24. Goal of this session: make every activity type solve by
**brute-force blocking** (no LLM where avoidable), and add **unit-level
orchestration** so `bun run bot` on a unit page finishes everything.

## How to run / test (IMPORTANT)

- `bun run bot` launches Brave with `--remote-debugging-port=9222`, waits for
  Enter, auto-detects the open activity, runs the matching runner.
- Selenium attaches to that debug port. **Only ONE driver at a time.** Running
  a second script against 9222 while the bot runs makes two drivers fight the
  same SPA and everything *looks* stuck. (This bit us repeatedly — the
  "vocabulary intro stuck" report was just this race, not a bug.)
- Modes (`scripts/cli.js` MODES + `detectMode`): `exercise` (native MUI,
  top-level), `iframe` (Bits/LTI player), `reading` (grouped reading BlExercise,
  iframe or top-level), `checkpoint`, `unit`.

## What shipped this session (committed)

All verified live unless noted.

- `6a2bbe3` **reading BlExercise brute-force** (`scripts/run-reading-exercise.js`,
  mode `reading`). Passage + numbered tiles, mixed MCQ/TFNG/paraphrase. Wrong
  answers re-enable their options, correct answers lock — so cycle A→D per
  question until the option locks; all-correct → Submit. No LLM. Verified
  **Score 100 (15/15)**.
- `b7e727c` **readingComprehension → brute force** (added to `BLAST_TYPES` in
  `scripts/run-exercise.js`). Cycles A–D against the Check feedback. No LLM.
- `8647e1d` **errorAnalysis parse fix** — parse candidates from bare `<u>`
  (some questions have no `<b>` wrapper). Was returning 0 candidates → "exhausted
  letters". errorAnalysis was already brute-forced. Verified A→D w/ unclick, 5/5.
- `0f3d43b` **vocabularyIntro detect fix** — don't false-match on the quiz-matching
  JSON manifest embedded in intro slides.
- `03dd506` **reading exercise: per-question Check-vs-blocking** — if a question
  has a "Check My Answer" button, cycle its options in place (Check → Incorrect
  keeps options enabled → next letter → Correct locks). Else use blocking via
  numbered-pill transitions; Save/Save & Next are not used for progression.
  Verified Unit 2 **Score 100**.
- `62193b3` **vocabulary presentation slides not counted as questions** — in
  `scripts/run-iframe-exercise.js`, `PRESENTATION_TYPES = {vocabularyIntro}` are
  walked forward via `#/n` without processQuestion / question count. (A vocab unit
  = ~15 word/meaning cards, THEN real `#/question-N` items.)
- `a0878af` **unit orchestrator routes reading exercises** — `run-unit.js`
  detectMode now looks inside the iframe: MUI option buttons → `reading`, else
  `iframe`. Verified: orchestrator ran MC Grammar (15/15) then auto-picked up the
  unlocked Listening (10/10, 100).
- `70d4fc2` **unit orchestrator auto-completes video + notes** — non-quiz rows:
  video → mute + seek `<video>` to end (fires `ended`; verified `ended===true`);
  notes/static → opening marks viewed. Added an `attempted` Set guard so a row
  that won't auto-complete isn't re-picked forever.

### Unit orchestration status
`bun run bot` on a unit page → `unit` mode → `run-unit.js`: finds first
unfinished row (yellow arrow = not locked, not checkmarked), opens it, clicks
Start/Continue gate, detects type, runs the right runner, returns to the list,
repeats; auto-picks up newly unlocked rows. **Verified end-to-end on Unit 2.**
Caveat: video/notes *completion registration* on a genuinely INCOMPLETE row is
unverified (all Unit 2 media was already done) — only the seek mechanism is
verified. Test on a unit with incomplete media; if the checkmark doesn't flip,
the platform likely needs a specific progress event.

### Which types use the LLM still
Only `fillInBlank` (free-text grammar blanks) — kept LLM on purpose (no finite
option set to brute-force). Everything else is brute-force/deterministic.

## Grouped top-level reading (UNCOMMITTED, LIVE VERIFIED)

Live-proven on **Unit 3** top-level `/BlExercise/...` paraphrase reading:

- No iframe. The page has **15** `.bl-button__container` numbered pills and
  **five visible A–D groups per pill**.
- Grouped reading detection requires at least two numeric pills and at least two
  visible option groups (a `button.bl-w-full.justify-content-start` sequence
  starts each time its letter restarts at A). This preserves ordinary one-group
  native exercises as `exercise`.
- Auto-detection order is iframe Bits → iframe reading → checkpoint gate →
  top-level grouped reading → generic top-level exercise → unit.
- Deferred grading is pill-only: after selecting the next A→F candidate in every
  unlocked group, the solver navigates to the next numbered pill. It never uses
  Save, Save & Next, Next, or Skip for grouped-reading progression. The final
  pill wraps to another pill before Submit. A wrong selection re-enables after a
  pill round trip; a correct selection locks all options. Both behaviors were
  verified live; Save was not used.
- `tests/grouped-reading.test.js` covers strict routing and the one-group native
  boundary. `tests/run-reading-exercise.test.js` covers pill-only traversal and
  final-pill departure before submit.
- Full live run completed all 15 pills in four blocking passes (A→B→C→D) and
  submitted **Score 100**. Logs showed 53 → 50 → 26 → 4 still-open groups per
  pass; no Save/Save & Next action was used.

## WORK IN PROGRESS — checkpoint brute-force (UNCOMMITTED)

**File: `scripts/run-checkpoint.js` — modified, NOT committed, unit-tested but
not yet verified end-to-end live.**

Goal: switch `checkpoint` from LLM (one guess/question, retry whole thing) to
brute-force blocking, like everything else.

### Checkpoint mechanics learned (verified live on "Checkpoint 1", 30 Q, pass 100, unlimited attempts)
- **No per-question feedback** — only a final score after the whole attempt.
- Questions are **one-per-screen**, 5 options, nav button is **"Save"** (not
  "Save & Next"). Navigated by a **numbered pill bar 1..30** at the top.
- Brute force is **across attempts**: answer all with a fixed letter, submit;
  the platform **re-presents only still-incorrect questions** on retry → answer
  those with the next letter. So attempt1=all A, attempt2=remaining B, then C, D.
  A correct answer LOCKS (its options disable) and must be preserved (skip it on
  later attempts), so fixed-letter-per-attempt converges with NO per-question
  bookkeeping. `main()` cycles letters A–F mod, unlimited retries until score 100.

### The blocker (where I stopped)
Do **NOT** use the "Save" button to advance — its next-jump is erratic (it goes
to the next *unanswered* question, and with polluted state from test runs it
jumped 3→27, skipping 4–26). **User's instruction: navigate by clicking the
number pills directly** (like `run-reading-exercise` clicks tiles).

I rewrote `answerAllQuestions(driver, letter)` to: `readPills` → for each pill,
`clickPill(n)`, answer with `letter` (skip if options already locked = correct),
then grade by navigating to another numbered pill. It never clicks Save or Save
& Next. Then `clickSubmit`. Pill state colors: answered `rgb(36,164,174)`, current
`rgb(232,131,38)`, unanswered `rgb(127,202,212)`.

**Remaining problem: the pill bar is COLLAPSED by default and only renders ~15
of 30 pills in the DOM** (`.bl-button__container` with numeric text). It EXPANDS
to show all 30 via a toggle button (the `^` chevron at the top-right of the pill
bar — see the user's screenshot). When expanded, `readPills` correctly returns
all 30 and `clickPill` works.

Last probe (fresh attempt) found **19** pills and these top-region svg buttons as
toggle candidates: `MuiButtonBase-root` at left≈791/829/862/896, top≈16–23 (the
pill bar is at top of page). The expand toggle is almost certainly one of those
(rightmost, ~left 896 top 16). **Next step: click the expand toggle before
`readPills` so all 30 pills are in the DOM, then the pill-nav loop should work.**
Verify the bar STAYS expanded across pill navigation (re-expand if it collapses).

### Also note
- The live "Checkpoint 1" attempt state is **polluted** from many test runs
  (partial answers persisted). For a clean test, may need to finalize/submit the
  current attempt first, or ensure "Start Attempt N" gives a blank attempt.
- `scripts/run-checkpoint-capture.js` (untracked) is an older brute-force
  *elimination* checkpoint solver (A→B→C→D by which questions drop from the
  re-presented set) that also records answers to `checkpoint-capture.json`. It
  predates this rewrite; reference only.
- `main()` letter cycling + pill-only traversal tests pass. Completion retries
  are bounded and stop on no progress instead of recursing forever; the full
  suite is **55/55 green**.

## Uncommitted working tree
- `scripts/run-checkpoint.js` — the WIP above.
- `src/dashboard.js`, `src/runner.js`, `tests/*.test.js` — pre-existing
  uncommitted changes from before this session (not touched by me; verify before
  committing).
- Untracked: `scripts/run-checkpoint-capture.js`, `scripts/_click_tile.js`,
  `scripts/_list_clickables.js`, `checkpoint-capture.json` (all pre-existing
  scratch/capture artifacts).

## Immediate next steps
1. Finish checkpoint: expand the pill bar (click the `^` toggle) before
   `readPills`; run live on Checkpoint 1 to Score 100; commit.
2. Test unit orchestrator on a unit with an INCOMPLETE video/notes to confirm
   completion registration.
