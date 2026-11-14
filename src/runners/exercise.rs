//! Port of `src/runners/exercise.js`.
//! Site behavior this relies on: docs/platform-notes.md#native-exercise

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::{native, ActivityOutcome, ActivityReport};
use crate::answer_loop::{process_question, Outcome, QuestionResult};
use crate::browser::{
    get_current_question_dom, go_to_next_question, jittered, DomWait, Driver, FramedPage, PageError, PageResult,
    PageSleeper, Scoped,
};
use crate::config::Config;
use crate::llm::CodexClient;
use crate::logger;
use crate::question_api::{find_on_page, ApiQuestion};
use crate::question_types::{LiveHandler, QuestionData, QuestionKind, Registry};
use crate::strategy::{TryEachOption, EXERCISE_QUESTION_TYPES};

/// What `submitIfPresent` found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Submit {
    NotPresent,
    Submitted { score: Option<u64> },
}

/// The result screen animates in, so the score is read and Next clicked in the same script.
const RESULT_SCORE_AND_NEXT: &str = r#"
      const scoreMatch = document.body.textContent.match(/Your Score:\s*(\d+)/);
      const buttons = Array.from(document.querySelectorAll('button'));
      const btn = buttons.find((b) => b.textContent.trim() === 'Next');
      const clicked = Boolean(btn && !btn.disabled);
      if (clicked) btn.click();
      return { score: scoreMatch ? Number(scoreMatch[1]) : null, clicked };
    "#;

/// Polls the result screen for 8 s every 300 ms; the first score seen wins.
pub async fn read_result_and_click_next<D: Driver>(driver: &D) -> PageResult<Option<u64>> {
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut score = None;
    while Instant::now() < deadline {
        let result = driver.execute_script(RESULT_SCORE_AND_NEXT, &[]).await?;
        if score.is_none() {
            score = result["score"].as_u64();
        }
        if result["clicked"] == json!(true) {
            break;
        }
        driver.sleep(300).await;
    }
    Ok(score)
}

/// Clicks the enabled button whose trimmed text is exactly `label`.
pub async fn click_button<D: Driver>(driver: &D, label: &str) -> PageResult<bool> {
    let clicked = driver
        .execute_script(
            r#"
    const buttons = Array.from(document.querySelectorAll('button'));
    const btn = buttons.find((b) => b.textContent.trim() === arguments[0]);
    if (btn && !btn.disabled) { btn.click(); return true; }
    return false;
  "#,
            &[json!(label)],
        )
        .await?;
    Ok(clicked == json!(true))
}

/// Numbered-pill exercises record the attempt only after Submit, Yes, then Next on the result screen.
pub async fn submit_if_present<D: Driver>(driver: &D) -> PageResult<Submit> {
    if !click_button(driver, "Submit").await? {
        return Ok(Submit::NotPresent);
    }
    driver.sleep(jittered(1000, 500)).await;
    click_button(driver, "Yes").await?;
    driver.sleep(1500).await;
    let score = read_result_and_click_next(driver).await?;
    driver.sleep(1500).await;
    Ok(Submit::Submitted { score })
}

/// Numbered-pill layouts have no Next button, so the next pill is clicked instead.
pub async fn click_pill_number<D: Driver>(driver: &D, number: u32) -> PageResult<bool> {
    click_button(driver, &number.to_string()).await
}

/// Saves the page of a question no handler recognised, for later inspection.
pub fn save_unhandled(config: &Config, name: &str, html: &str) {
    if let Err(e) = logger::save_unhandled(&config.paths.unhandled_log_dir, name, html) {
        logger::warn("Could not save the unhandled page", [("error", json!(e.to_string()))]);
    }
}

pub fn question_fields(question_num: u32, kind: QuestionKind, result: QuestionResult) -> Vec<(&'static str, Value)> {
    let mut fields = vec![("questionNum", json!(question_num)), ("type", json!(kind.name()))];
    match result {
        QuestionResult::Unhandled => fields.extend([("status", json!("unhandled")), ("attempts", json!(0))]),
        QuestionResult::Finished { outcome, attempts } => {
            fields.extend([("status", json!(outcome.name())), ("attempts", json!(attempts))]);
        }
    }
    fields
}

