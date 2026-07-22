# Beelingua Auto-Bot

A browser-driven bot that completes the Beelingua English course automatically. It reads each question straight out of the page DOM, asks an LLM (any OpenAI-compatible provider, or Gemini) for the answer, and drives Selenium to submit it — retrying in place (up to 10 times, feeding the wrong-answer feedback, plus a running list of already-ruled-out answers, back to the LLM each try) before giving up on that question. It can also drive a whole unit's activity list end to end (`npm run unit`), opening and finishing each unfinished exercise in turn.

> [!IMPORTANT]
> This project is for personal/educational automation of your own coursework. You are responsible for complying with your institution's academic integrity policies before using it.

## How it works

1. **Attach to your browser.** Instead of logging in for you, the bot attaches to an already-running **Chromium-based** browser (Chrome, Brave, Edge — launched with remote debugging enabled) so it reuses your existing logged-in session. This attach path is CDP-based and does not work with Firefox or Safari.
2. **Detect the question type.** Each question type has a small module under `src/questionTypes/` that knows how to recognize its DOM shape via `detect(dom)`.
3. **Parse and ask the LLM.** The matching module extracts structured question data (`parse(dom)`) — for the LTI-embedded types, this includes the *actual on-screen instruction text*, read straight off the page rather than assumed, since both the wording and the graded rule (which tense, find-the-correct-word vs find-the-incorrect-word) vary per question. That instruction is sent to the LLM verbatim, plus a short answer-format note.
4. **Answer and check.** The module performs the DOM interaction to submit the LLM's answer (`answer(driver, llmResult)`), then checks whether Beelingua marked it correct (`checkResult(dom)`). Answering is paced with randomized human-like delays (reading time before answering, a beat before hitting Check, a gap between retries) rather than firing instantly.
5. **Retry in place, then stop on that question.** The LTI quiz's Check button re-evaluates on every click rather than locking after one try, so a wrong answer is retried with feedback appended to the prompt — up to `config.retry.maxAnswerRetries` (10) times — before the bot logs it as still incorrect and stops instead of advancing past it. The retry feedback isn't just "try again": if the page surfaces a specific hint on an incorrect answer, that's read and passed through verbatim (some quiz pages show the *same* hint regardless of which wrong option was picked, so every answer already tried and confirmed wrong is also listed explicitly — otherwise a stateless retry call has no memory of its own earlier guesses and can repeat one). A transient LLM/proxy failure is retried at the API-call level too, separately from this question-level retry, so a blip doesn't burn one of the limited attempts without ever trying a new answer.
6. **Unknown types don't get guessed.** If no registered module matches the DOM, the bot saves the HTML (and a screenshot, where available) to `./logs/unhandled/` and stops, rather than guessing blindly.

### Supported question types

