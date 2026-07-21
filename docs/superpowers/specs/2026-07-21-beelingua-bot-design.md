# Beelingua Auto-Bot — Design

## Purpose
Automate completion of the Beelingua English course via a browser-driven bot. Bot reads course questions, uses an LLM (OpenAI-compatible provider, custom base URL + API key) to determine answers, and performs the DOM interactions to submit them.

## Stack
- Node.js
- `selenium-webdriver` (Chrome)
- `openai` SDK, configured with custom `baseURL` and `apiKey`
- `dotenv` for config

## Flow

1. **Login handoff.** Bot launches Chrome, navigates to Beelingua login page, then polls the DOM waiting for a signal that login succeeded (e.g. presence of a post-login-only element). User logs in manually. Bot never handles credentials.
2. **Course navigation loop.** Bot walks lesson → section structure automatically, covering the whole course unattended.
3. **Section instructions.** Per section, scrape the instruction text once at section start; cache it as context for every question answered within that section.
4. **Per-question loop:**
   - Detect question type via DOM signature (class names / data-attributes), using an ordered registry of type detectors.
   - **Known type:** dedicated parser extracts structured question data (text, options, blanks, pills, etc.) → LLM call with `{ instruction, questionData }` → dedicated handler performs the DOM interaction (click/type) → submit.
   - **Unknown type:** save screenshot + DOM dump to `./logs/unhandled/<timestamp>-<type>.{png,html}`, log the event, pause the run (or skip just that section) so a parser can be added later. No guessing via vision fallback.
5. **Result check / retry.** After submitting, check if Beelingua marks the answer wrong. If wrong: re-prompt the LLM once, including the "that was wrong" feedback, and retry the same question. If still wrong (or no retry possible): log it as skipped/failed and move on. Never get stuck on one question.
6. **Progress persistence.** Bot writes `progress.json` (last completed lesson/section) after each section, so a stopped/crashed run can resume rather than restarting the whole course.

## Question-type perception strategy
Hybrid: DOM scrape is the primary path for known types (cheap, reliable, works well with DOM-based answering). Screenshot-based vision fallback is reserved, but per the "unknown type" decision above, it is **not** auto-invoked — unhandled types pause for manual parser authoring instead of best-effort guessing. (This keeps behavior predictable; can be revisited later if unknown types turn out to be common.)

## Components

- `src/browser.js` — WebDriver setup, manual-login wait, navigation helpers.
- `src/llm.js` — OpenAI client wrapper; `answerQuestion(instruction, questionData)` → structured answer.
- `src/questionTypes/` — one module per question type (`fillBlank.js`, `multipleChoice.js`, `pillClick.js`, ...), each exporting:
  - `detect(dom)` — does this DOM match this type?
  - `parse(dom)` — extract structured question data
  - `answer(driver, llmResult)` — perform the DOM interaction to submit the answer
- `src/questionTypes/registry.js` — ordered list of type modules; `findHandler(dom)` returns the matching module or `null` (unknown-type path).
- `src/runner.js` — main loop: section iteration, instruction caching, question loop, retry-once-on-wrong, progress save.
- `src/progress.js` — read/write `progress.json`.
- `src/logger.js` — general logging + unhandled-type dumps to `./logs/unhandled/`.
- `.env` — `OPENAI_BASE_URL`, `OPENAI_API_KEY`, `OPENAI_MODEL`.
- `config.js` — timeouts, retry counts, misc tunables.

## DOM discovery
Beelingua's actual DOM structure (per question type) is not yet known. Parsers will be authored during a live session: user logs in, we drive a real Selenium/devtools session against actual pages to capture DOM structure for each question type as we build its module. This happens during implementation, not during this design phase.

## Testing
No CI fixtures against the live site (requires manual login, no test account automation). Instead:
- `npm run smoke` — dry-run script: opens browser, waits for manual login, processes only the first question it finds, prints detected type + parsed question data + LLM's chosen answer, but does **not** submit/click. Used to validate a new parser without risking incorrect submissions on the real course.
- Real parser correctness is verified live during the DOM-discovery/build session per type.

## Out of scope / explicitly deferred
- Vision-based fallback answering for unknown question types (logs + pauses instead).
- Any credential storage/auto-login.
- Automated test fixtures / CI for DOM parsing (site requires live login).
