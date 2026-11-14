# Beelingua platform notes

What the bot knows about lms.binus.ac.id and the Beelingua course pages. Most facts were verified against the live site. The source file that implements each fact is named in parentheses.

## Browser attach

- The binary launches the browser with `--remote-debugging-port=9222` and the start URL (default `https://lms.binus.ac.id`) unless the debug port already answers. In the terminal (`--no-ui`) the user logs in, opens the page, and presses Enter. In the window, Start does the same launch. (src/session.rs, ensure_browser)
- On Home the window polls `/json/list` about once a second, which lists tabs without opening or focusing any, to show whether the browser and an LMS tab are open. With two or more LMS tabs it also asks each tab which one is visible, over one CDP session it opens once and reuses. (src/session.rs, Watcher::connection)
- After Start the window looks again about once a second. Once a page target's URL contains `lms.binus.ac.id`, it attaches once and runs page detection (read-only scripts, no clicks) on every look, reusing that CDP session until it stops answering. A supported page starts the run by itself and the watch session is dropped, since the run attaches its own. The island map starts the island run (since 2026-11-10). (src/session.rs, Watcher; src/detect.rs, classify_page)
- A Final Test page is refused in the window before any other detection, since its gate and questions look like a checkpoint or an exercise. It matches when the URL contains "final test" (any separator) or the body text contains "Final Test", unless the page is a course roadmap. Not yet verified on a live Final Test page. `bilinkers final-test` always runs in the terminal. (src/detect.rs, FINAL_TEST)
- The bot talks CDP directly through chromiumoxide, so no chromedriver is needed. (src/browser.rs, attach_to_lms)
- Tab choice: among page targets whose URL contains `lms.binus.ac.id`, the bot evaluates `document.visibilityState` and `document.hasFocus()` in each and picks the first that is visible and focused, else the first visible one (the active tab of some window), else the first listed. It logs "Picked LMS tab" with `why` = `focused`, `visible` or `first`. Attach, the Home probe and the watch before a run use the same rule. Evaluating does not focus a tab. Only attach brings the picked tab to front, since pointer clicks hang in a background tab. (src/browser.rs, choose_tab, pick_lms_page)
- `document.hasFocus()` is false in every tab while another app (the Bilinkers window, a terminal) has the focus, so `visible` is the usual reason.
- A hidden tab can leave `Runtime.evaluate` unanswered (seen live on a background binusmaya tab in Brave), so each tab gets 1.5 s and a silent tab counts as hidden.
- Ties go to the tab `/json/list` lists first, which Chromium is said to sort by last activity, most recent first. Not verified by switching tabs. On 2026-11-10 Brave listed the one visible LMS tab (the island map) first, ahead of a hidden island map tab and a hidden course tab, which fits that order.
- The DevTools HTTP endpoint only answers HTTP/1.1 probes and may keep the connection open, so a probe reads up to Content-Length. (src/session.rs)
- The activity iframe is cross-origin, so it is its own CDP target. The bot attaches to it by the iframe's frame id (which is its target id) and looks it up fresh on every call, so a reloaded iframe is picked up. It reattaches only when the connection drops, never when a script throws, so a click never runs twice. (src/frame.rs)
- CDP attach works only with Chromium browsers (Chrome, Brave, Edge, Chromium).

Window:

- The Rune window opens when stdout is a terminal, a display is available, neither `--no-ui` nor `NO_TUI` is set, and the mode is not `final-test`. Otherwise events print as JSON lines. (src/bin/bilinkers.rs, src/ui/)
- It opens on a welcome screen unless `codex login status` exits 0 and the browser settings resolve to an installed binary. Codex 0.167 prints its status ("Logged in using ChatGPT", "Not logged in") on stderr; the exit code decides. Sign in runs `codex login`, which opens the system browser and exits when done. Codex is looked up on PATH, then in /opt/homebrew/bin, /usr/local/bin and per-user npm dirs, since a Finder launch has a short PATH, then in the app's own bin dir. The app's copy comes last because it never updates itself, so an install the user manages wins. Status, sign-in and `codex exec` all use this lookup. (src/setup.rs, find_codex)
- When Codex is missing, the welcome screen offers Install Codex. It reads `https://api.github.com/repos/openai/codex/releases/latest` and downloads `codex-<target>.tar.gz` (`aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-pc-windows-msvc.exe`, `aarch64-pc-windows-msvc.exe`, and the static `x86_64-unknown-linux-musl` / `aarch64-unknown-linux-musl`). Each archive holds one file named `codex-<target>[.exe]`. The download URL must start with `https://github.com/openai/codex/releases/download/`, and GitHub redirects it to release-assets.githubusercontent.com. curl runs with `--proto =https --proto-redir =https`. (src/install.rs)
- The release's `codex-package_SHA256SUMS` covers only the `codex-package-*` bundles, not the plain CLI tarballs. The `.sigstore` bundles exist only for Linux. So the download is checked against the `digest` (sha256) that the GitHub API records for each asset, and an asset without a digest is refused. Sigstore signatures are not verified.
- The tarball is unpacked with the system `tar` into a staging folder inside the bin dir, then made executable (unix) and renamed to `codex` / `codex.exe`. The bin dir is `~/Library/Application Support/Bilinkers/bin` on macOS, `%APPDATA%\Bilinkers\bin` on Windows, and `$XDG_DATA_HOME/bilinkers/bin` (else `~/.local/share/bilinkers/bin`) on Linux.
- curl does not set `com.apple.quarantine`, so the binary runs without removing that attribute. Verified 2026-11-10 with rust-v0.167.0 on macOS arm64: the 108,379,331-byte archive installed, `codex --version` printed `codex-cli 0.167.0`, and `xattr` listed nothing.
- The browser choice is saved as `BROWSER=` lines in `settings.env` in the platform config dir, written to a temp file and renamed over the old one. Settings are read from the environment, then `./.env`, then `settings.env`; the first non-empty value wins, so a `BROWSER` in `.env` locks the choice. (src/setup.rs, EnvLayers)
- The logger sink only does a non-blocking channel send and wakes the window, so the bot never waits on the UI. (src/ui/mod.rs, src/logger.rs)

