//! Port of `src/runners/checkpoint.js`.
//! Site behavior this relies on: docs/platform-notes.md#checkpoint

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::{CheckpointOutcome, CheckpointReport};
use crate::browser::{get_current_question_dom, DomWait, Driver, FramedPage, Lms, PageResult};
use crate::js;
use crate::logger;
use crate::question_types::{QuestionKind, Registry};

/// One letter per attempt, cycling A..F. Attempts repeat without limit until the score is 100.
pub const LETTERS: [char; 6] = ['A', 'B', 'C', 'D', 'E', 'F'];

pub const GATE_WAIT: Duration = Duration::from_millis(8000);
pub const EXPAND_WAIT: Duration = Duration::from_millis(3000);
pub const PILL_ACTIVE_WAIT: Duration = Duration::from_millis(5000);
pub const PILL_SAVED_WAIT: Duration = Duration::from_millis(5000);
pub const SCORE_WAIT: Duration = Duration::from_millis(10000);
pub const MAX_COMPLETION_PASSES: u32 = 3;
const PILLS_WAIT: Duration = Duration::from_millis(8000);
const ENABLED_OPTIONS_WAIT: Duration = Duration::from_millis(2000);
const MAX_SUBMIT_FAILURES: u32 = 3;

// "Start Attempt N" for a new attempt, "Continue" for one in progress.
const GATE_BUTTON: &str = r#"
      const buttons = Array.from(document.querySelectorAll('button'));
      const btn = buttons.find((b) => {
        const text = b.textContent.trim();
        return text.startsWith('Start Attempt') || text === 'Continue';
      });
      if (btn && !btn.disabled) { btn.click(); return true; }
      return false;
    "#;

const SUBMIT_BUTTON: &str = r#"
        const buttons = Array.from(document.querySelectorAll('button'));
        const btn = buttons.find((b) => b.textContent.trim() === 'Submit');
        if (btn && !btn.disabled) { btn.click(); return true; }
        return btn ? false : null;
      "#;

const CONFIRM_YES: &str = r#"
        const buttons = Array.from(document.querySelectorAll('button'));
        const btn = buttons.find((b) => b.textContent.trim() === 'Yes');
        if (btn && !btn.disabled) { btn.click(); return true; }
        return false;
      "#;

// The score follows "Your Score:" with no separator, e.g. "Your Score:83Correct25".
const SCORE: &str = r#"
      const match = document.body.textContent.match(/Your Score:(\d+)/);
      return match ? Number(match[1]) : null;
    "#;

// Pills are counted by class, not colour. A colour filter missed the tan-bordered ones.
pub const READ_PILLS: &str = r#"
    const seen = {}, out = [];
    for (const el of document.querySelectorAll('.bl-button__container')) {
      const t = el.textContent.trim();
      if (!/^\d{1,2}$/.test(t)) continue;
      const n = Number(t);
      if (n in seen) continue;
      seen[n] = 1;
      out.push({ n });
    }
    return out.sort((a, b) => a.n - b.n);
  "#;

pub const EXPAND_CHEVRON: &str = r#"
    const pills = [...document.querySelectorAll('.bl-button__container')]
      .filter((el) => /^\d{1,2}$/.test(el.textContent.trim()));
    const rects = pills.map((el) => el.getBoundingClientRect()).filter((r) => r.width && r.height);
    if (rects.length === 0) return false;

    const right = Math.max(...rects.map((r) => r.right));
    const top = Math.min(...rects.map((r) => r.top));
    const bottom = Math.max(...rects.map((r) => r.bottom));
    const buttons = [...document.querySelectorAll('button')].filter((button) => {
      const path = button.querySelector('path')?.getAttribute('d') || '';
      const rect = button.getBoundingClientRect();
      return path.startsWith('M16.59 8.59L12 13.17') && rect.width && rect.height && rect.left >= right && rect.top >= top - 10 && rect.top <= bottom;
    });
    if (buttons[0] && !buttons[0].disabled) { buttons[0].click(); return true; }
    return false;
  "#;

const CLICK_PILL: &str = r#"
    const target = String(arguments[0]);
    const el = [...document.querySelectorAll('.bl-button__container')].find((e) => e.textContent.trim() === target);
    if (el) { (el.closest('button') || el).click(); return true; }
    return false;
  "#;

const PILL_LOCKED: &str = r#"
    const target = String(arguments[0]);
    const pill = [...document.querySelectorAll('.bl-button__container')].find((el) => el.textContent.trim() === target);
    return pill?.classList.contains('success-shade-color') || false;
  "#;

const PILL_ACTIVE: &str = r#"
      const target = String(arguments[0]);
      const pill = [...document.querySelectorAll('.bl-button__container')].find((el) => el.textContent.trim() === target);
      return pill?.classList.contains('secondary-shade-color') || false;
    "#;

const SAVE_BUTTON: &str = r#"
    const buttons = Array.from(document.querySelectorAll('button'));
    const btn = buttons.find((b) => {
      const t = b.textContent.trim();
      return t === 'Save' || t === 'Save & Next';
    });
    if (btn && !btn.disabled) { btn.click(); return true; }
    return false;
  "#;

const PILL_SAVED: &str = r#"
      const target = String(arguments[0]);
      const pill = [...document.querySelectorAll('.bl-button__container')].find((el) => el.textContent.trim() === target);
      return pill ? !pill.classList.contains('primary-light-shade-color') : false;
    "#;

const ANSWER_EVERY_GROUP: &str = r#"
    const letter = String(arguments[0]);
    const buttons = [...document.querySelectorAll('button.bl-w-full.justify-content-start')]
      .filter((button) => button.textContent.trim().startsWith(letter) && !button.disabled);
    for (const button of buttons) button.click();
    return buttons.length;
  "#;

pub const ENABLED_OPTIONS: &str =
    "return [...document.querySelectorAll('button.bl-w-full.justify-content-start')].filter((b) => !b.disabled).length";

pub const UNANSWERED_PILLS: &str = r#"
      return [...document.querySelectorAll('.bl-button__container')]
        .filter((pill) => /^\d{1,2}$/.test(pill.textContent.trim()) && pill.classList.contains('primary-light-shade-color'))
        .map((pill) => Number(pill.textContent.trim()));
    "#;

const FAILED_TO_LOAD: &str = "return document.body.innerText.includes('Failed to load Assessment type')";

fn is_true(value: &Value) -> bool {
    *value == json!(true)
}

pub async fn click_gate_button<D: Driver>(driver: &D, timeout: Duration) -> PageResult<bool> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if is_true(&driver.execute_script(GATE_BUTTON, &[]).await?) {
            return Ok(true);
        }
        driver.sleep(300).await;
    }
    Ok(false)
}

