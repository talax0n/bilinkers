//! Port of `src/runners/iframe.js`.
//! Site behavior this relies on: docs/platform-notes.md#lti-iframe-player

use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{json, Value};

use super::exercise::{question_fields, save_unhandled};
use super::{ActivityOutcome, ActivityReport};
use crate::answer_loop::{process_question, Answerer, Outcome, QuestionResult};
use crate::browser::{
    get_current_question_dom, jittered, DomWait, Driver, FramedPage, PageError, PageResult, PageSleeper, Scoped,
};
use crate::config::Config;
use crate::llm::{CodexClient, LlmAnswerer, LlmError};
use crate::logger;
use crate::question_types::{LiveHandler, QuestionData, Registry};
use crate::strategy::{
    build_instruction, iframe_retry_limit, solve_for, BruteForceAnswerer, EmptyAnswer, Solve, IFRAME_QUESTION_TYPES,
};

const CLOSING_TEXT: &str = "You can now close this activity";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ActivityState {
    pub url: String,
    /// The "Question N of M" counter, kept for logging only: it is sometimes stale.
    pub question: Option<String>,
    /// All of `#content`'s text, which changes whenever the counter or the question does.
    pub content: String,
    pub closing: bool,
}

const READ_ACTIVITY_STATE: &str = r#"
    const text = document.querySelector('#content')?.innerText || '';
    const question = text.match(/Question\s+\d+\s+of\s+\d+/i)?.[0] || null;
    return {
      url: window.location.href,
      question,
      content: text,
      closing: text.includes('You can now close this activity'),
    };
  "#;

const CLICK_CONTINUE: &str = r#"
    const link = document.querySelector('.quiz-next-btn');
    if (link && getComputedStyle(link).display !== 'none') { link.click(); return true; }
    return false;
  "#;

pub async fn read_activity_state<D: Driver>(driver: &D) -> PageResult<ActivityState> {
    let state = driver.execute_script(READ_ACTIVITY_STATE, &[]).await?;
    serde_json::from_value(state).map_err(|e| PageError(format!("activity state: {e}")))
}

pub async fn wait_for_activity_change<D: Driver>(
    driver: &D,
    previous: &ActivityState,
    timeout: Duration,
) -> PageResult<Option<ActivityState>> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let current = read_activity_state(driver).await?;
        if current.closing || current.content != previous.content {
            return Ok(Some(current));
        }
        driver.sleep(200).await;
    }
    Ok(None)
}

/// Clicks a visible Continue and waits for the slide to change. False means the activity is over.
pub async fn advance<D: Driver>(driver: &D, previous: &ActivityState, timeout: Duration) -> PageResult<bool> {
    if driver.execute_script(CLICK_CONTINUE, &[]).await? != json!(true) {
        return Ok(false);
    }
    Ok(wait_for_activity_change(driver, previous, timeout).await?.is_some())
}

/// The answerer each strategy uses. A fresh brute forcer per question keeps its counter from leaking.
enum IframeAnswerer<'a> {
    Llm(LlmAnswerer<'a>),
    LetterCombinations(BruteForceAnswerer),
    Empty(EmptyAnswer),
}

impl Answerer<QuestionData> for IframeAnswerer<'_> {
    async fn answer(
        &mut self,
        instruction: &str,
        data: &QuestionData,
        feedback: Option<&str>,
    ) -> Result<Value, LlmError> {
        match self {
            IframeAnswerer::Llm(llm) => llm.answer(instruction, data, feedback).await,
            IframeAnswerer::LetterCombinations(brute) => brute.answer(instruction, data, feedback).await,
            IframeAnswerer::Empty(empty) => Answerer::<QuestionData>::answer(empty, instruction, data, feedback).await,
        }
    }
}

fn instruction_text(data: &QuestionData) -> Option<&str> {
    match data {
        QuestionData::FillInBlank(d) => Some(&d.instruction_text),
        QuestionData::ErrorAnalysis(d) => Some(&d.instruction_text),
        QuestionData::QuizMatching(d) => Some(&d.instruction_text),
        _ => None,
    }
}

