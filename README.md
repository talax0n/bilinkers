<h1 align="center">Bilinkers</h1>

## Requirements

| Tool | Why |
|---|---|
| Rust, from [rustup](https://rustup.rs) | Builds Bilinkers. `rust-toolchain.toml` selects Rust 1.99.1, which Rune needs. |
| Chrome, Brave, Edge, or Chromium | The browser that Bilinkers drives. It searches your computer for them. |
| [Codex CLI](https://github.com/openai/codex), signed in with `codex login` | Answers the questions that need an LLM. It uses your ChatGPT sign-in, so no API key is stored. Only `iframe` fill-in-blank questions and `final-test` need it. |

## Quick start

1. Optional: copy the example config to change advanced settings:

   ```bash
   cp .env.example .env
   ```

   The window picks the browser for you. A `BROWSER` line in `.env` wins over that choice, so delete it if you want to choose in the window.

2. Start Bilinkers:

   ```bash
   cargo run --release
   ```

3. The first time, the welcome screen asks you to sign in with ChatGPT and to choose your browser. Press Continue.
4. Press Start. Bilinkers opens the browser at the LMS. Log in and open a course, unit, or activity, and the run starts by itself. The Final Test is never started from the window.

## Modes

The window always detects the page type itself. Pass a mode to use it for the first run instead, for example `cargo run --release -- course`. `final-test` always runs in the terminal.

| Mode | What it does | Uses the LLM |
|---|---|---|
| `course` | Reads the roadmap counters ("Unit 4/8 · CP 1/3") and runs the next unfinished Unit or Checkpoint until none is left. Never opens the Final Test. | No |
| `unit` | Walks the activity rows top to bottom. Skips rows that already have a checkmark, plays video rows, and runs each activity with the matching mode. | No |
| `exercise` | Native exercises, such as reading comprehension and listening. Tries A, B, C, and so on until Check says correct, then clicks Submit. | No |
| `iframe` | LTI activities. Fill-in-blank asks the LLM. Error analysis tries every letter combination. Matching reads the answer off the page. Vocabulary slides are clicked through. | Fill-in-blank only |
| `reading` | Grouped reading passages. Cycles A to F on each question until the option locks, then clicks Submit. | No |
| `checkpoint` | Answers every question with one letter per attempt, submits, and starts a new attempt until the score is 100. | No |
| `final-test` | Writes an LLM answer key to `final-test-answers.md`. Read-only unless you opt in. **Never clicks Submit.** | Yes |

[`docs/platform-notes.md`](docs/platform-notes.md) describes how each page type works, with selectors and timing quirks.

## The Final Test

A Final Test allows exactly one attempt, so `final-test` is careful by default:

- It only reads the questions and writes the answer key to `final-test-answers.md`. It also writes `final-test-answers.json` for later passes.
- Each later run re-derives every answer independently. Answers that agree across passes get higher confidence.
- It clicks answers only when you set `FINAL_TEST_AUTO_ANSWER=1`. Then it clicks only the answers whose confidence reaches `FINAL_TEST_CONFIDENCE_THRESHOLD`.
- `FINAL_TEST_GUESS_LISTENING=1` clicks a random letter on each listening question instead of leaving it blank.
- It never clicks Submit. You review the answers and submit yourself.

`.env.example` documents every `FINAL_TEST_*` option.

## Configuration

Set these in the environment or in `.env`. Bilinkers reads the environment first, then `.env`, then the window's `settings.env` (`~/Library/Application Support/Bilinkers/` on macOS, `%APPDATA%\Bilinkers\` on Windows, `~/.config/bilinkers/` on Linux). The first one that sets a key wins.

| Variable | Meaning | Default |
|---|---|---|
| `BROWSER` | `chrome`, `brave`, `edge`, or `chromium` | `chrome` |
| `BROWSER_BINARY_PATH` | Path to the browser executable, when the search does not find it | Empty |
| `CDP_PORT` | Remote debugging port | `9222` |
| `CODEX_MODEL` | Model for `codex exec` | Empty, which uses the Codex CLI default |
| `MAX_ANSWER_RETRIES` | Retries for each LLM-answered question | `25` |
| `FINAL_TEST_*` | Final Test opt-ins | All off |

## Command reference

```bash
cargo run --release -- [--no-ui] [mode] [url]
```

| Command | What it does |
|---|---|
| `[mode] [url]` | Runs a mode. `url` defaults to `https://lms.binus.ac.id`. |
| `--no-ui` | Skips the window. You confirm in the terminal and get JSON log lines. `NO_TUI=1` does the same. |
| `attach` | Checks that Bilinkers can see your LMS tab. Changes nothing. |
| `open-row "<row title>"` | Opens one activity row by its title. |
| `eval-frame "<script>"` | Runs JavaScript in the activity frame and prints the result. Useful for debugging selectors. |
| `ui-demo --snap out.png` | Renders the window from sample events into an image, without a browser. |

## Project layout

| Path | Contents |
|---|---|
| `src/bin/` | The command-line entry point and argument parsing |
| `src/session.rs` | Run modes, browser launch, and the run thread that the terminal and the window share |
| `src/setup.rs` | The Codex sign-in check, the installed browser search, and the settings file |
| `src/browser.rs`, `src/frame.rs` | The CDP connection to the LMS tab and script evaluation inside the activity iframe |
| `src/detect.rs` | Detects which mode fits the open page |
| `src/runners/` | One runner per mode |
| `src/question_types/` | Detects, parses, and answers each question type |
| `src/strategy.rs`, `src/answer_loop.rs` | Answering strategies without the LLM, and the retry loop around each answer |
| `src/llm.rs` | Prompts, response parsing, and the Codex CLI client |
| `src/final_test.rs` | The Final Test answer key and the merge of multiple passes |
| `src/ui/` | The desktop window, behind the `ui` feature |
| `docs/platform-notes.md` | What Bilinkers knows about the LMS pages |

## Development

```bash
cargo test
cargo clippy --all-targets -- -D warnings
```

To build without the desktop window, turn off the default `ui` feature:

```bash
cargo build --no-default-features
```

## Credits

Bilinkers is made by TX25-2.