/// What one `clickSubmit` call ended with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Submit {
    Confirmed,
    NeverRendered,
    StayedDisabled,
    /// Every Submit click landed without the Yes confirm ever appearing.
    NoConfirm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubmitOptions {
    pub max_submit_clicks: u32,
    pub confirm_wait: Duration,
    pub enabled_wait: Duration,
}

impl Default for SubmitOptions {
    fn default() -> Self {
        SubmitOptions {
            max_submit_clicks: 3,
            confirm_wait: Duration::from_millis(8000),
            enabled_wait: Duration::from_millis(4000),
        }
    }
}

/// Submit exists only on the last question, next to a Save that does not finalize.
/// Clicks can drop silently, so retry until the Yes confirm appears.
pub async fn click_submit<D: Driver>(driver: &D, options: SubmitOptions) -> PageResult<Submit> {
    for submit_attempt in 1..=options.max_submit_clicks {
        // Submit can stay disabled briefly while the last save settles.
        let deadline = Instant::now() + options.enabled_wait;
        let mut clicked = Value::Bool(false);
        while Instant::now() < deadline {
            clicked = driver.execute_script(SUBMIT_BUTTON, &[]).await?;
            if js::truthy(&clicked) {
                break;
            }
            driver.sleep(200).await;
        }
        if !js::truthy(&clicked) {
            let fields = [("submitAttempt", json!(submit_attempt))];
            if clicked.is_null() {
                logger::warn("Checkpoint: Submit button never appeared, cannot submit", fields);
                return Ok(Submit::NeverRendered);
            }
            logger::warn("Checkpoint: Submit button stayed disabled, cannot submit", fields);
            return Ok(Submit::StayedDisabled);
        }

        let confirm_deadline = Instant::now() + options.confirm_wait;
        while Instant::now() < confirm_deadline {
            if is_true(&driver.execute_script(CONFIRM_YES, &[]).await?) {
                return Ok(Submit::Confirmed);
            }
            driver.sleep(200).await;
        }

        // No confirm, so the Submit click dropped. Retry.
        driver.sleep(300).await;
    }
    Ok(Submit::NoConfirm)
}

pub async fn wait_for_score<D: Driver>(driver: &D, timeout: Duration) -> PageResult<Option<u64>> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let score = driver.execute_script(SCORE, &[]).await?;
        if !score.is_null() {
            return Ok(Some(js::to_number(Some(&score)) as u64));
        }
        driver.sleep(300).await;
    }
    Ok(None)
}

pub async fn read_pills<D: Driver>(driver: &D) -> PageResult<Vec<u32>> {
    let pills = driver.execute_script(READ_PILLS, &[]).await?;
    let pills = pills.as_array().map(Vec::as_slice).unwrap_or_default();
    Ok(pills.iter().filter_map(|p| p["n"].as_u64()).map(|n| n as u32).collect())
}

/// Only 15 pills render at first. The unlabeled chevron right of the grid expands the rest.
pub async fn ensure_pills_expanded<D: Driver>(driver: &D, timeout: Duration) -> PageResult<bool> {
    let before = read_pills(driver).await?.len();
    if before == 0 {
        return Ok(false);
    }
    if !is_true(&driver.execute_script(EXPAND_CHEVRON, &[]).await?) {
        return Ok(false);
    }
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if read_pills(driver).await?.len() > before {
            return Ok(true);
        }
        driver.sleep(100).await;
    }
    Ok(false)
}

fn pill_arg(n: u32) -> [Value; 1] {
    [json!(n.to_string())]
}

pub async fn click_pill<D: Driver>(driver: &D, n: u32) -> PageResult<bool> {
    Ok(is_true(&driver.execute_script(CLICK_PILL, &pill_arg(n)).await?))
}

/// Locked (correct) pills never turn active when clicked, so skip them before navigating.
pub async fn is_pill_locked<D: Driver>(driver: &D, n: u32) -> PageResult<bool> {
    Ok(is_true(&driver.execute_script(PILL_LOCKED, &pill_arg(n)).await?))
}

pub async fn wait_for_pill_active<D: Driver>(driver: &D, n: u32, timeout: Duration) -> PageResult<bool> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        ensure_pills_expanded(driver, EXPAND_WAIT).await?;
        if is_true(&driver.execute_script(PILL_ACTIVE, &pill_arg(n)).await?) {
            return Ok(true);
        }
        driver.sleep(100).await;
    }
    Ok(false)
}

/// Pill navigation alone sometimes left an answer unsaved, so Save after each answer.
pub async fn click_save_button<D: Driver>(driver: &D) -> PageResult<bool> {
    Ok(is_true(&driver.execute_script(SAVE_BUTTON, &[]).await?))
}

/// Wait for the save to land. A pill click during its re-render can be swallowed.
pub async fn wait_for_pill_saved<D: Driver>(driver: &D, n: u32, timeout: Duration) -> PageResult<bool> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if is_true(&driver.execute_script(PILL_SAVED, &pill_arg(n)).await?) {
            return Ok(true);
        }
        driver.sleep(100).await;
    }
    Ok(false)
}

pub async fn answer_every_group<D: Driver>(driver: &D, letter: char) -> PageResult<u64> {
    let selected = driver.execute_script(ANSWER_EVERY_GROUP, &[json!(letter.to_string())]).await?;
    Ok(js::to_number(Some(&selected)) as u64)
}

/// The checkpoint page's steps, so tests can stand in for any of them.
#[allow(async_fn_in_trait)]
pub trait CheckpointPage: FramedPage + Sized {
    async fn click_gate_button(&self) -> PageResult<bool> {
        click_gate_button(self, GATE_WAIT).await
    }
    async fn ensure_pills_expanded(&self) -> PageResult<bool> {
        ensure_pills_expanded(self, EXPAND_WAIT).await
    }
    async fn read_pills(&self) -> PageResult<Vec<u32>> {
        read_pills(self).await
    }
    async fn click_pill(&self, n: u32) -> PageResult<bool> {
        click_pill(self, n).await
    }
    async fn is_pill_locked(&self, n: u32) -> PageResult<bool> {
        is_pill_locked(self, n).await
    }
    async fn wait_for_pill_active(&self, n: u32) -> PageResult<bool> {
        wait_for_pill_active(self, n, PILL_ACTIVE_WAIT).await
    }
    async fn answer_every_group(&self, letter: char) -> PageResult<u64> {
        answer_every_group(self, letter).await
    }
    async fn click_save_button(&self) -> PageResult<bool> {
        click_save_button(self).await
    }
    async fn wait_for_pill_saved(&self, n: u32) -> PageResult<bool> {
        wait_for_pill_saved(self, n, PILL_SAVED_WAIT).await
    }
    async fn click_submit(&self) -> PageResult<Submit> {
        click_submit(self, SubmitOptions::default()).await
    }
    async fn wait_for_score(&self) -> PageResult<Option<u64>> {
        wait_for_score(self, SCORE_WAIT).await
    }
    /// The current question's handler, from the types a checkpoint registers.
    async fn question_kind(&self, wait: DomWait) -> PageResult<Option<QuestionKind>> {
        let mut registry = Registry::new();
        registry.register(QuestionKind::AudioMultipleChoice);
        registry.register(QuestionKind::ReadingComprehension);
        let question = get_current_question_dom(self, wait).await?;
        Ok(registry.find_kind(&question.dom))
    }
}