## Page detection

The app chrome (header, nav icons, XP badge, the unit row list) is built from the same MUI components on every page. Generic markers such as `MuiButtonBase` or `.bl-w-full` match every page, including the unit list, so they are not used. Only specific markers count, checked from most to least unambiguous.

Order in `src/detect.rs` (detectMode):

1. An iframe is present and its document contains `quiz-input-sa`, `quiz-input-radio`, `quiz-matching`, or `href="#/n"`. Mode `iframe`. The LTI (Bits) player only ever appears inside an iframe.
2. The iframe document contains `bl-w-full justify-content-start`. Mode `reading`. The reading BlExercise (passage plus numbered 1..N tiles, lettered option buttons) is also iframe-embedded but uses the MUI option buttons, not the Bits `quiz-input-*` markup.
3. The body text matches `/Checkpoint/i` and a button's text starts with `Start Attempt` or equals `Continue`. Mode `checkpoint`. A checkpoint gate (for example "ENG-B1.2 - INDEPENDENT (V2) - Checkpoint 1" with Total Question, Passing Score, Maximum Attempt) has the same shape as a plain exercise's BlExercise gate. The word "Checkpoint" is the only difference.
4. A `.hoverable-pointer` element whose whitespace-stripped text matches `^(Unit|Checkpoint)\d+$`. Mode `course`. These roadmap nodes exist on no other page.
5. A `.hoverable-pointer` element whose whitespace-stripped text matches `^[BC]\d\.\d$`. Mode `island` (since 2026-11-10). The course islands exist only on the Beelingua home.
6. The top-level document has the grouped reading layout (see below). Mode `reading`.
7. The top-level document contains `bl-w-full justify-content-start` (the lettered option buttons). Mode `exercise`.
8. Two or more `button.bl-w-full` elements contain a `.bl-text-ellipsis` title. Mode `unit`. Verified against the "Unit 2" activity list, which has exactly this shape, no iframe and no lettered options.

Grouped reading layout: at least two distinct numeric `.bl-button__container` pills, and at least two visible `button.bl-w-full.justify-content-start` buttons whose text starts with "A" (two or more option groups on screen). (src/grouped_reading.rs)

Inside a unit, after a row is opened and its gate passed, `src/runners/unit.rs` (detectMode) decides the runner:

