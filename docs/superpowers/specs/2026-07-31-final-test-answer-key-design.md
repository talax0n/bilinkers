# Final Test answer-key scraper design

## Context

`run-course.js` deliberately never opens the "Final Test" node on a course roadmap (whitelist only matches `Unit N` / `Checkpoint N`) — confirmed by inline comment at `scripts/run-course.js:17,62-65`. No runner or `cli.js` mode exists for it either. Unlike a checkpoint (30 questions, unlimited attempts, brute-forceable across retries), a Final Test is **50 questions, 1 attempt** — there is no room to brute-force wrong guesses, and there's no live gate to re-open on failure. The user wants to work the Final Test manually in the browser but have an answer key ready ahead of time, so this tool only *reads* the exam and writes a guess for every question — it never clicks an option, Save, or Submit.

## Entry point

Browser is already sitting on an open Final Test attempt (same manual-open convention as every other mode) — past the gate, on the same native MUI pill-nav quiz DOM `run-checkpoint.js` already knows how to read (`.bl-button__container` pills, collapsed-by-default pill bar with an expand chevron). This design reuses `run-checkpoint.js`'s already-solved pill helpers (`ensurePillsExpanded`, `readPills`, `clickPill`) unmodified.

## Question flow (read-only)

For each pill 1..N (N=50, but read from `readPills()`, not hardcoded):

1. `clickPill(driver, n)` to navigate (no Save/Submit anywhere in this script).
2. `getCurrentQuestionDom(driver)` (from `src/browser.js`, same helper every runner uses).
3. `registry.findHandler(dom)` against a registry with the same two types `run-checkpoint.js` registers: `audioMultipleChoice`, `readingComprehension`.
4. If the handler is `audioMultipleChoice` (listening — the bot can't hear audio) → record `{ number: n, type: 'listening', skipped: true }`, no LLM call.
5. If the handler is `readingComprehension` → `handler.parse(dom)` gives `{ text, options: [{letter, text}] }` (already exactly what `answerQuestion()` expects — no new parsing code needed). Call `answerQuestion(llmClient, model, instruction, questionData, null)` from `src/llm.js` **once** — single best guess, no feedback loop (there is no per-question feedback to loop on, and only one real attempt exists). Record `{ number: n, type: 'mcq', text, options, answer: llmResult.answer }`.
6. If no handler matches → record `{ number: n, type: 'unhandled' }` (logged, not fatal — see error handling).

This never calls `handler.answer()` (the part that clicks an option) and never touches Save/Submit — the only DOM interaction in the whole script is pill navigation.

## Error handling

- LLM call failure (network/parse error) → retry up to 3x (same `callWithRetry` shape as `src/runner.js`); still failing after 3 → record `{ answer: 'NEEDS REVIEW' }` for that question and continue to the next pill. One bad question must not abort the other 49.
- Pill bar fails to expand / `readPills()` returns 0 pills → fatal, stop immediately (nothing useful can happen with a handful of the 50 questions visible).
- Unhandled question type → logged and recorded as `unhandled` in the output, loop continues (matches the "don't let one gap kill the whole run" principle, but unlike the checkpoint loop there's no retry to worry about corrupting).

## Output

Markdown file `final-test-answers.md` (repo root, gitignored scratch output — not committed), written after the full pass (not incrementally), one section per question in pill order:

```md
## Q3
Which sentence uses the past perfect correctly?
A. ...
B. ...
C. ...
D. ...
**Jawaban: B**
```

Listening questions render as:

```md
## Q7 (listening — skipped, jawab manual)
```

Unhandled questions render as:

```md
## Q12 (unhandled question type — jawab manual)
```

## cli.js wiring

- `MODES['final-test'] = '../scripts/run-final-test.js'`.
- `package.json` gets `"final-test": "bun scripts/cli.js final-test"`, alongside the other modes.
- No `detectMode()` branch — Final Test is opened explicitly by the user the same way `checkpoint-capture` is (not auto-detected), since it's a rare, high-stakes, manually-invoked action distinct from routine solving.

## Testing

Unit tests (`tests/run-final-test.test.js`, faked `driver`/`llmClient`, same style as `tests/run-checkpoint.test.js`):

- MCQ question → `answerQuestion` called once, result recorded with the right shape.
- `audioMultipleChoice`-detected question → `answerQuestion` never called, recorded as skipped.
- `answerQuestion` rejecting 3x → recorded as `NEEDS REVIEW`, loop continues to the next pill.
- Unhandled DOM → recorded as `unhandled`, loop continues.
- Markdown output formatting (question/options/answer, skip line, unhandled line) — pure function, testable without a driver at all.
- Zero pills after `ensurePillsExpanded` → function returns/throws the fatal error path, no LLM calls attempted.

No live verification is possible yet — the Final Test is presumably locked until the course's units/checkpoints are complete, same caveat `run-checkpoint.js`'s original WIP carried. Flagged as a known limitation: DOM assumptions here (pill shape, handler detection) are inherited from the already-verified `run-checkpoint.js`/`run-exercise.js` code, but the Final Test page itself is unverified until run live.