impl CheckpointPage for Lms {}

/// Why the completion passes ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Completion {
    AllAnswered,
    /// The leftover pills did not shrink on the last pass; they just don't fit this letter.
    NoProgress {
        unanswered: Vec<u64>,
    },
    PassesExhausted {
        unanswered: Vec<u64>,
    },
}

/// What `answerAllQuestions` returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnswerResult {
    /// No pills, or a pill that never turned active.
    Unhandled {
        question_num: u32,
    },
    Submitted {
        question_num: u32,
        completion: Completion,
    },
    SubmitFailed {
        question_num: u32,
        completion: Completion,
        submit: Submit,
    },
}

async fn find_pills<P: CheckpointPage>(page: &P) -> PageResult<Vec<u32>> {
    let mut pills = Vec::new();
    let mut reloaded = false;
    loop {
        let deadline = Instant::now() + PILLS_WAIT;
        while Instant::now() < deadline {
            page.ensure_pills_expanded().await?;
            pills = page.read_pills().await?;
            if !pills.is_empty() {
                break;
            }
            page.sleep(300).await;
        }
        if !pills.is_empty() || reloaded {
            return Ok(pills);
        }
        // "Failed to load Assessment type" is a transient glitch. One reload clears it.
        if !is_true(&page.execute_script(FAILED_TO_LOAD, &[]).await?) {
            return Ok(pills);
        }
        logger::warn("Checkpoint: page failed to load the assessment, reloading once", []);
        page.reload().await?;
        page.sleep(2000).await;
        reloaded = true;
    }
}