- It polls for 8 s every 200 ms and checks the iframe and the top-level markup on every tick. A native exercise never has an iframe, so waiting for the iframe first used to burn the whole timeout.
- An iframe whose document contains `bl-w-full justify-content-start` is `reading`. Any other iframe is `iframe`.
- Without an iframe, the grouped reading layout is `reading`, and `bl-w-full justify-content-start` is `exercise`.
- Nothing by the deadline means a video or notes row.
- A row titled exactly "Reading" always gets the `reading` runner without sniffing. See [Unit activity list](#unit-activity-list).

## Island map (Beelingua home)

- `https://lms.binus.ac.id/beelingua` shows the eight courses B1.1 to C2.2 as `.hoverable-pointer` nodes. Whitespace-stripped labels match `^[BC]\d\.\d$`. (src/detect.rs, PageMode::Island)
- A JS `click()` on a node does nothing (verified 2026-11-10). A real pointer click (CDP mouse events at the element's center) opens a side panel instead of navigating.
- The panel contains the heading `ENG-<label>`. A **View Unit List** button means the course is unfinished. Only **View Course Detail** means it is complete. Either button navigates to that course's roadmap at `/beelingua/student/class/<id>`.
- An open panel can cover other nodes. Reload the home page before opening the next one.
- Each node's `<img>` has `alt` set to the course's status (verified 2026-11-10). The live map with every course complete showed `alt="done"` on all eight. The site's bundle renders `<img src={icons[status]} alt={course.status}>` with icons for `done`, `locked` and `progress`, so completion is read without clicking. (src/runners/island.rs, CourseState::from_marker)
- The marker mapping is `done` complete, `locked` locked, `progress` or `notStarted` unfinished. Only `done` was seen live; the others come from the bundle. Any other value falls back to the panel.
- On a completed course the panel's buttons are only "View Course Detail" (verified 2026-11-10 on B1.1 with a CDP pointer click, then a reload left 8 nodes on the map).
- A locked node does not open the panel; its click asks the site for "closed" info instead. The island runner never clicks a node whose marker says locked.
- The island runner (`bilinkers island`, or Start on this page) reads B1.1 to C2.2 in label order, opens the first unfinished one through "View Unit List", runs the course runner on it, reloads home and repeats. Each course is opened at most once per run. It stops when a course run is stuck. With nothing unfinished at the start it ends as `all-courses-complete`.

## Course roadmap

- The roadmap is an isometric city map. A zig-zag road of hexagon nodes ("Unit 1", "Unit 2", "Checkpoint 1", "Unit 3", ...) with a header readout like "Unit 4/8  CP 1/3  FT 0/1". Its URL has no `/session/{id}` suffix.
- Each node is a rasterized PNG inside a `.hoverable-pointer` div, with the label as a sibling text node ("Unit8", "Checkpoint3" once whitespace is removed). Colour, lock state and checkmark are baked into the image. No class or attribute shows state. (src/runners/course.rs, readCourseNodes)
- Correction (verified 2026-11-10): roadmap node images also carry `alt` with the unit's status (`alt="done"` on every node of a finished course, the Final Test node included). The bundle sets it from `unit.status` (`locked`, `notStarted`, `progress`, `done`). The course runner still uses the header counts.
- Progress comes from the header instead. `Unit\s*(\d+)\s*/\s*(\d+)` and `CP\s*(\d+)\s*/\s*(\d+)` on `document.body.innerText` give done and total counts. (src/runners/course.rs, readProgress)
- Course order is fixed. Sorting nodes by the `style.top` of their first descendant with `top` in its style, descending, gives course order. Verified live order: Unit1, Unit2, Checkpoint1, Unit3, Unit4, Checkpoint2, Unit5, Unit6, Checkpoint3, Unit7, Unit8. This matches the visual order and the header counts.
- The next node is the first in order whose number is greater than the done count for its type and that has not been tried in this run.
- Labels are whitelisted. Only `Unit N` and `Checkpoint N` are recognised, so the Final Test node is never opened, whatever its label.
- Nodes are opened with a JS `click()` on the `.hoverable-pointer` element. (src/runners/course.rs, clickNode)
- Clicking a locked node never changes the URL (verified live). The runner waits 10 s for a URL change and stops if none comes. This also covers a click that lands on road or decoration. The platform's lock rules win over what the header counters imply.
- Right after navigating back, the roadmap is still loading. A fixed 1500 ms sleep was not always enough (a read found 0 nodes and null progress, then read fine moments later). Nodes and progress are polled together for up to 5 s every 300 ms.
- After a node opens the runner sleeps 1500 ms, runs the unit or checkpoint runner, then returns with `driver.get(courseUrl)` and another 1500 ms sleep.
- The course run stops when a unit returns `stuck`, or a checkpoint returns `unhandled` or `no-gate`.

## Unit activity list

- The unit page is a stack of rows: video, notes (Bee Notes), Interactive Vocabulary or Grammar Activity, Multiple Choice Grammar Activity, Listening, Reading. Each is locked, in progress, or done. Its URL has no `/content/{id}` suffix.
- Each row is a `button.bl-w-full` MUI button with no href (React onClick). The title is in `.bl-text-ellipsis`. (src/runners/unit.rs, readRows)
- A locked row has both the `disabled` attribute and the `Mui-disabled` class. Verified on "Unit 2", where Reading, Listening and others stay locked until the row above is complete.
- A completed row always shows the same checkmark SVG, matched by its path data `M14.2021 1.52344L5.45215 10.273L1.07715 5.89844`. Verified on a done "Interactive Vocabulary Activity" row.
- A JS `click()` on a row sometimes does not trigger navigation. The row is scrolled to the center, then clicked with a real pointer click after 200 ms. (src/runners/unit.rs, clickRow)
- Titles are compared with `textContent.trim()`, never rendered text. Rendered text normalizes whitespace (it collapses the non-breaking space some titles contain), so it never matched the title `readRows` returned.
- After the click the runner waits 10 s for the URL to change.
- Rows already opened in this run are not picked again. A row that cannot be auto-completed would otherwise loop forever.
- The run stops when an activity runner returns `unhandled` or `incorrect`.

Start gate (src/runners/unit.rs, clickStartGate):

- Native and iframe activities both open behind a top-level "Exercise" gate first. URL suffix `/BlExercise?status=0`, Total Question and Passing Score, Cancel and Start buttons, no iframe yet. Verified on "Interactive Grammar Activity". The real content loads only after the click.
- With a prior attempt on record the button reads "Continue" instead of "Start" (verified on a second visit). Both are accepted.
- The button is not mounted when the URL changes (a query right after found zero buttons while React rendered the route). It is polled for 8 s every 300 ms.
- Not every activity has a gate. Not finding one is not an error.
- After the gate click the runner sleeps 1500 ms.

"Reading" rows (src/runners/unit.rs, main):

- A Reading row can mix plain questions with an "Original vs Paraphrased Text" cluster that renders several questions' option groups at once (verified: 5 groups of 5 buttons, told apart only by scroll position, several enabled at once).
- DOM sniffing sometimes calls such a page `exercise`. The native exercise's option click (`querySelectorAll(...).find(startsWith(letter))` in src/question_types/option_buttons.rs) then hits the first matching group in DOM order, not the current one, and burns every retry as incorrect.
- The reading runner scopes clicks per group and handles plain questions too, so the row title "Reading" picks it directly.

Videos and notes (src/runners/unit.rs, completeMediaActivity):

- Video and notes rows have no gate and no questions. They complete by being viewed.
- Every `<video>` on the page is muted and played. Top-level videos are tried first, then each iframe.
- A video counts only once it reaches its end. Without seeking, a 5:24 video held the run for its full length.
- Setting `currentTime` exactly to `duration` never sets `ended` (it stayed false indefinitely). Seeking to `duration - 1` while playing lets the last second play out, and `ended` turns true within about 2 s.
- `duration` may not be known on the first pass while metadata loads, so the check repeats every 300 ms (seek only when `currentTime < duration - 1`, play again if paused).
- The timeout is 15 minutes. One video's metadata loaded so slowly that it fell back to real-time playback of its ~4:44 length and finished seconds after an old 5-minute cap had thrown.
- Notes need nothing beyond opening (300 ms).
- The SPA's onEnded handler POSTs completion to the backend. Navigating away at once (`driver.get`) beat that call and left the row "In Progress" although the client saw `ended: true`. The runner waits 3 s after a video.
- The same race hits quiz activities. "Interactive Vocabulary Activity" scored 14/14 and reached its closing screen, yet showed as incomplete on the next row read. The runner waits 3 s after every finished activity, then `driver.get(unitUrl)` and 1500 ms.

## Native exercise

- Option buttons are `button.bl-w-full.justify-content-start` (`MuiButtonBase-root bl-w-full justify-content-start ...`). Option text is the `bl-rtf undefined` element inside each button, up to `</p></div>`. Letters A to F follow DOM order. (src/question_types/option_buttons.rs, parseOptions)
- The question text is the first `bl-rtf undefined` block on the page, up to `</p></div>`.
- An option is chosen by clicking the button whose text starts with its letter, then the button with text "Check". (src/question_types/option_buttons.rs, selectOptionAndCheck)
- Feedback is an `h5` reading "Correct!" or "Incorrect!", polled for 5 s every 200 ms. A wrong answer can be retried in place.
- An incorrect answer comes with a specific hint in `heading.parentElement.nextElementSibling`. Example on a True/False/Not-Given reading question: "Revisit the passage to determine if Jenna's role in organizing the pantry was mentioned." It is passed to the retry. (src/question_types/option_buttons.rs, waitForCheckFeedback)
- The exercise runner uses no LLM. It tries each option in order until Check says correct (`retryLimit` is options minus one). Listening questions cannot be heard, and with in-place Check feedback, exhausting A to D is cheaper and more reliable than asking a model. (src/runners/exercise.rs, tryEachOption)
- Native exercises never render an iframe, so the DOM read uses a 300 ms iframe wait instead of the 30 s LTI default. (src/runners/exercise.rs, main)
- Advancing: a top-level button with text "Next" (src/browser.rs, goToNextQuestion). If there is none, the pill numbered `questionNum + 1`. If neither exists, the exercise is finished.

Numbered-pill exercises (src/runners/exercise.rs, submitIfPresent):

- Some exercises, for example "Multiple Choice Grammar Activity" (15 questions, pills 1..15 across the top), have no per-question "Next" button. Moving on means clicking the next pill.
- They need an explicit final "Submit" after the last question. Without it every answer can be correct and the exercise still shows as unfinished.
- Submit opens an "Are you sure?" confirm with Yes and No. The runner waits 1 to 1.5 s, clicks "Yes", then waits 1500 ms.
- The result screen reads like "Excellent! You Passed! Your Score: 100" and has its own "Next" button. Next returns to the unit list (the URL drops back to the plain session URL and the progress bar advances).
- The result screen takes longer than a fixed sleep to render (score and confetti animation). The runner polls for 8 s every 300 ms, reading `Your Score:\s*(\d+)` and clicking Next in the same script, so the score comes from the same render as the click.
- When there is no Submit button (the reading and audio flow), having no further Next is already a complete finish.

## LTI iframe player

Loading (src/browser.rs, getCurrentQuestionDom):

- The LTI player injects its content iframe with JS after load. The DOM read polls for an iframe for up to 30 s every 200 ms and uses the first one.
- Opening an activity first shows an "LTI launcher" page inside that iframe, a hidden auto-submitting form with id `ltiForm` that POSTs to the Bits player.
- After that POST, the Bits app shows an empty `<main id="content">` for a moment while its JS fills in the slide.
- In either state the page has no question markers and no `#/n` link, so it used to be read as unhandled. The read waits up to 30 s, every 300 ms, until there is a body, no `ltiForm`, and a `#content` with children.
- Navigation happens inside the iframe's own document, and the `<iframe>` element is not replaced. The iframe is looked up fresh on each retry anyway.
- A resume prompt "Do you want to continue from the page your last visited?" is answered by clicking the parent of the `span` with text "NO".

Page parts:

- Author instructions (task, tense or rule notes, worked examples) are always styled `color:rgb(26, 106, 132)` (a teal) in the Bits editor, whatever the question type or wording. They are extracted by that style instead of by phrases, with tags, `&nbsp;` and zero-width spaces stripped. (src/question_types/lti_quiz.rs, extractInstructionText)
- Answers are submitted with `#quiz-submit-btn` ("Check My Answer").
- `.quiz-next-btn` is the post-answer "Continue" link. It is visible (`display` not `none`) only once a question passes. Its visibility is the trusted "correct" signal.
- The generic `.correct.timeout` and `.incorrect.timeout` banners (src/question_types/lti_quiz.rs, waitForQuizFeedback) do not reliably reflect the result for the `sa`, matching or radio quiz types. See [Question types](#question-types).
- The submit click occasionally does not register with the app's grading. Every LTI type clicks `#quiz-submit-btn` a second time 2.5 s before its deadline.
- A "Question N of M" counter sits in `#content`. It is sometimes stale. On a fillInBlank activity, Continue advanced the hash and the sentence, but the counter stayed frozen. Progress is detected by any change in the full `#content` innerText. (src/runners/iframe.rs, readActivityState)
- The final slide reads "You can now close this activity and continue to the next one". It has no `#/n` link and no quiz markers, so it matches no handler. That is the normal end.
- The SPA hash router uses `#/question-N`. It accepts direct navigation regardless of what is unlocked. After a wrong answer the Check-once UI keeps Continue hidden, and the hash was the only way forward. The bot does not use it; it stops on a still-wrong answer instead. (src/runners/iframe.rs)

Runner flow (src/runners/iframe.rs, main):

- Handler order is fillInBlank, errorAnalysis, quizMatching, vocabularyIntro. `registry.findHandler()` returns the first match.
- Strategy per type: fillInBlank uses the LLM, errorAnalysis tries letter combinations, quizMatching reads the answer from the page, vocabularyIntro clicks through.
- `retryLimit` is `config.retry.maxAnswerRetries` (`MAX_ANSWER_RETRIES`, default 25) for LLM and letter-combination types, and 0 for quizMatching.
- Vocabulary slides are clicked through without counting them as questions.
- A 100 to 200 ms reading delay comes before each answer so submissions do not land instantly.
- A question still incorrect after all retries stops the run instead of advancing past it.
- Advancing clicks a visible `.quiz-next-btn`, then waits up to 60 s for `#content` to change or the closing text to appear. No Continue link means the activity is complete. Between questions the runner sleeps 1.5 to 3 s.
- The on-screen instruction is the only source of truth for the task (wording and the graded rule vary per question). It is sent to the LLM verbatim, plus a per-type format note.

## Grouped reading exercise

- Layout: numbered 1..N tile pills (`.bl-button__container`), the passage on the left, option-button sub-questions on the right. Verified on "ENG-B2.2 Unit 1" (15 questions, mixed reading comprehension, true/false/not-given and paraphrase items). (src/runners/reading.rs)
- Solved by brute force with no LLM.
- A wrong answer re-enables its options. A correct answer locks them (all disabled). So each question is cycled A, B, C, D until it sticks, and "all options locked" can only mean correct.
- Each tile's background colour is the per-question verdict, updated when a Check or a pill transition grades the answer. It works for every item type.
  - correct: `rgba(54, 192, 203, .3)` (faint teal)
  - wrong: `rgba(232, 166, 38, .3)` (faint orange)
  - unanswered: `rgb(127, 202, 212)` (solid teal)
  - current: solid orange
- The runner does not read these colours today. It reads option `disabled` state.
- Options are one flat list of visible `button.bl-w-full.justify-content-start`. A new group starts each time a button's letter is "A". (src/runners/reading.rs, answerOpenGroups)
- Tried letters are remembered per group, keyed by pill number, group index and the first 60 characters of the group's first option. The key survives navigation within a run and does not collide on repeated option text.
- After a pill click the runner waits 800 ms. A pill with no enabled options is already correct.
- Some questions have a per-question button matching `/^Check( My Answer)?$/i`. Those grade in place. The runner picks a letter, waits 400 ms, clicks Check, waits 1400 ms, and reads an `h1` to `h6` reading "Correct!" or "Incorrect!". "Incorrect!" leaves the options enabled, so it moves to the next letter (A to F). (src/runners/reading.rs, answerCheckQuestion)
- Questions without Check are graded by leaving them. The runner clicks the next pill (then waits 1000 ms) and sees the lock on a later pass.
- Submit is gated. Clicking Submit (then 1200 ms) shows "All answers must be correct" while any item is wrong. The runner then clicks "Close" (600 ms) and keeps fixing. (src/runners/reading.rs, trySubmit)
- With everything correct, Submit opens an "Are you sure?" confirm. Clicking the visible "Yes" (then 3000 ms) records the attempt. With neither the guard nor Yes, the runner clicks Close and treats it as not done.
- The result screen shows "You Passed! Your Score: 100" and a "Next" button back to the unit list. The runner polls 8 s every 300 ms, reading the score and clicking Next.
- Passes are capped at 12. A pass that answered nothing while Submit still refuses means a locked item is wrong, which breaks the re-enable assumption, so the run stops as `stuck`.
- The runner reads the DOM with a 300 ms iframe wait. `src/detect.rs` and `src/runners/unit.rs` find this layout both inside an iframe and at top level. The source comment called it a native MUI exercise with no iframe.

## Checkpoint

Gate and attempts (src/runners/checkpoint.rs):

- Gate example: "ENG-B1.2 ... Checkpoint 1", Total Question 30, Passing Score 100, Maximum Attempt Unlimited.
- The gate button reads "Start Attempt 1" on a fresh checkpoint, "Start Attempt N" later, or "Continue" when an attempt is in progress (verified live). Matched by prefix `Start Attempt` or exact `Continue`, polled for 8 s every 300 ms. Then the runner sleeps 1500 ms. (clickGateButton)
- The platform allows unlimited attempts. The runner retries the whole checkpoint from the gate without limit until the score is 100. Letters cycle A to F and wrap around.
- The run stops early only when there is no gate button (`no-gate`), on a structural problem (`unhandled`, meaning no pills or pill navigation that does not settle), or when Submit fails three times in a row (`submit-failed`). Retrying an unhandled page would only hit it again.
- Between attempts the runner returns with `driver.get(gateUrl)` and 1500 ms.

Brute force across attempts:

- Checkpoints never show a per-question "Correct!" or "Incorrect!". There is only the final score, so a question cannot be brute-forced in place.
- Each attempt uses one shared letter for every unlocked pill (A, then B on the next attempt, and so on). Correctness is known only on the next attempt. Every pill stays visible, but correct ones are locked and only still-wrong ones are editable (verified live). Locked pills are skipped, so their correct answers are kept. (answerAllQuestions)
- On each pill the runner clicks every enabled option button whose text starts with the letter (all groups on that question).
- A pill with fewer options than the letter (for example a 3-option TFNG question on an "E" round) has nothing to click. A later letter may fit.

Pills:

- Each nav pill is a `.bl-button__container` with a 1 or 2 digit label. Pills are found by class, not by background colour. A colour whitelist missed the tan-bordered ones and found only 16 of 30. (readPills)
- Only 15 pills render at first. An unlabeled down-chevron button (its `path` `d` starts with `M16.59 8.59L12 13.17`) right of the pill grid and within its vertical range expands the rest. Success is checked by the pill count growing within 3 s. (ensurePillsExpanded)
- `success-shade-color` marks a locked (correct) pill. Such a pill never gains `secondary-shade-color` when clicked, because the app does not render into a locked question. It is skipped before navigating, instead of being treated as a failed pill transition. (isPillLocked)
- `secondary-shade-color` marks the active pill. A click counts once the pill turns active within 5 s. (waitForPillActive)
- `primary-light-shade-color` marks an unanswered pill. (waitForPillSaved)
- Pills are clicked via `(el.closest('button') || el).click()`. A pill click occasionally does not register (a pill reported "did not settle", then worked on a plain re-click seconds later). It is retried once.
- After the gate click, pills can render late. One fixed sleep found zero pills, while a re-check seconds later on the same page found 25 or more. Pills are polled for 8 s every 300 ms.
- The attempt page occasionally shows "Failed to load Assessment type / Please contact Beelingua Support" instead of the attempt (seen after resuming an in-progress attempt). One page reload (then 2000 ms) clears it. A second failure is treated as real.
- Questions render top-level, so the DOM read uses a 600 ms iframe wait.
- An active pill does not mean the options have rendered. They can be momentarily all disabled, which looks the same as a locked question. Enabled options are polled for 2 s every 150 ms before a pill is treated as locked.

Save and Submit:

- Moving on by pill click alone sometimes left the just-answered question unsaved (question 30's answer was missing at Submit). Every answer is followed by a click on "Save" or "Save & Next" (label varies by attempt type). Never "Submit". (clickSaveButton)
- Save fires a network call. A pill click before it lands can be swallowed by the save's re-render, which stalls on a later, unrelated pill (a race, not a bad pill). The runner waits up to 5 s for the saved pill to lose `primary-light-shade-color`. (waitForPillSaved)
- After a pass, unanswered pills are counted by `primary-light-shade-color`. Up to 3 completion passes run. When a pass makes no progress, or the last pass ends, the runner submits what is answered. Returning `unhandled` here used to kill a converging checkpoint (score 40, then 60) on attempt 6.
- "Save & Next" jumps to the next unanswered question, so after the passes the page may not be on the last pill. The runner clicks the last pill first.
- The last question has no "Save & Next". It shows a per-question "Save" and the checkpoint-wide "Submit" side by side. "Save" comes first in DOM order and only re-saves. "Submit" exists only on the last question's screen and is the only finalizer. (clickSubmit)
- Submit can be briefly disabled while the last save's network call settles. The runner polls up to 4 s for an enabled Submit (it may also not be rendered yet).
- Submit leads to an "Are you sure?" Yes and No confirm (as in the native exercise, but with no "Next" afterwards). A dropped Submit click leaves the page unchanged with no modal and no error. The runner polls 8 s for "Yes". If no confirm appears, it clicks Submit again, up to 3 clicks.
- When Submit fails, the per-pill answers are already saved. A retry from the gate ("Continue") resumes the same attempt and only needs to land Submit.

Result:

- Confirming Submit lands on `.../result/version/{id}` directly (no `/review` needed). The text reads like "...Sorry, Try Again.Your Score:83Correct25Incorrect5No Answer0...". The number follows "Your Score:" with no separator (a run that missed 5 of 30 read "Your Score:83"). The runner matches `Your Score:(\d+)` for up to 10 s. (waitForScore)

Passed checkpoint gate (verified live 2026-11-10, C2.2 Checkpoint 1):

- The node opens `.../session/{id}/content/{id}/BlCheckpoint?status=2`.
- The gate shows the title, Total Question 30, Passing Score 100, Maximum Attempt Unlimited, the assessment description, and a History block (Attempted 5, Score 100, Completion Date, Duration).
- Its only buttons are "Cancel" and "Detail". There is no "Start Attempt N" and no "Continue", so a passed checkpoint cannot be re-attempted and the runner stops with `no-gate` after the 8 s poll (seen with the Rust binary).

## Final test

- A Final Test attempt has 50 questions and exactly 1 attempt, with a passing score of 66. It uses the same pill-nav native quiz DOM as a checkpoint. Option rows are `button.bl-w-full.justify-content-start`. (src/runners/final_test_runner.rs)
- Pill helpers (`readPills`, `clickPill`, `ensurePillsExpanded`, `clickSaveButton`, `waitForPillSaved`) are shared with src/runners/checkpoint.rs.
- The pill nav re-collapses to a default view of about 12 pills after landing on some questions (seen after Q11 and Q12). Expanding once before the loop is not enough, so it re-expands before every click attempt.
- A failed pill click used to be ignored. The run then re-read the same question for every later pill with no error. Each pill click is tried up to 3 times, then the run stops as `stuck`. (scrapeFinalTest)
- A question still rendering gives different content on two reads spaced apart. A settled one reads the same. Comparing to the previous question's DOM did not work, because outerHTML noise (ids, timestamps) made it differ at once. The runner requires two consecutive reads 300 ms apart to match, for up to 5 s, with a 600 ms iframe wait.
- By default it is read-only. Only pill navigation touches the page. No option, Save or Submit is clicked. A Final Test has one attempt, so there is no room to brute-force.
- Every question goes into `final-test-answers.md` (a Markdown key) and `final-test-answers.json` (answer, confidence, agreements, question text, options).
- Listening questions (audioMultipleChoice) are skipped since the bot cannot hear the audio. With `FINAL_TEST_GUESS_LISTENING=1` each gets a random option clicked and saved instead. A blank scores 0, while a random guess is right about 25% of the time, which helps when only the pass mark matters. The guess is still written to the key.
- With `FINAL_TEST_AUTO_ANSWER=1`, an answer is clicked (visible option starting with the letter, no Check) and saved on the spot when its confidence is at least `FINAL_TEST_CONFIDENCE_THRESHOLD` (default 0.9) and its agreement count is at least `FINAL_TEST_REVIEWS_REQUIRED` (default 1). The rest stay in the key for manual answering. An overconfident click cannot be undone.
- Each question gets a single LLM answer with a self-assessed confidence (`{ "answer": "<letter>", "confidence": <0..1> }`). There is no per-question feedback to loop on. Up to 3 calls (1 s apart) cover transient failures only. After that the question is marked `NEEDS REVIEW` so one bad question does not abort the other 49. (answerWithRetry)

Multi-pass review:

- A later run over the same test loads the previous answers and re-derives each question independently. The prompt names the previous suggestion, says not to assume it is correct, and asks for confidence in the model's own judgment only. Keeping the prompt neutral keeps the review independent. (reviewInstruction)
- A single self-assessed confidence is unreliable. Two passes agreeing set confidence to `min(0.99, max(previous, current) + 0.2)` and add 1 to `agreements`. A disagreement records the new answer with confidence `min(previous, current) * 0.5` and resets `agreements` to 1. `NEEDS REVIEW` on either side keeps the other side's record. (combineReview)
- The json stores `testId`, the attempt URL without its `#/question-N` hash (the hash changes every pill). A json from a different test is reported as stale, ignored and overwritten. (testIdFromUrl, loadPreviousAnswers)

## Question types

Each handler exposes `detect`, `parse`, `answer` and `checkResult` (src/questionTypes/). Native types read the top-level page. LTI types require `dom.insideIframe`.

### audioMultipleChoice

- Detected by a visible `.MuiSlider-root` audio player plus `bl-w-full justify-content-start` option buttons. (src/question_types/audio_multiple_choice.rs)
- A past listening question's `MuiSlider-root` player stays mounted, only hidden, after the SPA moves on. A substring match on outerHTML would keep matching audioMultipleChoice forever after the first listening question. `hasVisibleSlider` counts only sliders with `offsetParent !== null` (not inside a `display:none` subtree). (src/browser.rs, getCurrentQuestionDom)
- Without `hasVisibleSlider` (fixture DOMs with no live visibility) detection falls back to the substring check.
- The instruction is the first `<strong>` in the first `bl-rtf undefined` block. The question is the next `<p>`.
- Answered and checked like any native option question. See [Native exercise](#native-exercise).

### readingComprehension

- Detected by option buttons and no visible slider. A stale hidden slider must not disqualify a real reading question. (src/question_types/reading_comprehension.rs)
- The text is the first `bl-rtf undefined` block with tags stripped.
- Answered and checked like any native option question, including the hint on "Incorrect!".

### fillInBlank

- Detected inside the iframe by `quiz-input-sa` (short answer). The blank count is the number of `quiz-input-sa` occurrences. (src/question_types/fill_in_blank.rs)
- The sentence is the first `p`, `div`, `li` or `td` block whose text has `__` and that contains `<strong>` or `<b>`. Fallback is the first bold chunk with `__`, plus the next bold chunk when it starts with `(` or `[`.
- Each `.quiz-input-sa` gets its value set, then `input` and `change` events (bubbling). Extra inputs reuse the first answer. Then `#quiz-submit-btn` is clicked after 100 to 200 ms.
- The `.incorrect` banner is noise for this type. A correct answer ("were rainy", confirmed by Continue going live and the input keeping it) still flashed `.incorrect` for about 2 s, with the same timing as a real wrong answer.
- Only a visible `.quiz-next-btn` counts as correct. Anything else by the 5 s deadline is `incorrect` (no exception), since this type has no reliable wrong-answer signal. Submit is re-clicked once 2.5 s before the deadline.
- The on-screen worked example shows the answer inside a full sentence (for example "Answer: ... I will be travelling to Surabaya."). Without a format note the model copied that shape, and an input held "I think he is going to win the race..." instead of "is going to win". The note asks for only the blank words, `{"answers": [...]}` in order for several blanks. (src/runners/iframe.rs, FORMAT_NOTES)

### errorAnalysis

- Detected inside the iframe by `quiz-input-radio`. (src/question_types/error_analysis.rs)
- Candidate words are underlined. Some questions wrap each in `<b><u>..</u></b>`, others use a bare `<u>` (seen on the "Although the intern tried..." A to E question). Bold-wrapped underlines are used when present, else every `<u>`.
- Candidates are marked inline, for example `...help [D]they[/D] improve...`. A plain word list loses position when a word repeats ("their" in both "their job" and "their skills", only one a candidate).
- Some templates mark candidates with absolutely positioned overlay labels (`data-options` with `label: A`, one per `.quiz-input-radio`) and no `<u>` at all ("By the time the guests arrived..."). The word cannot be recovered without layout math, but only the letters matter.
- Candidates are padded with empty words up to the `.quiz-input-radio` count. One question had 5 radios but only 4 `<u>` tags. Only A to D were tried, and E was the correct answer.
- The page grades only one submit per page load. After "Check My Answer", every radio is `disabled` and further clicks do nothing (seen on a graded question with Continue showing). Unlike the mc and sa types, it does not re-grade. Before each answer, if all radios are disabled, the runner reloads the page, waits for the player, restores `window.location.hash`, and waits 1500 ms. (resetIfLocked)
- `.quiz-input-radio` is not an exclusive radio group. Several checks can show at once. The click script sets exactly the requested letters and unchecks the rest.
- Right after a fresh activity opens, the radios may not be rendered yet. A throw there skipped the letter for good, because the brute-forcer advances on any failure (a correct A was skipped while B to E were tried). Radios are polled for 8 s every 300 ms.
- Correct means `.quiz-next-btn` is visible, or no `.quiz-input-radio` is left. The banners are unreliable here too (a correct E, with Continue visible and the radio locked, timed out waiting for them). Anything else by 5 s is `incorrect`, and the next attempt's reload handles the lock. Submit is re-clicked once 2.5 s before the deadline.
- Solved by brute force, not an LLM. Every non-empty letter subset is tried in increasing size (A, B, ..., A+B, A+C, ...), one per retry. There are only a handful of candidates. A fresh answerer per question keeps the counter from leaking. (src/runners/iframe.rs, createBruteForceAnswerer)

### quizMatching

- Detected inside the iframe by `quiz-matching`, `dropzone` and `draggable` all present. (src/question_types/quiz_matching.rs)
- A drag-and-drop word bank. The app's grading (`checkQuizMatching` in app.js) only checks `dropzone.data-filled === dropzone.data-drag-target`. The right draggable for a blank has that index as its `data-drag`. Distractors have indexes that match no blank.
- Simulated mouse drags did not register with interact.js (the dropzone stayed empty and Check stayed disabled). The handler writes the end state the app's onDrop produces instead. It centers each draggable on its dropzone in `%` units, sets `data-filled`, and removes `disabled` from `#quiz-submit-btn`. The grading code cannot tell the difference.
- The answer comes from the page, so `retryLimit` is 0.
- After a correct fill and submit, both `.timeout` banners stayed `display:none` for the whole 5 s while `.quiz-next-btn` went visible at once. Correct means a visible `.quiz-next-btn`. A visible `.incorrect.timeout` or `.incorrect` means incorrect.
- The submit click sometimes did not register (fill correct, `#quiz-submit-btn` still enabled, timeout). A second click flipped `.quiz-next-btn` visible at once. The wait is 20 s with one re-click 2.5 s before the end. A timeout throws.

### vocabularyIntro

- Vocabulary presentation pages (word cards, meaning and example cards, instruction screens) sit between quiz pages in the same iframe SPA. They are not graded and have no options. (src/question_types/vocabulary_intro.rs)
- A right-arrow link with `href="#/n"` walks the slide sequence forward (word card, then meaning/example card, then the next word).
- The visible "Continue" button on these pages sometimes has a stale href equal to the page's own hash (an authoring template leftover) and never advances. So `a[href="#/n"]` is clicked instead.
- Quiz pages also carry a `#/n` link. It is their post-answer Continue, hidden with `style="display:none"` until answered. Matching on the href alone skipped real questions. Pages with `quiz-input-radio` or `quiz-input-sa` are excluded.
- The matching exclusion uses quizMatching's full condition (`quiz-matching`, `dropzone`, `draggable`). Intro slides embed the activity's quiz manifest as JSON, which contains literal `quiz-matching-N` ids, so a bare `quiz-matching` check excluded intro slides too.
- `checkResult` is always `correct`. The runner does not count these slides as questions.

## LLM and the answer loop

- The default provider is `codex`. Each question runs `codex exec --ephemeral --sandbox read-only -o <file> -` with the prompt on stdin, so the call uses the user's own `codex login` (ChatGPT OAuth) and no key is stored. About 8 to 10 s per call. Verified live 2026-11-10 on C2.2 Unit 1 Interactive Grammar Activity (5 of 5 fill-in-blank correct on the first try). (src/llm.rs, askCodex)
Provider calls (src/llm.rs):

- `LLM_PROVIDER` is `openai` (default, any OpenAI-compatible endpoint via `OPENAI_BASE_URL`) or `gemini`. A Gemini client is detected by having `getGenerativeModel`, since it has no `chat.completions` API.
- The user prompt is JSON of `{ instruction, questionData, feedback }`. The system prompt asks for strict JSON only, `{ "answer": ... }` or `{ "answers": [...] }`.
- `response_format: json_object` is not set. One provider returns a 502 for the whole request when it is. JSON is enforced only by the system prompt.
- Models sometimes wrap the reply in a `` ```json `` fence anyway. It is stripped before parsing.

Transient failures (src/answer_loop.rs, callWithRetry):

- Each LLM call gets up to 3 tries, 1 to 2 s apart. An OpenAI-compatible proxy intermittently returned 403 with a voucher or billing error, then succeeded on the identical next call seconds later.
- This is separate from the question-level loop, so a blip does not use up an answer attempt without clicking anything new.

Question loop (src/answer_loop.rs, processQuestion):

- The loop runs up to `retryLimit + 1` attempts, 100 to 200 ms apart, and stops at the first `correct`.
- `checkResult` returns `'correct'`, `'incorrect'`, or `{ outcome, hint }` when the page explains a wrong answer. Any other value is an error.
- After a wrong answer the feedback is "previous answer was wrong. Hint: <hint>" or "previous answer was wrong, try again.", followed by every answer already tried and confirmed wrong.
- The tried-answers list exists because some pages show the same static hint whatever wrong option was picked (a True/False/Not-Given question gave the same hint after both "True" and "False"). Each retry is a stateless LLM call with no memory, so without the list the model can repeat a wrong guess.
- An exception during an attempt counts as an incorrect attempt with feedback "previous attempt failed with an error: <message>".
- `MAX_ANSWER_RETRIES` sets `config.retry.maxAnswerRetries` (default 25). (src/config.rs)
- Only fillInBlank (src/runners/iframe.rs) and the Final Test (src/runners/final_test_runner.rs) call the LLM. The native exercise, grouped reading, checkpoint, errorAnalysis and quizMatching are solved without it.

## Question API

- Beelingua's native pages load their questions as JSON from `https://apim-bl-prod.azure-api.net/assessment-fe/Assessment/...`. Seen live 2026-11-14 on the C2.2 Unit 2 Interactive Vocabulary Activity review (`.../Version/<id>/Student/Review`), captured with `bilinkers dump`. (src/dump.rs)
- Each item in `question` carries `no`, `questionText` (HTML with `____` for the blank), `questionType` (for example "BiTS Vocabulary"), `answerType` (for example "Single Answer"), `isCorrect`, `selectedAnswerMultipleChoice` (`index`, `text`) and `feedback`. `questionCategory` is not reliable. That vocabulary item says "Listening".
- The review lists the submitted answer for every question, so after one attempt it is an answer key.
- An attempt in progress loads `.../Assessment/<assessmentId>/Class/<classId>/Student/Question`. Each item adds `answer` (the choices, `id` and HTML `text`), `correctFeedback`, `incorrectFeedback` and `remainingAttempt`, and `selectedAnswerMultipleChoice` holds a saved draft answer. Seen live on the C2.2 Checkpoint 1 draft (30 questions, all "Multiple Choice").
- A read-only crawl of every review on one account, C1.1 to C2.2 (2103 questions, 2026-11-14), found exactly four `questionType` values, all with `answerType` "Single Answer". "Multiple Choice" (1458, checkpoints, final tests, listening, reading and multiple-choice grammar). "BiTS Vocabulary" (385, Interactive Vocabulary, a word bank for a `____` blank). "BiTS Error Recognition" (130) and "BiTS Short Answer" (130, typed text in `selectedAnswerText`), both in Interactive Grammar. The BiTS types are the old LTI iframe kinds (quizMatching, errorAnalysis, fillInBlank), now also rendered natively with numbered pills, Skip and Check.
- The API needs the page's `Authorization: Bearer` header plus role headers (`rOId`, `beelinguarOId`, `beelinguaRoleId`, `roleId`, `roleName`, `academicCareer`, `institution`). The walk is `course-fe/Course/List/Student`, `course-fe/ClassSession/Class/<classId>/List/Student` (`unitGroups[].units[].unitId`), `course-fe/Class/<classId>/ClassSession/<unitId>/ContentProgress` (`listContent[]`), `assessment-fe/Assessment/Class/<classId>/ClassSessionContent/<contentId>/Student/OpeningPage` (`maxAttempt`, `history[].versionId`), then the Review.
- chromiumoxide 0.9.1 drops a whole `Network.responseReceived` it cannot deserialize, which hid this API on Chrome 157 (Helium). The dump reads Network events as raw JSON over a non-flattened session to the tab's own target instead. (src/dump.rs, Recorder)

Native questions without option buttons (src/runners/native.rs):

- Detection: a top-level button whose text is "Check" marks a native question page, mode `exercise`. Lettered option pages also have one but match earlier on their option buttons. Checkpoints use Save, not Check.
- The exercise runner keeps its lettered-option path. A question it does not recognise sends it to the question API: the tab is reloaded once while a raw session listens for `/Student/Question`, then the runner waits up to 15 s for Check, clicking Start or Continue if the reload landed on the gate. Verified live on the C2.2 Checkpoint 1 draft (30 questions read, the on-screen question matched).
- The question on screen is found by its sentence: every piece of the API text around the blank must appear in the page's innerText. A blank is any run of three or more underscores (checkpoints write `___________`).
- Answers are tried best first (src/question_api.rs, candidates). Choices that `correctFeedback` names go first, then the rest. A short answer tries the phrases quoted in `correctFeedback`, then asks the LLM up to `MAX_ANSWER_RETRIES` times.
- A choice is clicked as the deepest visible element whose whole text is that word, preferring one inside a button. A short answer is typed into the first visible text field through the native value setter. Then Check, and the same `Correct!` / `Incorrect!` heading as the lettered options. None of this has been verified on a live BiTS page yet, since the test account has finished all of them. A word or field that is not found stops the activity as `unhandled` and saves the page.
