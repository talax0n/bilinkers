# Beelingua Bot

Completes Beelingua English-course activities on `lms.binus.ac.id`. It drives your own logged-in browser over the Chrome DevTools Protocol (CDP), answers each question, checks the result on the page, and moves on. A desktop window built with [Rune](https://github.com/anomalyco/rune) shows the run live.

## How each page gets completed

| Mode | What it does | Uses the LLM |
|---|---|---|
| `course` | Reads the roadmap's "Unit 4/8 · CP 1/3" counters and runs the next unfinished Unit or Checkpoint node until none is left. Never opens the Final Test. | no |
| `unit` | Walks the activity rows top to bottom, skips checkmarked ones, plays video rows, and runs each activity with the matching mode. | no |
| `exercise` | Native exercises (reading comprehension, listening): tries A, B, C… until Check says correct, then Submit. | no |
| `iframe` | LTI activities. Fill-in-blank asks the LLM. Error analysis tries every letter combination. Matching reads the answer off the page. Vocabulary slides are clicked through. | fill-in-blank only |
| `reading` | Grouped reading passage: cycles A to F per question until the option locks, then Submit. | no |
| `checkpoint` | Answers every question with one letter per attempt, submits, and starts a new attempt until the score is 100. | no |
| `final-test` | Writes an LLM answer key to `final-test-answers.md`. Read-only unless you opt in. **Never clicks Submit.** | yes |

Without a mode, the bot detects the page type itself. How each page works, with selectors and timing quirks, is in [`docs/platform-notes.md`](docs/platform-notes.md).

## Setup

1. Install Rust with [rustup](https://rustup.rs). `rust-toolchain.toml` selects Rust 1.99.1, which Rune needs.
2. Install the [Codex CLI](https://github.com/openai/codex) and run `codex login`. The bot sends questions through `codex exec`, so it uses your ChatGPT sign-in and stores no API key.
3. Have Chrome, Brave, or Edge installed.
4. Copy `.env.example` to `.env`. The defaults work with Chrome. For Brave, set `BROWSER=brave`.

| Variable | Meaning |
|---|---|
| `BROWSER` | `chrome` (default), `brave`, or `edge` |
| `BROWSER_BINARY_PATH` | The browser's executable, when it is not in the default location |
| `CODEX_MODEL` | Model for `codex exec`. Empty uses the Codex CLI default. |
| `CDP_PORT` | Debug port, default `9222` |
| `MAX_ANSWER_RETRIES` | Retries per LLM-answered question, default 25 |
| `FINAL_TEST_*` | Final-test opt-ins, documented in `.env.example` |

## Run

```bash
cargo run --release -- course
```

The bot starts your browser with remote debugging on (or reuses one already running with it), waits for you to log in and open the page, then runs when you press Enter. Replace `course` with any mode from the table, or leave it out to auto-detect.

- `--no-ui` (or `NO_TUI=1`) prints JSON log lines instead of opening the window.
- `cargo run --release -- attach` checks that the bot can see your LMS tab, without changing anything.

## Development

```bash
cargo test
cargo clippy --all-targets -- -D warnings
cargo run -- ui-demo --snap out.png   # render the window headlessly from sample events
```

`cargo build --no-default-features` builds without the window (the `ui` feature).