/// Lettered option questions need no LLM: listening can't be heard, and in-place Check makes
/// trying each option cheap. Other native questions are read off the question API (src/runners/native.rs),
/// and only their short answers may ask the LLM.
pub async fn run<P: FramedPage>(page: &P, config: &Config) -> PageResult<ActivityReport> {
    let mut registry = Registry::new();
    for (kind, _) in EXERCISE_QUESTION_TYPES {
        registry.register(kind);
    }
    let llm = CodexClient::from_config(config).ok();
    let mut api_questions: Option<Vec<ApiQuestion>> = None;

    let mut question_num = 0;
    loop {
        question_num += 1;
        // Native exercises have no iframe, so skip the long LTI iframe wait.
        let question = get_current_question_dom(page, DomWait::iframe_wait(300)).await?;
        let Some(kind) = registry.find_kind(&question.dom) else {
            if api_questions.is_none() {
                api_questions = Some(native::load_questions(page).await?);
            }
            let page_text = page.execute_script(native::PAGE_TEXT, &[]).await?;
            let questions = api_questions.as_deref().unwrap_or_default();
            let found = find_on_page(questions, page_text.as_str().unwrap_or_default());
            let result = match found {
                Some(q) => native::solve(page, q, llm.as_ref(), config.retry.max_answer_retries).await?,
                None => QuestionResult::Unhandled,
            };
            if let Some(q) = found {
                question_num = q.no;
                let mut fields = vec![("questionNum", json!(q.no)), ("type", json!(q.kind.name()))];
                if let QuestionResult::Finished { outcome, attempts } = result {
                    fields.extend([("status", json!(outcome.name())), ("attempts", json!(attempts))]);
                }
                logger::info("Question processed", fields);
            }
            match result {
                QuestionResult::Finished { outcome: Outcome::Correct, .. } => {}
                QuestionResult::Finished { outcome: Outcome::Incorrect, .. } => {
                    logger::warn(
                        "Still incorrect after every answer — stopping",
                        [("questionNum", json!(question_num))],
                    );
                    return Ok(ActivityReport::new(ActivityOutcome::Incorrect).question_num(question_num));
                }
                QuestionResult::Unhandled => {
                    save_unhandled(config, &format!("run-exercise-q{question_num}"), &question.dom.outer_html);
                    logger::warn("Unhandled question type — stopping", [("questionNum", json!(question_num))]);
                    return Ok(ActivityReport::new(ActivityOutcome::Unhandled).question_num(question_num));
                }
            }
            if let Some(report) = advance(page, question_num).await? {
                return Ok(report);
            }
            continue;
        };

        let options = match kind.parse(&question.dom) {
            QuestionData::AudioMultipleChoice(data) => data.options,
            QuestionData::ReadingComprehension(data) => data.options,
            _ => Vec::new(),
        };
        let retry_limit = options.len() as i64 - 1;
        let mut handler = LiveHandler { kind, driver: Scoped::new(page, question.scope) };
        let result = process_question(
            Some(&mut handler),
            &question.dom,
            "",
            retry_limit,
            &mut TryEachOption::new(options),
            &mut PageSleeper(page),
        )
        .await
        .map_err(|e| PageError(e.to_string()))?;
        logger::info("Question processed", question_fields(question_num, kind, result));

        if let Some(report) = advance(page, question_num).await? {
            return Ok(report);
        }
    }
}

/// Moves past `question_num` by Next, else by its pill. With neither, submits and reports the end.
async fn advance<D: Driver>(page: &D, question_num: u32) -> PageResult<Option<ActivityReport>> {
    if go_to_next_question(page).await? || click_pill_number(page, question_num + 1).await? {
        return Ok(None);
    }
    let submit = submit_if_present(page).await?;
    let message = match submit {
        Submit::Submitted { .. } => "Submitted the exercise",
        Submit::NotPresent => "No further \"Next\" button or pill — exercise complete",
    };
    logger::info(message, [("questionNum", json!(question_num))]);
    let score = match submit {
        Submit::Submitted { score } => score,
        Submit::NotPresent => None,
    };
    Ok(Some(ActivityReport::new(ActivityOutcome::Complete).question_num(question_num).score(score)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::fake::FakePage;
    use crate::browser::Scope;

    fn page_with(
        mut script: impl FnMut(&str, &[Value]) -> Value,
    ) -> FakePage<impl FnMut(Scope, &str, &[Value]) -> PageResult<Value>> {
        FakePage::new(move |_: Scope, code: &str, args: &[Value]| Ok(script(code, args)))
    }

    fn labelled(args: &[Value], label: &str) -> bool {
        args.first() == Some(&json!(label))
    }

    #[tokio::test]
    async fn submit_if_present_resolves_submitted_false_when_there_is_nothing_to_submit() {
        let page = page_with(|_, _| json!(false));
        assert_eq!(submit_if_present(&page).await, Ok(Submit::NotPresent));
    }

    #[tokio::test]
    async fn submit_if_present_parses_the_score_off_the_result_screen_before_while_clicking_next() {
        let page = page_with(|code, args| {
            if labelled(args, "Submit") || labelled(args, "Yes") {
                return json!(true);
            }
            if code.contains("Your Score") {
                return json!({ "score": 100, "clicked": true });
            }
            json!(false)
        });
        assert_eq!(submit_if_present(&page).await, Ok(Submit::Submitted { score: Some(100) }));
    }

    #[tokio::test]
    async fn submit_if_present_keeps_polling_until_the_result_screen_and_its_next_button_appear() {
        let mut poll_count = 0;
        let page = page_with(|code, args| {
            if labelled(args, "Submit") || labelled(args, "Yes") {
                return json!(true);
            }
            if code.contains("Your Score") {
                poll_count += 1;
                if poll_count < 3 {
                    return json!({ "score": null, "clicked": false });
                }
                return json!({ "score": 87, "clicked": true });
            }
            json!(false)
        });
        assert_eq!(submit_if_present(&page).await, Ok(Submit::Submitted { score: Some(87) }));
        drop(page);
        assert_eq!(poll_count, 3);
    }
}