/// Settled once the pill turns active. A pill click sometimes doesn't register, so it is retried once.
async fn navigate_to<P: CheckpointPage>(page: &P, n: u32) -> PageResult<bool> {
    for _ in 0..2 {
        if page.click_pill(n).await? && page.wait_for_pill_active(n).await? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Options can still be mid-render (all disabled) after the pill turns active.
/// Polled so they aren't mistaken for a locked question.
async fn enabled_option_count<P: Driver>(page: &P) -> PageResult<f64> {
    let deadline = Instant::now() + ENABLED_OPTIONS_WAIT;
    let mut count = 0.0;
    while Instant::now() < deadline {
        count = js::to_number(Some(&page.execute_script(ENABLED_OPTIONS, &[]).await?));
        if count > 0.0 {
            break;
        }
        page.sleep(150).await;
    }
    Ok(count)
}

/// Selects, saves and waits. Any failure means this pill has no option for the letter.
async fn answer_pill<P: CheckpointPage>(page: &P, n: u32, letter: char) -> Result<u64, String> {
    let selected = page.answer_every_group(letter).await.map_err(|e| e.to_string())?;
    if selected == 0 {
        return Err(format!("no option button found for letter \"{letter}\""));
    }
    page.click_save_button().await.map_err(|e| e.to_string())?;
    page.wait_for_pill_saved(n).await.map_err(|e| e.to_string())?;
    Ok(selected)
}

/// One shared letter per attempt. Correct pills lock and are skipped on the next attempt.
pub async fn answer_all_questions<P: CheckpointPage>(
    page: &P,
    letter: char,
    max_completion_passes: u32,
) -> PageResult<AnswerResult> {
    page.question_kind(DomWait::default()).await?;
    // Pills can render well after the gate click, so poll.
    let pills = find_pills(page).await?;
    if pills.is_empty() {
        logger::warn("Checkpoint: no nav pills found, stopping", []);
        return Ok(AnswerResult::Unhandled { question_num: 0 });
    }
    logger::info("Checkpoint attempt: answering questions", [("pills", json!(pills.len()))]);

    let mut answered = 0;
    let mut previous_unanswered: Option<Vec<u64>> = None;
    let mut completion = Completion::AllAnswered;
    for completion_pass in 1..=max_completion_passes {
        for &n in &pills {
            page.ensure_pills_expanded().await?;
            if page.is_pill_locked(n).await? {
                continue;
            }
            if !navigate_to(page, n).await? {
                logger::warn("Checkpoint: pill navigation did not settle, stopping", [("pill", json!(n))]);
                return Ok(AnswerResult::Unhandled { question_num: answered });
            }

            // Questions render top-level, so use a short iframe wait.
            let Some(kind) = page.question_kind(DomWait::iframe_wait(600)).await? else {
                logger::warn("Checkpoint: unhandled question type, skipping pill", [("pill", json!(n))]);
                continue;
            };
            if enabled_option_count(page).await? <= 0.0 {
                continue; // locked, already correct
            }

            match answer_pill(page, n, letter).await {
                Ok(selected) => {
                    answered += 1;
                    logger::info(
                        "Checkpoint question answered",
                        [
                            ("pill", json!(n)),
                            ("type", json!(kind.name())),
                            ("letter", json!(letter.to_string())),
                            ("selected", json!(selected)),
                            ("completionPass", json!(completion_pass)),
                        ],
                    );
                }
                // Fewer options than this letter (e.g. a 3-option TFNG on an E round). A later letter may fit.
                Err(error) => logger::info(
                    "Checkpoint question has no option for this letter, will retry with the next letter next attempt",
                    [
                        ("pill", json!(n)),
                        ("letter", json!(letter.to_string())),
                        ("error", json!(error)),
                        ("completionPass", json!(completion_pass)),
                    ],
                ),
            }
        }

        page.ensure_pills_expanded().await?;
        let unanswered = page.execute_script(UNANSWERED_PILLS, &[]).await?;
        let unanswered: Vec<u64> =
            unanswered.as_array().map(|a| a.iter().filter_map(Value::as_u64).collect()).unwrap_or_default();
        if unanswered.is_empty() {
            completion = Completion::AllAnswered;
            break;
        }

        logger::warn(
            "Checkpoint: unanswered pills remain after completion pass",
            [
                ("unanswered", json!(unanswered)),
                ("completionPass", json!(completion_pass)),
                ("maxCompletionPasses", json!(max_completion_passes)),
            ],
        );
        // Leftover pills just don't fit this letter. Submit what is answered instead of failing.
        if let Some(previous) = &previous_unanswered {
            if unanswered.len() >= previous.len() {
                logger::info(
                    "Checkpoint: unanswered pills made no progress this letter, submitting what is answered",
                    [
                        ("unanswered", json!(unanswered)),
                        ("previousUnanswered", json!(previous)),
                        ("completionPass", json!(completion_pass)),
                    ],
                );
                completion = Completion::NoProgress { unanswered };
                break;
            }
        }
        previous_unanswered = Some(unanswered.clone());

        if completion_pass == max_completion_passes {
            logger::info(
                "Checkpoint: unanswered pills remain after max completion passes, submitting what is answered",
                [
                    ("unanswered", json!(unanswered)),
                    ("completionPass", json!(completion_pass)),
                    ("maxCompletionPasses", json!(max_completion_passes)),
                ],
            );
            completion = Completion::PassesExhausted { unanswered };
        }
    }

    // Submit exists only on the last pill, and Save & Next may have jumped elsewhere.
    if let Some(&last) = pills.last() {
        page.ensure_pills_expanded().await?;
        page.click_pill(last).await?;
        page.wait_for_pill_active(last).await?;
    }

    let submit = page.click_submit().await?;
    if submit != Submit::Confirmed {
        logger::warn(
            "Checkpoint: submit did not go through, stopping instead of pretending the attempt completed",
            [("questionNum", json!(answered))],
        );
        return Ok(AnswerResult::SubmitFailed { question_num: answered, completion, submit });
    }
    Ok(AnswerResult::Submitted { question_num: answered, completion })
}

/// What `runCheckpointAttempt` returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttemptResult {
    Complete { question_num: u32, score: Option<u64> },
    Unhandled { question_num: u32 },
    SubmitFailed { question_num: u32 },
}

impl AttemptResult {
    fn fields(self) -> Vec<(&'static str, Value)> {
        match self {
            AttemptResult::Complete { question_num, score } => {
                vec![("status", json!("complete")), ("questionNum", json!(question_num)), ("score", json!(score))]
            }
            AttemptResult::Unhandled { question_num } => {
                vec![("status", json!("unhandled")), ("questionNum", json!(question_num))]
            }
            AttemptResult::SubmitFailed { question_num } => {
                vec![("status", json!("submit-failed")), ("questionNum", json!(question_num))]
            }
        }
    }
}

pub async fn run_checkpoint_attempt<P: CheckpointPage>(page: &P, letter: char) -> PageResult<AttemptResult> {
    let question_num = match answer_all_questions(page, letter, MAX_COMPLETION_PASSES).await? {
        AnswerResult::Unhandled { question_num } => return Ok(AttemptResult::Unhandled { question_num }),
        AnswerResult::SubmitFailed { question_num, .. } => return Ok(AttemptResult::SubmitFailed { question_num }),
        AnswerResult::Submitted { question_num, .. } => question_num,
    };
    let score = page.wait_for_score().await?;
    logger::info(
        "Checkpoint attempt graded",
        [("questionNum", json!(question_num)), ("letter", json!(letter.to_string())), ("score", json!(score))],
    );
    Ok(AttemptResult::Complete { question_num, score })
}

/// Runs one attempt from the gate; tests swap in a scripted one.
#[allow(async_fn_in_trait)]
pub trait AttemptRunner<P> {
    async fn attempt(&mut self, page: &P, letter: char) -> PageResult<AttemptResult>;
}

pub struct LiveAttempt;

impl<P: CheckpointPage> AttemptRunner<P> for LiveAttempt {
    async fn attempt(&mut self, page: &P, letter: char) -> PageResult<AttemptResult> {
        run_checkpoint_attempt(page, letter).await
    }
}

async fn back_to_gate<P: Driver>(page: &P, gate_url: &str) -> PageResult<()> {
    page.get(gate_url).await?;
    page.sleep(1500).await;
    Ok(())
}

/// Retries from the gate without limit until the score is 100, on the current gate tab.
pub async fn run<P: CheckpointPage, R: AttemptRunner<P>>(page: &P, runner: &mut R) -> PageResult<CheckpointReport> {
    let gate_url = page.current_url().await?;
    let mut letter_index = 0;
    let mut submit_failures = 0;
    let mut attempt = 0;

    loop {
        attempt += 1;
        let letter = LETTERS[letter_index % LETTERS.len()];
        logger::info(
            "Starting checkpoint attempt",
            [("attempt", json!(attempt)), ("letter", json!(letter.to_string()))],
        );

        if !page.click_gate_button().await? {
            logger::warn(
                "Could not find a Start Attempt/Continue button on the gate page, stopping",
                [("attempt", json!(attempt))],
            );
            return Ok(CheckpointReport::new(CheckpointOutcome::NoGate, attempt));
        }
        page.sleep(1500).await;

        let result = runner.attempt(page, letter).await?;
        let mut fields = vec![("attempt", json!(attempt))];
        fields.extend(result.fields());
        logger::info("Checkpoint attempt finished", fields);

        match result {
            AttemptResult::Unhandled { question_num } => {
                // Structural, not a wrong guess. A retry would hit the same page.
                logger::warn(
                    "Unhandled question type inside checkpoint, stopping",
                    [("attempt", json!(attempt)), ("questionNum", json!(question_num))],
                );
                return Ok(CheckpointReport::new(CheckpointOutcome::Unhandled, attempt).question_num(question_num));
            }
            AttemptResult::SubmitFailed { .. } => {
                // Saved answers survive, so Continue resumes the attempt.
                submit_failures += 1;
                logger::warn(
                    "Checkpoint: submit did not go through, retrying from the gate",
                    [("attempt", json!(attempt)), ("submitFailures", json!(submit_failures))],
                );
                if submit_failures >= MAX_SUBMIT_FAILURES {
                    logger::warn(
                        "Checkpoint: submit kept failing, stopping",
                        [("attempt", json!(attempt)), ("submitFailures", json!(submit_failures))],
                    );
                    return Ok(CheckpointReport::new(CheckpointOutcome::SubmitFailed, attempt));
                }
                back_to_gate(page, &gate_url).await?;
            }
            AttemptResult::Complete { score, .. } => {
                submit_failures = 0;
                if score == Some(100) {
                    logger::info(
                        "Checkpoint passed",
                        [("attempt", json!(attempt)), ("letter", json!(letter.to_string())), ("score", json!(score))],
                    );
                    return Ok(CheckpointReport::new(CheckpointOutcome::Passed, attempt).score(score));
                }
                logger::warn(
                    "Checkpoint attempt did not reach a passing score, retrying",
                    [("attempt", json!(attempt)), ("letter", json!(letter.to_string())), ("score", json!(score))],
                );
                letter_index += 1;
                back_to_gate(page, &gate_url).await?;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    //! Ports of tests/checkpoint.test.js. Each JS `...Fn` injection is a hook on `Fake`;
    //! an absent hook falls through to the real step, which runs scripts against `script`.
    use super::*;
    use crate::browser::PageError;
    use std::cell::{Cell, RefCell};

    type Hook<'a, A, R> = Option<Box<dyn FnMut(A) -> R + 'a>>;

    #[derive(Default)]
    struct Hooks<'a> {
        read_pills: Hook<'a, (), Vec<u32>>,
        click_pill: Hook<'a, u32, bool>,
        is_locked: Hook<'a, u32, bool>,
        wait_active: Hook<'a, u32, bool>,
        answer: Hook<'a, char, u64>,
        save: Hook<'a, (), bool>,
        saved: Hook<'a, u32, bool>,
        submit: Hook<'a, (), Submit>,
    }

    type Script<'a> = Box<dyn FnMut(&str) -> Value + 'a>;

    struct Fake<'a> {
        script: RefCell<Script<'a>>,
        on_sleep: RefCell<Box<dyn FnMut(u64) + 'a>>,
        hooks: RefCell<Hooks<'a>>,
        urls: RefCell<Vec<String>>,
    }

    impl<'a> Fake<'a> {
        fn new(script: impl FnMut(&str) -> Value + 'a) -> Self {
            Fake {
                script: RefCell::new(Box::new(script)),
                on_sleep: RefCell::new(Box::new(|_| {})),
                hooks: RefCell::default(),
                urls: RefCell::new(vec!["https://lms.binus.ac.id/checkpoint-gate".to_string()]),
            }
        }

        fn on_sleep(self, hook: impl FnMut(u64) + 'a) -> Self {
            *self.on_sleep.borrow_mut() = Box::new(hook);
            self
        }

        fn hooks(self, edit: impl FnOnce(&mut Hooks<'a>)) -> Self {
            edit(&mut self.hooks.borrow_mut());
            self
        }
    }

    /// Calls the hook when the test set one, else the real step.
    macro_rules! hooked {
        ($self:ident, $hook:ident, $arg:expr, $fallback:expr) => {{
            let hooked = $self.hooks.borrow_mut().$hook.as_mut().map(|h| h($arg));
            match hooked {
                Some(value) => Ok(value),
                None => $fallback.await,
            }
        }};
    }

    impl Driver for Fake<'_> {
        async fn current_url(&self) -> PageResult<String> {
            Ok(self.urls.borrow().last().cloned().unwrap_or_default())
        }
        async fn execute_script(&self, body: &str, _args: &[Value]) -> PageResult<Value> {
            Ok((self.script.borrow_mut())(body))
        }
        async fn get(&self, url: &str) -> PageResult<()> {
            self.urls.borrow_mut().push(url.to_string());
            Ok(())
        }
        async fn sleep(&self, ms: u64) {
            (self.on_sleep.borrow_mut())(ms);
            // Deadlines are wall clock, so yield briefly instead of spinning.
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }

    impl FramedPage for Fake<'_> {
        async fn execute_in_frame_at(&self, _index: usize, _body: &str, _args: &[Value]) -> PageResult<Value> {
            Err(PageError("no iframe".to_string()))
        }
        async fn reload(&self) -> PageResult<()> {
            Ok(())
        }
    }

    impl CheckpointPage for Fake<'_> {
        async fn ensure_pills_expanded(&self) -> PageResult<bool> {
            Ok(true)
        }
        async fn read_pills(&self) -> PageResult<Vec<u32>> {
            hooked!(self, read_pills, (), read_pills(self))
        }
        async fn click_pill(&self, n: u32) -> PageResult<bool> {
            hooked!(self, click_pill, n, click_pill(self, n))
        }
        async fn is_pill_locked(&self, n: u32) -> PageResult<bool> {
            hooked!(self, is_locked, n, is_pill_locked(self, n))
        }
        async fn wait_for_pill_active(&self, n: u32) -> PageResult<bool> {
            hooked!(self, wait_active, n, wait_for_pill_active(self, n, PILL_ACTIVE_WAIT))
        }
        async fn answer_every_group(&self, letter: char) -> PageResult<u64> {
            hooked!(self, answer, letter, answer_every_group(self, letter))
        }
        async fn click_save_button(&self) -> PageResult<bool> {
            hooked!(self, save, (), click_save_button(self))
        }
        async fn wait_for_pill_saved(&self, n: u32) -> PageResult<bool> {
            hooked!(self, saved, n, wait_for_pill_saved(self, n, PILL_SAVED_WAIT))
        }
        async fn click_submit(&self) -> PageResult<Submit> {
            hooked!(self, submit, (), click_submit(self, SubmitOptions::default()))
        }
        async fn question_kind(&self, _wait: DomWait) -> PageResult<Option<QuestionKind>> {
            Ok(Some(QuestionKind::ReadingComprehension))
        }
    }

    fn gate_page(gate_clickable: bool) -> Fake<'static> {
        Fake::new(move |code| json!(code.contains("Start Attempt") && gate_clickable))
    }

    /// `runExerciseFn`: one scripted result per attempt, with the letter it was given.
    struct Scripted<F>(F);

    impl<P, F: FnMut(char) -> AttemptResult> AttemptRunner<P> for Scripted<F> {
        async fn attempt(&mut self, _page: &P, letter: char) -> PageResult<AttemptResult> {
            Ok((self.0)(letter))
        }
    }

    fn complete(score: u64) -> AttemptResult {
        AttemptResult::Complete { question_num: 30, score: Some(score) }
    }

    fn passed(attempt: u32) -> CheckpointReport {
        CheckpointReport::new(CheckpointOutcome::Passed, attempt).score(Some(100))
    }

    fn pills(list: &[u32]) -> Hook<'static, (), Vec<u32>> {
        let list = list.to_vec();
        Some(Box::new(move |()| list.clone()))
    }

    fn always<'a, A, R: Clone + 'a>(value: R) -> Hook<'a, A, R> {
        Some(Box::new(move |_| value.clone()))
    }

    fn submitted(question_num: u32) -> AnswerResult {
        AnswerResult::Submitted { question_num, completion: Completion::AllAnswered }
    }

    #[tokio::test]
    async fn passes_on_the_first_attempt_when_the_score_is_100() {
        let page = gate_page(true);
        let result = run(&page, &mut Scripted(|_| complete(100))).await.unwrap();
        assert_eq!(result, passed(1));
    }

    #[tokio::test]
    async fn retries_from_the_gate_when_the_score_is_below_100_then_passes() {
        let page = gate_page(true);
        let mut call = 0;
        let result = run(
            &page,
            &mut Scripted(|_| {
                call += 1;
                complete(if call == 1 { 83 } else { 100 })
            }),
        )
        .await
        .unwrap();
        assert_eq!(result, passed(2));
    }

    #[tokio::test]
    async fn stops_immediately_on_an_unhandled_question_type_without_retrying() {
        let page = gate_page(true);
        let mut calls = 0;
        let result = run(
            &page,
            &mut Scripted(|_| {
                calls += 1;
                AttemptResult::Unhandled { question_num: 7 }
            }),
        )
        .await
        .unwrap();
        assert_eq!(result, CheckpointReport::new(CheckpointOutcome::Unhandled, 1).question_num(7));
        assert_eq!(calls, 1);
    }

    #[tokio::test]
    async fn keeps_retrying_past_the_old_5_attempt_cap_until_it_passes() {
        let page = gate_page(true);
        let mut calls = 0;
        let result = run(
            &page,
            &mut Scripted(|_| {
                calls += 1;
                complete(if calls < 8 { 90 } else { 100 })
            }),
        )
        .await
        .unwrap();
        assert_eq!(result, passed(8));
        assert_eq!(calls, 8);
    }

    #[tokio::test]
    async fn stops_when_the_gate_button_is_never_found() {
        let page = gate_page(false);
        let result = run(&page, &mut Scripted(|_| panic!("should not be called"))).await.unwrap();
        assert_eq!(result, CheckpointReport::new(CheckpointOutcome::NoGate, 1));
    }

    #[tokio::test]
    async fn wait_for_score_parses_the_score_out_of_the_post_submit_result_page() {
        let page = Fake::new(|_| json!(77));
        assert_eq!(wait_for_score(&page, SCORE_WAIT).await, Ok(Some(77)));
    }

    #[tokio::test]
    async fn wait_for_score_resolves_undefined_if_the_score_never_appears_within_the_timeout() {
        let page = Fake::new(|_| Value::Null);
        assert_eq!(wait_for_score(&page, Duration::from_millis(50)).await, Ok(None));
    }

    #[tokio::test]
    async fn ensure_pills_expanded_clicks_the_checkpoint_chevron_and_waits_for_more_pills() {
        let pill_count = Cell::new(15);
        let clicks = Cell::new(0);
        let page = Fake::new(|code| {
            if code.contains("path.startsWith('M16.59 8.59L12 13.17')") {
                clicks.set(clicks.get() + 1);
                pill_count.set(30);
                return json!(true);
            }
            json!((1..=pill_count.get()).map(|n| json!({ "n": n })).collect::<Vec<_>>())
        });
        assert_eq!(ensure_pills_expanded(&page, EXPAND_WAIT).await, Ok(true));
        assert_eq!(clicks.get(), 1);
    }

    #[tokio::test]
    async fn wait_for_pill_saved_polls_until_the_pill_drops_its_unanswered_marker() {
        let calls = Cell::new(0);
        let page = Fake::new(|_| {
            calls.set(calls.get() + 1);
            json!(calls.get() >= 3)
        });
        assert_eq!(wait_for_pill_saved(&page, 5, PILL_SAVED_WAIT).await, Ok(true));
        assert_eq!(calls.get(), 3);
    }

    #[tokio::test]
    async fn wait_for_pill_saved_resolves_false_if_the_pill_never_clears_within_the_timeout() {
        let page = Fake::new(|_| json!(false));
        assert_eq!(wait_for_pill_saved(&page, 5, Duration::from_millis(250)).await, Ok(false));
    }

    #[tokio::test]
    async fn answer_all_questions_skips_a_locked_pill_without_clicking_it_or_waiting_for_it_to_go_active() {
        let visits = RefCell::new(vec![]);
        let waited_for = RefCell::new(vec![]);
        let answered_pills = RefCell::new(vec![]);
        let active = Cell::new(0);
        let page = Fake::new(|code| {
            if code.contains("filter((b) => !b.disabled).length") {
                return json!(6);
            }
            if code.contains("primary-light-shade-color") {
                return json!([]);
            }
            json!(false)
        })
        .hooks(|h| {
            h.read_pills = pills(&[1, 2, 3]);
            // Pill 2 is already correct from a prior attempt.
            h.is_locked = Some(Box::new(|n| n == 2));
            h.click_pill = Some(Box::new(|n| {
                visits.borrow_mut().push(n);
                active.set(n);
                true
            }));
            h.wait_active = Some(Box::new(|n| {
                waited_for.borrow_mut().push(n);
                active.get() == n
            }));
            h.answer = Some(Box::new(|letter| {
                answered_pills.borrow_mut().push((active.get(), letter));
                1
            }));
            h.save = always(true);
            h.saved = always(true);
            h.submit = always(Submit::Confirmed);
        });

        let result = answer_all_questions(&page, 'A', MAX_COMPLETION_PASSES).await.unwrap();
        drop(page);
        // The last pill is re-navigated before Submit.
        assert_eq!(visits.into_inner(), vec![1, 3, 3]);
        assert_eq!(waited_for.into_inner(), vec![1, 3, 3]);
        assert_eq!(answered_pills.into_inner(), vec![(1, 'A'), (3, 'A')]);
        assert_eq!(result, submitted(2));
    }

    #[tokio::test]
    async fn answer_all_questions_retries_a_pill_click_once_before_treating_it_as_a_stall() {
        let click_attempts = RefCell::new(vec![]);
        let active = Cell::new(0);
        let pill2_active_checks = Cell::new(0);
        // Everything else, including the enabled-options check, answers true.
        let page = Fake::new(|code| if code.contains("primary-light-shade-color") { json!([]) } else { json!(true) })
            .hooks(|h| {
                h.read_pills = pills(&[1, 2]);
                h.is_locked = always(false);
                h.click_pill = Some(Box::new(|n| {
                    click_attempts.borrow_mut().push(n);
                    active.set(n);
                    true
                }));
                h.wait_active = Some(Box::new(|n| {
                    if n == 2 {
                        pill2_active_checks.set(pill2_active_checks.get() + 1);
                        if pill2_active_checks.get() == 1 {
                            return false; // the first click's wait times out
                        }
                    }
                    active.get() == n
                }));
                h.answer = always(1);
                h.save = always(true);
                h.saved = always(true);
                h.submit = always(Submit::Confirmed);
            });

        let result = answer_all_questions(&page, 'A', MAX_COMPLETION_PASSES).await.unwrap();
        drop(page);
        // Pill 2 is clicked twice (miss, retry), then again for the last-pill nav before Submit.
        assert_eq!(click_attempts.into_inner(), vec![1, 2, 2, 2]);
        assert_eq!(result, submitted(2));
    }

    #[tokio::test]
    async fn answer_all_questions_navigates_by_pill_saves_each_answer_and_submits_without_leaving_the_final_pill() {
        let visits = RefCell::new(vec![]);
        let submissions = RefCell::new(vec![]);
        let answered_pills = RefCell::new(vec![]);
        let saved_pills = RefCell::new(vec![]);
        let active = Cell::new(0);
        let page = Fake::new(|code| {
            if code.contains("filter((b) => !b.disabled).length") {
                return json!(if active.get() != 2 { 6 } else { 0 });
            }
            if code.contains("primary-light-shade-color") {
                return json!([]);
            }
            if code.contains("t === 'Save'") {
                saved_pills.borrow_mut().push(active.get());
                return json!(true);
            }
            json!(false)
        })
        .hooks(|h| {
            h.read_pills = pills(&[1, 2, 3]);
            h.click_pill = Some(Box::new(|n| {
                visits.borrow_mut().push(n);
                active.set(n);
                true
            }));
            h.wait_active = Some(Box::new(|n| active.get() == n));
            h.answer = Some(Box::new(|letter| {
                answered_pills.borrow_mut().push((active.get(), letter));
                1
            }));
            h.saved = always(true);
            h.submit = Some(Box::new(|()| {
                submissions.borrow_mut().push(visits.borrow().clone());
                Submit::Confirmed
            }));
        });

        let result = answer_all_questions(&page, 'A', MAX_COMPLETION_PASSES).await.unwrap();
        drop(page);
        assert_eq!(answered_pills.into_inner(), vec![(1, 'A'), (3, 'A')]);
        assert_eq!(saved_pills.into_inner(), vec![1, 3]);
        assert_eq!(visits.into_inner(), vec![1, 2, 3, 3]);
        assert_eq!(submissions.into_inner(), vec![vec![1, 2, 3, 3]]);
        assert_eq!(result, submitted(2));
    }

    #[tokio::test]
    async fn answer_all_questions_uses_the_same_shared_letter_for_every_pill_in_the_attempt() {
        let letters_used = RefCell::new(vec![]);
        let active = Cell::new(0);
        let page = Fake::new(|code| {
            if code.contains("filter((b) => !b.disabled).length") {
                return json!(6);
            }
            if code.contains("primary-light-shade-color") {
                return json!([]);
            }
            json!(false)
        })
        .hooks(|h| {
            h.read_pills = pills(&[1, 2, 3, 4, 5, 6]);
            h.click_pill = Some(Box::new(|n| {
                active.set(n);
                true
            }));
            h.wait_active = Some(Box::new(|n| active.get() == n));
            h.answer = Some(Box::new(|letter| {
                letters_used.borrow_mut().push((active.get(), letter));
                1
            }));
            h.saved = always(true);
            h.submit = always(Submit::Confirmed);
        });

        answer_all_questions(&page, 'C', MAX_COMPLETION_PASSES).await.unwrap();
        drop(page);
        let letters_used = letters_used.into_inner();
        assert_eq!(letters_used.len(), 6);
        assert!(letters_used.iter().all(|&(_, letter)| letter == 'C'));
    }

    #[tokio::test]
    async fn answer_all_questions_applies_the_next_shared_letter_on_a_fresh_attempt_skipping_pills_already_locked() {
        let letters_used = RefCell::new(vec![]);
        let pill1_locked = Cell::new(false);
        let (letters, locked) = (&letters_used, &pill1_locked);
        let page = || {
            Fake::new(|code| {
                if code.contains("filter((b) => !b.disabled).length") {
                    return json!(6);
                }
                if code.contains("primary-light-shade-color") {
                    return json!([]);
                }
                json!(false)
            })
            .hooks(|h| {
                h.read_pills = pills(&[1, 3]);
                h.is_locked = Some(Box::new(move |n| n == 1 && locked.get()));
                h.click_pill = always(true);
                h.wait_active = always(true);
                h.answer = Some(Box::new(move |letter| {
                    letters.borrow_mut().push(letter);
                    1
                }));
                h.save = always(true);
                h.saved = always(true);
                h.submit = always(Submit::Confirmed);
            })
        };

        answer_all_questions(&page(), 'A', MAX_COMPLETION_PASSES).await.unwrap();
        assert_eq!(*letters_used.borrow(), vec!['A', 'A']);

        pill1_locked.set(true); // pill 1's A was graded correct, so it is frozen
        letters_used.borrow_mut().clear();
        answer_all_questions(&page(), 'B', MAX_COMPLETION_PASSES).await.unwrap();
        assert_eq!(*letters_used.borrow(), vec!['B']); // only pill 3 gets the new letter
    }

    #[tokio::test]
    async fn answer_all_questions_submits_whatever_is_answered_when_unanswered_pills_make_no_progress_across_passes() {
        let visits = RefCell::new(vec![]);
        let submissions = Cell::new(0);
        let active = Cell::new(0);
        let unanswered_checks = Cell::new(0);
        let page = Fake::new(|code| {
            if code.contains("filter((b) => !b.disabled).length") {
                return json!(6);
            }
            if code.contains("primary-light-shade-color") {
                unanswered_checks.set(unanswered_checks.get() + 1);
                return json!([2, 3]);
            }
            json!(false)
        })
        .hooks(|h| {
            h.read_pills = pills(&[1, 2, 3]);
            h.click_pill = Some(Box::new(|n| {
                visits.borrow_mut().push(n);
                active.set(n);
                true
            }));
            h.wait_active = Some(Box::new(|n| active.get() == n));
            h.answer = always(1);
            h.saved = always(true);
            h.submit = Some(Box::new(|()| {
                submissions.set(submissions.get() + 1);
                Submit::Confirmed
            }));
        });

        let result = answer_all_questions(&page, 'A', 3).await.unwrap();
        drop(page);
        // Not unhandled: a letter that stops resolving new pills is expected brute-force
        // behaviour, so the attempt still submits and the outer loop gets a real score.
        assert_eq!(
            result,
            AnswerResult::Submitted { question_num: 6, completion: Completion::NoProgress { unanswered: vec![2, 3] } }
        );
        assert_eq!(submissions.get(), 1);
        assert_eq!(unanswered_checks.get(), 2);
        assert_eq!(visits.into_inner(), vec![1, 2, 3, 1, 2, 3, 3]);
    }

    // Completion passes retry saves; they don't advance the letter.
    #[tokio::test]
    async fn answer_all_questions_reuses_the_same_letter_across_completion_passes_within_one_attempt() {
        let letters_used = RefCell::new(vec![]);
        let page = Fake::new(|code| {
            if code.contains("filter((b) => !b.disabled).length") {
                return json!(6);
            }
            if code.contains("primary-light-shade-color") {
                return json!([2]);
            }
            json!(false)
        })
        .hooks(|h| {
            h.read_pills = pills(&[1, 2]);
            h.is_locked = always(false);
            h.click_pill = always(true);
            h.wait_active = always(true);
            h.answer = Some(Box::new(|letter| {
                letters_used.borrow_mut().push(letter);
                1
            }));
            h.save = always(true);
            h.saved = always(true);
            h.submit = always(Submit::Confirmed);
        });

        answer_all_questions(&page, 'E', 2).await.unwrap();
        drop(page);
        let letters_used = letters_used.into_inner();
        assert!(!letters_used.is_empty());
        assert!(letters_used.iter().all(|&letter| letter == 'E'));
    }

    #[tokio::test]
    async fn main_advances_the_shared_letter_on_each_retry_until_the_checkpoint_passes() {
        let page = gate_page(true);
        let mut letters = vec![];
        let result = run(
            &page,
            &mut Scripted(|letter| {
                letters.push(letter);
                complete(if letters.len() == 1 { 80 } else { 100 })
            }),
        )
        .await
        .unwrap();
        assert_eq!(result, passed(2));
        assert_eq!(letters, vec!['A', 'B']); // attempt 1 all A, attempt 2 all B
    }

    #[tokio::test]
    async fn letters_wrap_around_after_f() {
        let page = gate_page(true);
        let mut letters = vec![];
        run(
            &page,
            &mut Scripted(|letter| {
                letters.push(letter);
                complete(if letters.len() < 8 { 50 } else { 100 })
            }),
        )
        .await
        .unwrap();
        assert_eq!(letters.iter().collect::<String>(), "ABCDEFAB");
    }

    fn submit_options(max_submit_clicks: u32, enabled_ms: u64, confirm_ms: u64) -> SubmitOptions {
        SubmitOptions {
            max_submit_clicks,
            enabled_wait: Duration::from_millis(enabled_ms),
            confirm_wait: Duration::from_millis(confirm_ms),
        }
    }

    #[tokio::test]
    async fn click_submit_retries_the_submit_click_when_the_first_one_never_opens_the_confirm() {
        let submit_clicks = Cell::new(0);
        let confirm_visible = Cell::new(false);
        let page = Fake::new(|code| {
            if code.contains("=== 'Submit'") {
                submit_clicks.set(submit_clicks.get() + 1);
                return json!(true);
            }
            if code.contains("=== 'Yes'") {
                return json!(confirm_visible.get());
            }
            json!(false)
        })
        .on_sleep(|ms| {
            // The backoff before a retry submit is where the page finally catches up.
            if ms == 300 {
                confirm_visible.set(true);
            }
        });
        assert_eq!(click_submit(&page, submit_options(3, 4000, 200)).await, Ok(Submit::Confirmed));
        assert_eq!(submit_clicks.get(), 2);
    }

    #[tokio::test]
    async fn click_submit_returns_false_when_the_submit_button_is_never_rendered() {
        let page = Fake::new(|_| Value::Null);
        assert_eq!(click_submit(&page, submit_options(3, 200, 200)).await, Ok(Submit::NeverRendered));
    }

    #[tokio::test]
    async fn click_submit_waits_for_a_transiently_disabled_submit_button_to_enable_then_submits() {
        let disabled = Cell::new(true);
        let submit_clicks = Cell::new(0);
        let confirm_visible = Cell::new(false);
        let page = Fake::new(|code| {
            if code.contains("=== 'Submit'") {
                if disabled.get() {
                    return json!(false); // found but disabled
                }
                submit_clicks.set(submit_clicks.get() + 1);
                confirm_visible.set(true);
                return json!(true);
            }
            if code.contains("=== 'Yes'") {
                return json!(confirm_visible.get());
            }
            json!(false)
        })
        .on_sleep(|ms| {
            if ms == 200 && disabled.get() {
                disabled.set(false); // the save settles and the button enables
            }
        });
        assert_eq!(click_submit(&page, submit_options(3, 1000, 200)).await, Ok(Submit::Confirmed));
        assert_eq!(submit_clicks.get(), 1);
    }

    #[tokio::test]
    async fn click_submit_returns_false_when_the_submit_button_stays_disabled_the_whole_window() {
        let page = Fake::new(|_| json!(false));
        assert_eq!(click_submit(&page, submit_options(3, 300, 100)).await, Ok(Submit::StayedDisabled));
    }

    #[tokio::test]
    async fn click_submit_returns_false_when_the_confirm_never_appears_across_all_submit_retries() {
        let submit_clicks = Cell::new(0);
        let page = Fake::new(|code| {
            if code.contains("=== 'Submit'") {
                submit_clicks.set(submit_clicks.get() + 1);
                return json!(true);
            }
            json!(false)
        });
        assert_eq!(click_submit(&page, submit_options(2, 4000, 100)).await, Ok(Submit::NoConfirm));
        assert_eq!(submit_clicks.get(), 2);
    }

    #[tokio::test]
    async fn answer_all_questions_reports_submit_failed_when_the_submit_click_never_lands() {
        let page = Fake::new(|code| {
            if code.contains("primary-light-shade-color") {
                return json!([]); // nothing left unanswered, so a single pass
            }
            if code.contains("filter((b) => !b.disabled).length") {
                return json!(1);
            }
            json!(false)
        })
        .hooks(|h| {
            h.read_pills = pills(&[1]);
            h.is_locked = always(false);
            h.click_pill = always(true);
            h.wait_active = always(true);
            h.answer = always(1);
            h.save = always(true);
            h.saved = always(true);
            h.submit = always(Submit::NoConfirm);
        });

        let result = answer_all_questions(&page, 'A', MAX_COMPLETION_PASSES).await.unwrap();
        assert_eq!(
            result,
            AnswerResult::SubmitFailed {
                question_num: 1,
                completion: Completion::AllAnswered,
                submit: Submit::NoConfirm
            }
        );
    }

    #[tokio::test]
    async fn run_checkpoint_retries_a_submit_failed_attempt_then_passes_once_submit_lands() {
        let page = gate_page(true);
        let mut call = 0;
        let result = run(
            &page,
            &mut Scripted(|_| {
                call += 1;
                if call == 1 {
                    AttemptResult::SubmitFailed { question_num: 30 }
                } else {
                    complete(100)
                }
            }),
        )
        .await
        .unwrap();
        assert_eq!(result, passed(2));
        assert_eq!(page.urls.borrow().len(), 2); // one trip back to the gate
    }

    #[tokio::test]
    async fn run_checkpoint_stops_after_3_consecutive_submit_failures() {
        let page = gate_page(true);
        let result = run(&page, &mut Scripted(|_| AttemptResult::SubmitFailed { question_num: 30 })).await.unwrap();
        assert_eq!(result, CheckpointReport::new(CheckpointOutcome::SubmitFailed, 3));
    }

    #[tokio::test]
    async fn read_pills_and_click_pill_are_exported_for_reuse_by_other_scripts() {
        let page = Fake::new(|code| {
            if code.contains("out.push({ n })") {
                return json!([{ "n": 1 }, { "n": 2 }]);
            }
            json!(code.contains("closest('button')"))
        });
        assert_eq!(read_pills(&page).await, Ok(vec![1, 2]));
        assert_eq!(click_pill(&page, 2).await, Ok(true));
    }
}