| Type | Module | Context |
|---|---|---|
| Audio multiple choice | `audioMultipleChoice.js` | Cycles every option in order, clicking Check after each, until one reports correct (audio can't be transcribed by the bot) |
| Reading comprehension | `readingComprehension.js` | LLM picks the correct option from the passage (incl. True/False/Not-Given) |
| Fill in the blank | `fillInBlank.js` | LTI-embedded quiz; LLM fills one or more blanks per the on-screen instruction (tense/rule + worked example, read from the page) |
| Error analysis | `errorAnalysis.js` | LTI-embedded quiz; LLM picks the matching underlined word(s) — correct or incorrect, per the on-screen instruction — and can return more than one letter |
| Vocabulary intro slides | `vocabularyIntro.js` | LTI-embedded, ungraded word/meaning presentation slides between quiz pages; clicks through via the slide deck's own "next" link, no LLM call |
| Drag-and-drop matching | `quizMatching.js` | LTI-embedded; fills each blank with the word bank entry the page's own grading logic maps it to (index-based, not semantic), since simulating a real mouse drag never reliably registered with the page's drag library — no LLM call |

## Setup

### Prerequisites

- [Bun](https://bun.sh) v1.3+ — the project runs on Bun, not plain Node. This is required for the live OpenTUI dashboard: its renderer needs Bun's native FFI and throws under plain Node even though `@opentui/core` ships a "node" build (that build only satisfies imports/types, not actual rendering). Everything else (selenium-webdriver's CDP attach, chromedriver resolution, the test suite) works identically under Bun.
- A Chromium-based browser (Chrome, Brave, or Edge), plus a matching [chromedriver](https://googlechromelabs.github.io/chrome-for-testing/) binary — **required** for `scripts/run-exercise.js` / `run-iframe-exercise.js`, which attach over CDP (`--remote-debugging-port`). Firefox and Safari don't support this attach mode.
- An OpenAI-compatible LLM API (OpenAI itself, or any compatible proxy/provider) or a Gemini API key

### Install

```bash
bun install
```

### Configure

Copy `.env.example` to `.env` and fill in your LLM credentials:

```bash
cp .env.example .env
```

| Variable | Description |
|---|---|
| `LLM_PROVIDER` | `openai` (default) or `gemini` |
| `OPENAI_API_KEY` | API key for your LLM provider (used when `LLM_PROVIDER=openai`) |
| `OPENAI_BASE_URL` | Base URL of the OpenAI-compatible endpoint |
| `OPENAI_MODEL` | Model name to use (defaults to `gpt-4o`) |
| `GEMINI_API_KEY` | API key for Gemini (used when `LLM_PROVIDER=gemini`) |
| `GEMINI_MODEL` | Gemini model name (defaults to `gemini-1.5-flash`) |
| `BROWSER` | `chrome` (default), `firefox`, `edge`, or `safari` — only used by `src/browser.js`'s `createDriver()` (fresh-launch path) |
| `BROWSER_BINARY_PATH` | Optional path to a specific browser binary (falls back to `CHROME_BINARY_PATH` for backwards compatibility) |

> [!NOTE]
> `BROWSER`/`createDriver()` supports any of the four browsers above. **`scripts/run-exercise.js` and `scripts/run-iframe-exercise.js` are Chromium-only** — they attach over CDP to a browser with `--remote-debugging-port=9222`, a protocol Firefox and Safari don't implement. They auto-detect a matching chromedriver by querying the debug port for the browser's actual Chromium version (this matters for Brave, whose own version number differs from its underlying Chromium build) and reusing/downloading a chromedriver for that major version — set `CHROMEDRIVER_PATH` in `.env` to override this if it ever picks wrong.

### Finding your browser binary path

`BROWSER_BINARY_PATH` (and legacy `CHROME_BINARY_PATH`) is only needed if your browser isn't in the default install location, or you want to point at a specific one (e.g. Brave instead of Chrome). Default locations:

**macOS**

| Browser | Path |
|---|---|
| Chrome | `/Applications/Google Chrome.app/Contents/MacOS/Google Chrome` |
| Brave | `/Applications/Brave Browser.app/Contents/MacOS/Brave Browser` |
| Edge | `/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge` |
| Firefox | `/Applications/Firefox.app/Contents/MacOS/firefox` |
| Safari | n/a — no binary path, `BROWSER=safari` uses the system browser via `safaridriver` |

**Windows**

| Browser | Path |
|---|---|
| Chrome | `C:\Program Files\Google\Chrome\Application\chrome.exe` |
| Brave | `C:\Program Files\BraveSoftware\Brave-Browser\Application\brave.exe` |
| Edge | `C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe` |
| Firefox | `C:\Program Files\Mozilla Firefox\firefox.exe` |

(32-bit installs may live under `C:\Program Files (x86)\...` instead.)

Don't know if yours matches? Find it yourself:

- **macOS**: `mdfind "kMDItemFSName == 'Brave Browser.app'"` (swap the name for your browser), or right-click the app in Finder → Show Package Contents → `Contents/MacOS/`.
- **Windows**: right-click the browser's desktop/Start-menu shortcut → Properties → "Target" field, or in PowerShell: `(Get-Command chrome.exe -ErrorAction SilentlyContinue).Source` (swap `chrome.exe` for `brave.exe`, `msedge.exe`, `firefox.exe`).

## Running it

### One command (recommended)

`scripts/cli.js` launches your browser with remote debugging on, waits for you to log in, then runs the bot — no manual browser flags needed.

```bash
# Auto-detects which one you need after you open the page — no need to know
# upfront whether it's the native app, an LTI-embedded activity, or a unit's
# own activity list
npm run bot

# Or force a specific one:
npm run exercise   # native MUI-based exercises (reading comprehension, audio multiple choice)
npm run iframe     # LTI-embedded activities (fill-in-blank, error analysis, vocab slides, matching)
npm run unit       # a unit's activity list — runs every unfinished row in turn
```

It opens the browser at `https://lms.binus.ac.id` by default (pass a different URL as an extra arg: `npm run bot -- https://example.com`), prompts `Log in and open the exercise, then press Enter to start the bot...`, and once you hit Enter:

- With `npm run bot`, it inspects the current tab's DOM — an iframe present means the LTI activity; failing that, the narrow `bl-w-full justify-content-start` class (the actual lettered option buttons) means a native exercise; failing that, two or more titled `button.bl-w-full` rows means a unit's activity list — and picks the matching runner automatically, logging which one it detected. (Generic `MuiButtonBase`/`.bl-w-full` alone isn't a reliable signal on its own — this app's header, nav, and every row list use those same classes on every page.)
- Either way, it then runs the loop described above: answers each question in place, checks the result, retries in place on a wrong answer, and advances until the exercise ends, a question stays wrong after all retries, or an unhandled question type is hit. Native exercises that sit behind a "Total Question / Passing Score / Start" gate screen, or that finish with a Submit → confirm → result-screen "Next" step instead of a plain per-question "Next" button, are driven through that too.
- `npm run unit` is a level up from the other two: it reads the unit's own row list (skipping anything already checkmarked or still locked), opens the next unfinished row, runs `exercise` or `iframe` against it on the *same* browser session, returns to the list, and repeats until nothing unfinished is left or something can't be auto-solved (in which case it stops there rather than guessing past it, same philosophy as the unhandled-question-type case above).

If multiple `lms.binus.ac.id` tabs are open, detection (and the bot itself) targets whichever one it finds first — close the others first if you want a specific tab picked.

Uses `BROWSER` (`chrome`/`brave`/`edge` — CDP attach is Chromium-only) and `BROWSER_BINARY_PATH` from `.env` to know which browser to launch; see [Finding your browser binary path](#finding-your-browser-binary-path) if you need to set that.

### Manual (equivalent, more control)

1. Launch your Chromium-based browser with remote debugging enabled and log in to Beelingua manually:

   ```bash
   /Applications/Brave\ Browser.app/Contents/MacOS/Brave\ Browser --remote-debugging-port=9222
   ```

2. Navigate to the exercise (or unit activity list) you want the bot to complete, then run the matching script from the project root:

   ```bash
   node scripts/run-exercise.js
   # or
   node scripts/run-iframe-exercise.js
   # or, for a whole unit's activity list:
   node scripts/run-unit.js
   ```

3. If a question type isn't recognized, check `./logs/unhandled/` for the saved DOM dump and add a new module under `src/questionTypes/` following the existing ones as a template.

### Live dashboard

When `npm run bot` / `npm run exercise` / `npm run iframe` is run in a real terminal (stdout is a TTY), an [OpenTUI](https://github.com/anomalyco/opentui) dashboard replaces the raw JSON log lines with a live view: a header, a running correct/incorrect tally, and a scrolling log feed. It's a thin wrapper around the existing logger (`src/logger.js`'s `setSink()`), so `runner.js` and every question-type module are unaware it exists. Piped output (e.g. redirecting to a file, or CI) falls back to the plain JSON lines automatically; set `NO_TUI=1` to force that fallback even in a real terminal.

## Testing

```bash
npm test
```

Runs the unit test suite (`bun test`, using Node's built-in `node:test`/`node:assert` APIs — Bun runs those directly) covering the runner, config, LLM wrapper, progress persistence, logger, browser helpers, and question-type registry. There are no automated tests against the live Beelingua site itself, since it requires a real logged-in session — question-type parsers are verified live during development instead.

## Project structure

```
src/
  browser.js           WebDriver setup (chrome/firefox/edge/safari), manual-login wait, DOM/iframe helpers — polls for the iframe on load/transitions, and waits out two transient LTI states (the launcher's auto-submit form, and the Bits player's own empty <main> render window) instead of misreading either as unanswerable content
  chromedriver.js      Resolves a chromedriver matching whatever's actually on the CDP debug port (reuses cached, else downloads)
  config.js             Env-driven config (timeouts, retries, paths)
  dashboard.js         Live OpenTUI view of a run (header, correct/incorrect tally, log feed)
  llm.js                 OpenAI-compatible / Gemini client wrapper
  logger.js             JSON logging + unhandled-question dumps; can redirect to dashboard.js via setSink()
  progress.js           Resumable lesson/section progress (progress.json)
  runner.js               Core per-question loop: parse → answer → check → retry (tracks every wrong answer tried and rules them out explicitly in the next prompt; retries a failed LLM/proxy call itself before counting it as a wasted attempt; delay between retries)
  questionTypes/
    registry.js          Ordered detect() lookup across registered types
    audioMultipleChoice.js
    readingComprehension.js
    fillInBlank.js
    errorAnalysis.js
    vocabularyIntro.js   Ungraded LTI vocab/meaning slides — clicks through, no LLM call
    quizMatching.js      LTI drag-and-drop word-bank questions — answered by DOM index mapping, no LLM call (see "Supported question types" above)
    _optionButtons.js    Shared helpers for MUI-based option UIs; surfaces the page's own incorrect-answer hint text alongside the correct/incorrect outcome, when present
    _ltiQuiz.js          Shared helpers for LTI-embedded quiz UIs, incl. extractInstructionText() (reads the author-written instruction text off the page by its consistent styling, instead of a hardcoded task description)
scripts/
  cli.js                   One-command entry: launches browser, waits for login, auto-detects exercise/iframe/unit (or takes an explicit exercise|iframe|unit arg), runs the bot
  run-exercise.js          Bot logic for native MUI exercises — including the Start/Continue gate screen, numbered-pill question navigation, and the Submit → confirm → result-screen Next finishing sequence some exercises use instead of a plain "Next" button (exports run(driver?), also runnable directly)
  run-iframe-exercise.js   Bot logic for LTI-embedded activities (exports run(driver?), also runnable directly)
  run-unit.js              Walks a unit's activity list end to end: opens the next unfinished row, runs exercise/iframe against it on the same browser session, returns to the list, repeats (exports run(driver?), also runnable directly)
docs/superpowers/
  specs/                 Design docs
  plans/                 Implementation plans
```
