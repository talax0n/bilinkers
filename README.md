# Beelingua Auto-Bot

A browser-driven bot that completes the Beelingua English course automatically. It reads each question straight out of the page DOM, asks an LLM (any OpenAI-compatible provider, or Gemini) for the answer, and drives Selenium to submit it — retrying once on a wrong answer before moving on.

> [!IMPORTANT]
> This project is for personal/educational automation of your own coursework. You are responsible for complying with your institution's academic integrity policies before using it.

## How it works

1. **Attach to your browser.** Instead of logging in for you, the bot attaches to an already-running **Chromium-based** browser (Chrome, Brave, Edge — launched with remote debugging enabled) so it reuses your existing logged-in session. This attach path is CDP-based and does not work with Firefox or Safari.
2. **Detect the question type.** Each question type has a small module under `src/questionTypes/` that knows how to recognize its DOM shape via `detect(dom)`.
3. **Parse and ask the LLM.** The matching module extracts structured question data (`parse(dom)`), which is sent to the LLM together with type-specific instructions.
4. **Answer and check.** The module performs the DOM interaction to submit the LLM's answer (`answer(driver, llmResult)`), then checks whether Beelingua marked it correct (`checkResult(dom)`).
5. **Retry once, then move on.** A wrong answer is retried once with feedback appended to the prompt; if it's still wrong, the bot logs the outcome and advances.
6. **Unknown types don't get guessed.** If no registered module matches the DOM, the bot saves the HTML (and a screenshot, where available) to `./logs/unhandled/` and stops, rather than guessing blindly.

### Supported question types

| Type | Module | Context |
|---|---|---|
| Audio multiple choice | `audioMultipleChoice.js` | Cycles every option until Check reports correct (audio can't be transcribed by the bot) |
| Reading comprehension | `readingComprehension.js` | LLM picks the correct option from the passage |
| Fill in the blank | `fillInBlank.js` | LTI-embedded quiz; LLM fills one or more blanks |
| Error analysis | `errorAnalysis.js` | LTI-embedded quiz; LLM picks the grammatically incorrect underlined word |

## Setup

### Prerequisites

- Node.js (v20+ recommended)
- A Chromium-based browser (Chrome, Brave, or Edge), plus a matching [chromedriver](https://googlechromelabs.github.io/chrome-for-testing/) binary — **required** for `scripts/run-exercise.js` / `run-iframe-exercise.js`, which attach over CDP (`--remote-debugging-port`). Firefox and Safari don't support this attach mode.
- An OpenAI-compatible LLM API (OpenAI itself, or any compatible proxy/provider) or a Gemini API key

### Install

```bash
npm install
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
# Native MUI-based exercises (reading comprehension, audio multiple choice)
npm run exercise

# LTI-embedded activities (fill-in-blank, error analysis)
npm run iframe
```

It opens the browser at `https://lms.binus.ac.id` by default (pass a different URL as an extra arg: `npm run exercise -- https://example.com`), prompts `Log in and open the exercise, then press Enter to start the bot...`, and once you hit Enter, runs the same loop as below: answers each question in place, checks the result, retries once on a wrong answer, and advances until the exercise ends or an unhandled question type is hit.

Uses `BROWSER` (`chrome`/`brave`/`edge` — CDP attach is Chromium-only) and `BROWSER_BINARY_PATH` from `.env` to know which browser to launch; see [Finding your browser binary path](#finding-your-browser-binary-path) if you need to set that.

### Manual (equivalent, more control)

1. Launch your Chromium-based browser with remote debugging enabled and log in to Beelingua manually:

   ```bash
   /Applications/Brave\ Browser.app/Contents/MacOS/Brave\ Browser --remote-debugging-port=9222
   ```

2. Navigate to the exercise you want the bot to complete, then run the matching script from the project root:

   ```bash
   node scripts/run-exercise.js
   # or
   node scripts/run-iframe-exercise.js
   ```

3. If a question type isn't recognized, check `./logs/unhandled/` for the saved DOM dump and add a new module under `src/questionTypes/` following the existing ones as a template.

## Testing

```bash
npm test
```

Runs the unit test suite (`node --test`) covering the runner, config, LLM wrapper, progress persistence, logger, browser helpers, and question-type registry. There are no automated tests against the live Beelingua site itself, since it requires a real logged-in session — question-type parsers are verified live during development instead.

## Project structure

```
src/
  browser.js           WebDriver setup (chrome/firefox/edge/safari), manual-login wait, DOM/iframe helpers
  config.js             Env-driven config (timeouts, retries, paths)
  llm.js                 OpenAI-compatible / Gemini client wrapper
  logger.js             JSON logging + unhandled-question dumps
  progress.js           Resumable lesson/section progress (progress.json)
  runner.js               Core per-question loop: parse → answer → check → retry
  questionTypes/
    registry.js          Ordered detect() lookup across registered types
    audioMultipleChoice.js
    readingComprehension.js
    fillInBlank.js
    errorAnalysis.js
    _optionButtons.js    Shared helpers for MUI-based option UIs
    _ltiQuiz.js          Shared helpers for LTI-embedded quiz UIs
scripts/
  cli.js                   One-command entry: launches browser, waits for login, runs the bot
  run-exercise.js          Bot logic for native MUI exercises (exports run(), also runnable directly)
  run-iframe-exercise.js   Bot logic for LTI-embedded activities (exports run(), also runnable directly)
docs/superpowers/
  specs/                 Design docs
  plans/                 Implementation plans
```
