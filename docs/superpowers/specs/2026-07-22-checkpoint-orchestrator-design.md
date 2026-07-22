# Checkpoint orchestrator design

## Context

The bot already drives one exercise (`run-exercise.js`), one LTI iframe activity (`run-iframe-exercise.js`), and a whole unit's activity list (`run-unit.js`, `4322abe`/`5a76a6d`/`a191b0a`). The course hierarchy also includes checkpoints: after every 2 units, a 30-question native quiz gated behind a "Start Attempt N" / "Continue" screen (`Total Question: 30`, `Passing Score: 100`, `Maximum Attempt: Unlimited`). This design covers driving one already-open checkpoint gate page to a passing (100) result.

Explicitly out of scope: clicking checkpoint nodes on the course map, walking multiple checkpoints/units across a whole course level. Those are a future course-level orchestrator, not built yet.

## Entry point

Browser is already sitting on the checkpoint gate page (same manual-open workflow as today's `exercise`/`iframe` modes — the user navigates there, then runs the bot). The gate button's text is **not fixed** — it reads "Start Attempt 1" on a fresh checkpoint, "Start Attempt N" on a subsequent one, or "Continue" if an attempt is already in progress. Matching must accept a button whose trimmed text starts with `Start Attempt` or equals `Continue`, not an exact string.

## Question flow

Once past the gate, the 30 questions render as the same native MUI quiz DOM `run-exercise.js` already drives (pill-number navigation 1..30, `Submit` → `Yes` confirm → result screen → `Next`). The checkpoint orchestrator reuses `run-exercise.js`'s exported `run(existingDriver)` directly — it is not a new question-answering loop, just a new entry/retry wrapper around the existing one. No new question-type handlers are needed unless a live run surfaces one that isn't registered today (same open risk as any other run).

## Score extraction

`run-exercise.js`'s `submitIfPresent()` clicks past the result screen ("Excellent! You Passed! Your Score: X") without reading it, since `run-unit.js` never needed the number. Add a small regex read (`/Your Score:\s*(\d+)/`) on the result-screen text before the existing "Next" poll/click, and include it as `score` on the object `submitIfPresent` resolves to. `main()`'s return value gains a `score` field (`undefined` when no result screen appeared, e.g. the reading/audio flow that finishes without a Submit). `run-unit.js` already spreads `...result` into a log call and ignores unknown fields — no change needed there.

## Attempt loop (`scripts/run-checkpoint.js`)

New script, same shape as the other `scripts/run-*.js` files (own `attachToBrave()` — matches the existing convention of each entry-point script attaching independently rather than sharing a driver-bootstrap module).

```
gateUrl = current URL
for attempt in 1..MAX_ATTEMPTS (5):
    clicked = click button where text starts with "Start Attempt" or === "Continue"
    if not clicked: log warning, stop (gate not found — nothing to do)
    wait for quiz DOM to render (poll, same pattern as clickStartGate)
    result = runExercise(driver)   // from run-exercise.js, existingDriver param
    if result.status === 'unhandled':
        log error, stop immediately — no retry, retrying hits the same unhandled type again
    if result.score === 100:
        log success, stop
    else:
        log the attempt's score/status, driver.get(gateUrl), continue loop
if loop exhausts MAX_ATTEMPTS without a 100:
    log warning: checkpoint not passed after N attempts, stop
```

`MAX_ATTEMPTS = 5` is a safety cap, not the platform's actual "Unlimited" policy — prevents an infinite loop if something structural (not just an unlucky guess) keeps the score below 100.

## cli.js wiring

- `MODES.checkpoint = '../scripts/run-checkpoint.js'`.
- `package.json` gets a `"checkpoint": "bun scripts/cli.js checkpoint"` script, alongside `exercise`/`iframe`/`unit`.
- `detectMode()` gets a new branch, checked after the iframe check and before the `exercise` lettered-option check (a checkpoint gate has neither an iframe nor lettered options, so ordering only matters for not falling through to `null`): a heading/text containing `Checkpoint` together with a `Start Attempt`/`Continue` button → `'checkpoint'`. This distinguishes it from the plain native-exercise `BlExercise` gate (same `Total Question`/`Passing Score` shape, but titled with the exercise's own name, not "Checkpoint").

## Testing

No existing `tests/*.test.js` covers `cli.js` or the `scripts/run-*.js` orchestrators (verified — they're driver-attached, live-browser scripts, not unit-tested). The score-regex parse in `submitIfPresent` is the one pure-logic piece worth a unit test; the rest is verified live against a real checkpoint the way every prior orchestrator change was (per the handoff's "Key gotchas" — DOM assumptions here are unverified until run against the actual page, since the checkpoint's UI was described, not yet driven).