pub async fn run<P: FramedPage>(page: &P, config: &Config) -> PageResult<ActivityReport> {
    let mut registry = Registry::new();
    for (kind, _) in IFRAME_QUESTION_TYPES {
        registry.register(kind);
    }
    // Only fillInBlank needs the model, so an unsupported provider fails only when one shows up.
    let llm = CodexClient::from_config(config);

    let mut question_num = 0;
    let mut slide_num = 0;
    loop {
        slide_num += 1;
        let question = get_current_question_dom(page, DomWait::default()).await?;
        let driver = Scoped::new(page, question.scope);
        let activity_state = read_activity_state(&driver).await?;

        // The closing slide matches no handler. That is the normal end.
        if question.dom.outer_html.contains(CLOSING_TEXT) {
            logger::info("Reached activity closing screen — done", [("questions", json!(question_num))]);
            return Ok(ActivityReport::new(ActivityOutcome::Complete).question_num(question_num));
        }

        let Some(kind) = registry.find_kind(&question.dom) else {
            save_unhandled(config, &format!("run-iframe-exercise-slide{slide_num}"), &question.dom.outer_html);
            logger::warn("Unhandled question type — stopping", [("slideNum", json!(slide_num))]);
            return Ok(ActivityReport::new(ActivityOutcome::Unhandled).slide_num(slide_num));
        };
        let solve = solve_for(&IFRAME_QUESTION_TYPES, kind).expect("every registered type has a strategy");

        // Vocabulary slides are walked without counting them as questions.
        if solve == Solve::ClickThrough {
            crate::question_types::vocabulary_intro::answer(&driver).await.map_err(PageError)?;
            driver.sleep(jittered(100, 100)).await;
            continue;
        }

        question_num += 1;
        // Short reading delay so submissions don't land instantly.
        driver.sleep(jittered(100, 100)).await;

        let data = kind.parse(&question.dom);
        let instruction = build_instruction(kind, instruction_text(&data));
        let mut answerer = match solve {
            Solve::Llm => IframeAnswerer::Llm(LlmAnswerer(llm.as_ref().map_err(|e| PageError(e.to_string()))?)),
            Solve::LetterCombinations => IframeAnswerer::LetterCombinations(BruteForceAnswerer::new()),
            _ => IframeAnswerer::Empty(EmptyAnswer),
        };
        let retry_limit = iframe_retry_limit(solve, config.retry.max_answer_retries) as i64;
        let mut handler = LiveHandler { kind, driver: Scoped::new(page, question.scope) };
        let result = process_question(
            Some(&mut handler),
            &question.dom,
            &instruction,
            retry_limit,
            &mut answerer,
            &mut PageSleeper(page),
        )
        .await
        .map_err(|e| PageError(e.to_string()))?;
        logger::info("Question processed", question_fields(question_num, kind, result));

        if matches!(result, QuestionResult::Finished { outcome: Outcome::Incorrect, .. }) {
            logger::warn(
                "Still incorrect after all retries — stopping instead of advancing",
                [("questionNum", json!(question_num))],
            );
            return Ok(ActivityReport::new(ActivityOutcome::Incorrect).question_num(question_num));
        }

        if !advance(&driver, &activity_state, Duration::from_secs(60)).await? {
            logger::info(
                "Could not advance (no Continue link, no next question hash) — activity complete",
                [("questionNum", json!(question_num))],
            );
            return Ok(ActivityReport::new(ActivityOutcome::Complete).question_num(question_num));
        }
        driver.sleep(jittered(1500, 1500)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::fake::FakePage;
    use crate::browser::Scope;

    fn state(url: &str, question: &str, content: &str) -> ActivityState {
        ActivityState {
            url: url.to_string(),
            question: Some(question.to_string()),
            content: content.to_string(),
            closing: false,
        }
    }

    fn as_json(s: &ActivityState) -> Value {
        json!({ "url": s.url, "question": s.question, "content": s.content, "closing": s.closing })
    }

    #[tokio::test]
    async fn wait_for_activity_change_ignores_url_only_changes_until_rendered_content_changes() {
        let mut states = vec![
            state("#/question-15", "Question 14 of 15", "Question 14 of 15\nold sentence"),
            state("#/question-15", "Question 15 of 15", "Question 15 of 15\nnew sentence"),
        ];
        let page = FakePage::new(|_: Scope, _: &str, _: &[Value]| -> PageResult<Value> {
            let next = if states.len() > 1 { states.remove(0) } else { states[0].clone() };
            Ok(as_json(&next))
        });
        let previous = state("#/question-14", "Question 14 of 15", "Question 14 of 15\nold sentence");
        let result = wait_for_activity_change(&page, &previous, Duration::from_secs(1)).await.unwrap().unwrap();
        assert_eq!(result.question.as_deref(), Some("Question 15 of 15"));
    }

    // The counter label can stay frozen while the sentence underneath moves on.
    #[tokio::test]
    async fn wait_for_activity_change_treats_it_as_advanced_when_content_changes_even_if_the_counter_label_is_stale() {
        let page = FakePage::new(|_: Scope, _: &str, _: &[Value]| -> PageResult<Value> {
            Ok(as_json(&state(
                "#/question-4",
                "Question 3 of 10",
                "Question 3 of 10\nWalk straight, then ________ at the drug store.",
            )))
        });
        let previous = state("#/question-3", "Question 3 of 10", "Question 3 of 10\nSome earlier sentence.");
        let result = wait_for_activity_change(&page, &previous, Duration::from_secs(1)).await.unwrap();
        assert_eq!(result.unwrap().question.as_deref(), Some("Question 3 of 10"));
    }

    #[tokio::test]
    async fn advance_returns_false_when_a_stale_visible_continue_never_changes_rendered_content() {
        let page = FakePage::new(|_: Scope, code: &str, _: &[Value]| -> PageResult<Value> {
            if code.contains("document.querySelector('.quiz-next-btn')") {
                return Ok(json!(true));
            }
            Ok(as_json(&state("#/question-15", "Question 14 of 15", "Question 14 of 15\nold sentence")))
        });
        let previous = state("#/question-14", "Question 14 of 15", "Question 14 of 15\nold sentence");
        assert_eq!(advance(&page, &previous, Duration::from_millis(10)).await, Ok(false));
    }

    #[test]
    fn iframe_runner_uses_shared_max_answer_retry_config() {
        let source = include_str!("iframe.rs");
        assert!(source.contains("iframe_retry_limit(solve, config.retry.max_answer_retries)"));
        assert!(!source.contains(concat!("MAX_ANSWER_RETRIES", " = 10")));
    }
}
