//! Port of the browser half of `src/runners/final-test.js`, on top of `crate::final_test`.
//! Site behavior this relies on: docs/platform-notes.md#final-test
//!
//! Read-only by default: only pill navigation touches the page. A Final Test has one attempt,
//! so Submit is never clicked, and an option or Save only under an explicit env opt-in.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};

use super::checkpoint::{self, PILL_SAVED_WAIT};
use super::Report;
use crate::answer_loop::Answerer;
use crate::browser::{
    get_current_question_dom, jittered, DomWait, FramedPage, Lms, PageError, PageResult, PageSleeper,
};
use crate::config::Config;
use crate::final_test::{
    answer_with_retry, build_markdown, combine_review, format_question_record, load_previous_answers, test_id_from_url,
    FinalAnswer, QuestionRecord, RecordSource, Review, DEFAULT_ANSWER_ATTEMPTS,
};
use crate::js;
use crate::llm::{CodexClient, LlmAnswerer};
use crate::logger;
use crate::question_types::{
    AudioMultipleChoiceData, Dom, OptionButton, QuestionData, QuestionKind, ReadingComprehensionData, Registry,
};

pub const OUTPUT_PATH: &str = "./final-test-answers.md";
pub const ANSWERS_PATH: &str = "./final-test-answers.json";

const CLICK_OPTION: &str = r#"
    const letter = arguments[0];
    const buttons = [...document.querySelectorAll('button.bl-w-full.justify-content-start')]
      .filter((b) => b.offsetParent !== null);
    const target = buttons.find((b) => b.textContent.trim().startsWith(letter));
    if (target && !target.disabled) { target.click(); return true; }
    return false;
  "#;

/// A question as the final test's registry (audioMultipleChoice, readingComprehension) parses it.
#[derive(Debug, Clone, PartialEq)]
pub enum FinalQuestion {
    Listening(AudioMultipleChoiceData),
    Mcq(ReadingComprehensionData),
}

/// The final test page's steps, so tests can stand in for any of them.
#[allow(async_fn_in_trait)]
pub trait FinalTestPage: FramedPage + Sized {
    async fn ensure_pills_expanded(&self) -> PageResult<bool> {
        checkpoint::ensure_pills_expanded(self, checkpoint::EXPAND_WAIT).await
    }
    async fn read_pills(&self) -> PageResult<Vec<u32>> {
        checkpoint::read_pills(self).await
    }
    async fn click_pill(&self, n: u32) -> PageResult<bool> {
        checkpoint::click_pill(self, n).await
    }
    /// Questions render top-level, so the iframe wait is short.
    async fn question_dom(&self) -> PageResult<Dom> {
        Ok(get_current_question_dom(self, DomWait::iframe_wait(600)).await?.dom)
    }
    fn classify(&self, dom: &Dom) -> Option<FinalQuestion> {
        let mut registry = Registry::new();
        registry.register(QuestionKind::AudioMultipleChoice);
        registry.register(QuestionKind::ReadingComprehension);
        match registry.find_kind(dom)?.parse(dom) {
            QuestionData::AudioMultipleChoice(data) => Some(FinalQuestion::Listening(data)),
            QuestionData::ReadingComprehension(data) => Some(FinalQuestion::Mcq(data)),
            _ => None,
        }
    }
    /// The visible option whose text starts with `letter`. No Check.
    async fn click_option_by_letter(&self, letter: &str) -> PageResult<bool> {
        Ok(self.execute_script(CLICK_OPTION, &[json!(letter)]).await? == json!(true))
    }
    async fn save_and_wait(&self, n: u32) -> PageResult<bool> {
        if !checkpoint::click_save_button(self).await? {
            return Ok(false);
        }
        checkpoint::wait_for_pill_saved(self, n, PILL_SAVED_WAIT).await
    }
}

impl FinalTestPage for Lms {}

fn random_letter(letters: &[Option<char>]) -> Option<char> {
    if letters.is_empty() {
        return None;
    }
    letters[jittered(0, letters.len() as u64) as usize]
}

#[derive(Debug, Clone)]
pub struct ScrapeOptions {
    /// `FINAL_TEST_AUTO_ANSWER=1`: click and save answers that clear both thresholds.
    pub auto_answer: bool,
    pub confidence_threshold: f64,
    pub reviews_required: f64,
    /// `FINAL_TEST_GUESS_LISTENING=1`: click and save a random option on listening questions.
    pub guess_listening: bool,
    pub change_wait: Duration,
    pub change_poll_ms: u64,
    /// Records from an earlier run over the same test, as stored.
    pub previous_answers: Vec<Value>,
    pub random_letter: fn(&[Option<char>]) -> Option<char>,
}

impl Default for ScrapeOptions {
    fn default() -> Self {
        ScrapeOptions {
            auto_answer: false,
            confidence_threshold: 0.9,
            reviews_required: 1.0,
            guess_listening: false,
            change_wait: Duration::from_millis(5000),
            change_poll_ms: 300,
            previous_answers: Vec::new(),
            random_letter,
        }
    }
}

/// `Number(process.env[key] || fallback)`, so a non-numeric value is NaN and never passes a threshold.
fn env_number(env: &HashMap<String, String>, key: &str, fallback: f64) -> f64 {
    match env.get(key).filter(|v| !v.is_empty()) {
        Some(value) => js::to_number(Some(&json!(value))),
        None => fallback,
    }
}

impl ScrapeOptions {
    pub fn from_env(env: &HashMap<String, String>) -> Self {
        let flag = |key: &str| env.get(key).is_some_and(|v| v == "1");
        ScrapeOptions {
            auto_answer: flag("FINAL_TEST_AUTO_ANSWER"),
            confidence_threshold: env_number(env, "FINAL_TEST_CONFIDENCE_THRESHOLD", 0.9),
            reviews_required: env_number(env, "FINAL_TEST_REVIEWS_REQUIRED", 1.0),
            guess_listening: flag("FINAL_TEST_GUESS_LISTENING"),
            ..ScrapeOptions::default()
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ScrapeOutcome {
    NoPills,
    /// A pill that would not take a click, which would otherwise re-read the same question.
    Stuck {
        pill: u32,
        records: Vec<QuestionRecord>,
    },
    Complete {
        records: Vec<QuestionRecord>,
    },
}

impl ScrapeOutcome {
    pub fn records(&self) -> &[QuestionRecord] {
        match self {
            ScrapeOutcome::NoPills => &[],
            ScrapeOutcome::Stuck { records, .. } | ScrapeOutcome::Complete { records } => records,
        }
    }
}

impl Report for ScrapeOutcome {
    fn status(&self) -> &'static str {
        match self {
            ScrapeOutcome::NoPills => "no-pills",
            ScrapeOutcome::Stuck { .. } => "stuck",
            ScrapeOutcome::Complete { .. } => "complete",
        }
    }

    fn fields(&self) -> Vec<(&'static str, Value)> {
        let mut fields = vec![("status", json!(self.status()))];
        if let ScrapeOutcome::Stuck { pill, .. } = self {
            fields.push(("pill", json!(pill)));
        }
        if !matches!(self, ScrapeOutcome::NoPills) {
            fields.push(("questions", json!(self.records().len())));
        }
        fields
    }
}

/// A stored record's review fields. A missing answer reads as JS `undefined`.
fn review_from_record(record: &Value) -> Review {
    Review {
        answer: match &record["answer"] {
            Value::String(text) => FinalAnswer::from_text(text),
            Value::Null => FinalAnswer::Given("undefined".to_string()),
            other => FinalAnswer::Given(js::to_string(other)),
        },
        confidence: record["confidence"].as_f64(),
        agreements: record["agreements"].as_u64().map(|n| n as u32),
    }
}

fn previous_by_number(previous: &[Value]) -> HashMap<u32, Review> {
    previous
        .iter()
        .filter(|r| r["type"] == json!("mcq"))
        .filter_map(|r| Some((r["number"].as_u64()? as u32, review_from_record(r))))
        .collect()
}

/// A dropped pill click would re-read the same question, and the pill nav re-collapses,
/// so re-expand and retry.
async fn open_pill<P: FinalTestPage>(page: &P, n: u32, poll_ms: u64) -> PageResult<bool> {
    for _ in 0..3 {
        page.ensure_pills_expanded().await?;
        if page.click_pill(n).await? {
            return Ok(true);
        }
        page.sleep(poll_ms).await;
    }
    Ok(false)
}

/// Trust the DOM once two reads `poll_ms` apart match. outerHTML noise makes
/// comparing to the previous question useless.
async fn settled_dom<P: FinalTestPage>(page: &P, wait: Duration, poll_ms: u64) -> PageResult<Dom> {
    let deadline = Instant::now() + wait;
    let mut previous = page.question_dom().await?.outer_html;
    page.sleep(poll_ms).await;
    let mut dom = page.question_dom().await?;
    while dom.outer_html != previous && Instant::now() < deadline {
        previous = dom.outer_html;
        page.sleep(poll_ms).await;
        dom = page.question_dom().await?;
    }
    Ok(dom)
}

/// Read-only unless `auto_answer` or `guess_listening` is on.
pub async fn scrape_final_test<P: FinalTestPage, A: Answerer<ReadingComprehensionData>>(
    page: &P,
    answerer: &mut A,
    options: &ScrapeOptions,
) -> PageResult<ScrapeOutcome> {
    let previous_by_number = previous_by_number(&options.previous_answers);

    page.ensure_pills_expanded().await?;
    let pills = page.read_pills().await?;
    if pills.is_empty() {
        logger::warn("Final test: no nav pills found, stopping", []);
        return Ok(ScrapeOutcome::NoPills);
    }

    let mut records = Vec::new();
    for n in pills {
        if !open_pill(page, n, options.change_poll_ms).await? {
            logger::warn("Final test: could not click pill, stopping", [("pill", json!(n))]);
            return Ok(ScrapeOutcome::Stuck { pill: n, records });
        }
        let dom = settled_dom(page, options.change_wait, options.change_poll_ms).await?;

        let question = match page.classify(&dom) {
            None => {
                logger::warn("Final test: unhandled question type", [("pill", json!(n))]);
                records.push(format_question_record(n, RecordSource::Unhandled, false));
                continue;
            }
            Some(FinalQuestion::Listening(data)) => {
                if !options.guess_listening {
                    logger::info("Final test: skipping listening question", [("pill", json!(n))]);
                    records.push(format_question_record(n, RecordSource::Listening { answer: None }, false));
                    continue;
                }
                let letters: Vec<Option<char>> = data.options.iter().map(|o| o.letter).collect();
                let guess = (options.random_letter)(&letters).map(String::from);
                // A letterless guess is JS `startsWith(undefined)`, which matches no option.
                let clicked = match &guess {
                    Some(letter) => page.click_option_by_letter(letter).await?,
                    None => false,
                };
                if clicked {
                    page.save_and_wait(n).await?;
                }
                logger::info(
                    "Final test: listening guessed",
                    [("pill", json!(n)), ("guess", json!(guess)), ("clicked", json!(clicked))],
                );
                let answer = guess.as_deref().filter(|_| clicked);
                records.push(format_question_record(n, RecordSource::Listening { answer }, clicked));
                continue;
            }
            Some(FinalQuestion::Mcq(data)) => data,
        };

        let previous = previous_by_number.get(&n);
        let fresh =
            answer_with_retry(answerer, &question, DEFAULT_ANSWER_ATTEMPTS, &mut PageSleeper(page), previous).await;
        if fresh.answer == FinalAnswer::NeedsReview {
            logger::warn("Final test: LLM failed for a question after retries, marking for manual review", []);
        }
        let review = combine_review(previous, &fresh);
        let reviewed = previous.is_some();

        // A missing confidence or agreement count is JS undefined, which fails every comparison.
        let confident = review.confidence.is_some_and(|c| c >= options.confidence_threshold);
        let agreed = review.agreements.is_some_and(|a| f64::from(a) >= options.reviews_required);
        let mut auto_answered = false;
        if options.auto_answer
            && review.answer != FinalAnswer::NeedsReview
            && confident
            && agreed
            && page.click_option_by_letter(&review.answer.to_string()).await?
        {
            page.save_and_wait(n).await?;
            auto_answered = true;
            logger::info("Final test: auto-answered", review_fields(n, &review, reviewed));
        }
        let mut fields = review_fields(n, &review, reviewed);
        fields.push(("autoAnswered", json!(auto_answered)));
        logger::info("Final test: question answered", fields);
        records.push(format_question_record(n, RecordSource::Mcq { data: &question, review }, auto_answered));
    }

    Ok(ScrapeOutcome::Complete { records })
}

fn review_fields(n: u32, review: &Review, reviewed: bool) -> Vec<(&'static str, Value)> {
    vec![
        ("pill", json!(n)),
        ("answer", json!(review.answer.to_string())),
        ("confidence", json!(review.confidence)),
        ("agreements", json!(review.agreements)),
        ("reviewed", json!(reviewed)),
    ]
}

/// One record in `final-test-answers.json`, with the JS key order and its dropped `undefined`s.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RecordJson<'a> {
    number: u32,
    #[serde(rename = "type")]
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    options: Option<&'a [OptionButton]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    answer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    confidence: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    agreements: Option<u32>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    auto_answered: bool,
}

impl<'a> From<&'a QuestionRecord> for RecordJson<'a> {
    fn from(record: &'a QuestionRecord) -> Self {
        let base = |number, kind| RecordJson {
            number,
            kind,
            text: None,
            options: None,
            answer: None,
            confidence: None,
            agreements: None,
            auto_answered: false,
        };
        match record {
            QuestionRecord::Listening { number, answer, auto_answered } => {
                RecordJson { answer: answer.clone(), auto_answered: *auto_answered, ..base(*number, "listening") }
            }
            QuestionRecord::Unhandled { number } => base(*number, "unhandled"),
            QuestionRecord::Mcq { number, text, options, answer, confidence, agreements, auto_answered } => {
                RecordJson {
                    text: Some(text),
                    options: Some(options),
                    answer: Some(answer.to_string()),
                    confidence: *confidence,
                    agreements: *agreements,
                    auto_answered: *auto_answered,
                    ..base(*number, "mcq")
                }
            }
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AnswersFile<'a> {
    test_id: &'a str,
    answers: Vec<RecordJson<'a>>,
}

/// `JSON.stringify({ testId, answers }, null, 2)`.
pub fn answers_json(test_id: &str, records: &[QuestionRecord]) -> String {
    let file = AnswersFile { test_id, answers: records.iter().map(RecordJson::from).collect() };
    serde_json::to_string_pretty(&file).expect("records serialize")
}

/// Scrapes the open Final Test attempt into the answer key files in the working directory.
pub async fn run<P: FinalTestPage>(
    page: &P,
    config: &Config,
    env: &HashMap<String, String>,
) -> PageResult<ScrapeOutcome> {
    let client = CodexClient::from_config(config).map_err(|e| PageError(e.to_string()))?;
    let mut options = ScrapeOptions::from_env(env);

    let test_id = test_id_from_url(&page.current_url().await?);
    let contents = std::fs::read_to_string(ANSWERS_PATH).ok();
    let previous = load_previous_answers(&test_id, contents.as_deref());
    if previous.stale {
        logger::warn(
            "Final test: previous answer key is for a DIFFERENT test, ignoring it and starting fresh",
            [("storedTestId", json!(previous.stored_test_id)), ("currentTestId", json!(test_id))],
        );
    } else if !previous.answers.is_empty() {
        logger::info(
            "Final test: review pass, re-deriving previous answers",
            [("previous", json!(previous.answers.len())), ("testId", json!(test_id))],
        );
    }
    if !previous.stale {
        options.previous_answers = previous.answers;
    }

    let result = scrape_final_test(page, &mut LlmAnswerer(&client), &options).await?;
    if result == ScrapeOutcome::NoPills {
        logger::warn("Final test: nothing to scrape. Is the browser on an open Final Test attempt?", []);
        return Ok(result);
    }

    let write = |path: &str, text: String| std::fs::write(path, text).map_err(|e| PageError(format!("{path}: {e}")));
    write(OUTPUT_PATH, build_markdown(result.records()))?;
    write(ANSWERS_PATH, answers_json(&test_id, result.records()))?;
    logger::info(
        "Final test: answer key written",
        [("path", json!(OUTPUT_PATH)), ("questions", json!(result.records().len()))],
    );
    Ok(result)
}

#[cfg(test)]
mod tests {
    //! Ports of the scrapeFinalTest tests in tests/final-test.test.js. The fake reports
    //! each pill's fixture as the question once that pill is clicked.
    use super::*;
    use crate::browser::Driver;
    use crate::llm::LlmError;
    use std::cell::{Cell, RefCell};

    struct FakeFinal {
        pills: Vec<u32>,
        doms: HashMap<u32, Option<FinalQuestion>>,
        active: Cell<u32>,
        option_clicks_land: bool,
        clicked: RefCell<Vec<String>>,
        saved: RefCell<Vec<u32>>,
    }

    impl FakeFinal {
        fn new(pills: &[u32], doms: Vec<(u32, Option<FinalQuestion>)>) -> Self {
            FakeFinal {
                pills: pills.to_vec(),
                doms: doms.into_iter().collect(),
                active: Cell::new(0),
                option_clicks_land: true,
                clicked: RefCell::default(),
                saved: RefCell::default(),
            }
        }
    }

    impl Driver for FakeFinal {
        async fn current_url(&self) -> PageResult<String> {
            Ok("https://lms.binus.ac.id/final#/question-1".to_string())
        }
        async fn execute_script(&self, body: &str, _args: &[Value]) -> PageResult<Value> {
            panic!("unexpected script {body}")
        }
        async fn get(&self, url: &str) -> PageResult<()> {
            panic!("unexpected navigation to {url}")
        }
        async fn sleep(&self, _ms: u64) {}
    }

    impl FramedPage for FakeFinal {
        async fn execute_in_frame_at(&self, _index: usize, body: &str, _args: &[Value]) -> PageResult<Value> {
            panic!("unexpected frame script {body}")
        }
        async fn reload(&self) -> PageResult<()> {
            panic!("unexpected reload")
        }
    }

    impl FinalTestPage for FakeFinal {
        async fn ensure_pills_expanded(&self) -> PageResult<bool> {
            Ok(true)
        }
        async fn read_pills(&self) -> PageResult<Vec<u32>> {
            Ok(self.pills.clone())
        }
        async fn click_pill(&self, n: u32) -> PageResult<bool> {
            self.active.set(n);
            Ok(true)
        }
        async fn question_dom(&self) -> PageResult<Dom> {
            Ok(Dom { outer_html: format!("pill-{}", self.active.get()), ..Dom::default() })
        }
        fn classify(&self, dom: &Dom) -> Option<FinalQuestion> {
            let n: u32 = dom.outer_html.strip_prefix("pill-")?.parse().ok()?;
            self.doms.get(&n).cloned().flatten()
        }
        async fn click_option_by_letter(&self, letter: &str) -> PageResult<bool> {
            self.clicked.borrow_mut().push(letter.to_string());
            Ok(self.option_clicks_land)
        }
        async fn save_and_wait(&self, n: u32) -> PageResult<bool> {
            self.saved.borrow_mut().push(n);
            Ok(true)
        }
    }

    fn opts(options: &[(char, &str)]) -> Vec<OptionButton> {
        options.iter().map(|&(letter, text)| OptionButton::new(letter, text)).collect()
    }

    fn mcq(text: &str, options: &[(char, &str)]) -> Option<FinalQuestion> {
        Some(FinalQuestion::Mcq(ReadingComprehensionData { text: text.to_string(), options: opts(options) }))
    }

    fn listening(question_text: &str, options: &[(char, &str)]) -> Option<FinalQuestion> {
        Some(FinalQuestion::Listening(AudioMultipleChoiceData {
            instruction: "listen".to_string(),
            question_text: question_text.to_string(),
            options: opts(options),
        }))
    }

    /// `answerQuestionFn` returning a fixed `{ answer, confidence }`.
    fn answers(answer: &'static str, confidence: f64) -> impl Answerer<ReadingComprehensionData> {
        move |_: &str, _: &ReadingComprehensionData, _: Option<&str>| -> Result<Value, LlmError> {
            Ok(json!({ "answer": answer, "confidence": confidence }))
        }
    }

    fn no_llm(_: &str, _: &ReadingComprehensionData, _: Option<&str>) -> Result<Value, LlmError> {
        panic!("LLM should not be called")
    }

    fn records(outcome: ScrapeOutcome) -> Vec<QuestionRecord> {
        match outcome {
            ScrapeOutcome::Complete { records } => records,
            other => panic!("expected complete, got {other:?}"),
        }
    }

    fn mcq_record(number: u32, text: &str, options: &[(char, &str)], answer: &str, confidence: f64) -> QuestionRecord {
        QuestionRecord::Mcq {
            number,
            text: text.to_string(),
            options: opts(options),
            answer: FinalAnswer::Given(answer.to_string()),
            confidence: Some(confidence),
            agreements: Some(1),
            auto_answered: false,
        }
    }

    fn agreements(record: &QuestionRecord) -> Option<u32> {
        match record {
            QuestionRecord::Mcq { agreements, .. } => *agreements,
            _ => None,
        }
    }

    #[tokio::test]
    async fn scrape_final_test_walks_every_pill_answers_mcq_questions_skips_listening_records_unhandled() {
        let page =
            FakeFinal::new(&[1, 2, 3], vec![(1, mcq("Q1?", &[('A', "x")])), (2, listening("Q2?", &[])), (3, None)]);
        let result = scrape_final_test(&page, &mut answers("A", 0.95), &ScrapeOptions::default()).await.unwrap();
        assert_eq!(
            records(result),
            vec![
                mcq_record(1, "Q1?", &[('A', "x")], "A", 0.95),
                QuestionRecord::Listening { number: 2, answer: None, auto_answered: false },
                QuestionRecord::Unhandled { number: 3 },
            ]
        );
        assert!(page.clicked.borrow().is_empty());
        assert!(page.saved.borrow().is_empty());
    }

    #[tokio::test]
    async fn scrape_final_test_review_pass_re_derives_and_boosts_confidence_on_agreement() {
        let page = FakeFinal::new(&[1], vec![(1, mcq("Q1?", &[('A', "x"), ('B', "y")]))]);
        let mut instructions = vec![];
        let mut answerer = |instruction: &str, _: &ReadingComprehensionData, _: Option<&str>| {
            instructions.push(instruction.to_string());
            Ok(json!({ "answer": "A", "confidence": 0.7 }))
        };
        let options = ScrapeOptions {
            previous_answers: vec![json!({ "number": 1, "type": "mcq", "answer": "A", "confidence": 0.7 })],
            ..ScrapeOptions::default()
        };
        let result = records(scrape_final_test(&page, &mut answerer, &options).await.unwrap());
        // The review instruction embeds the previous answer; agreement lifts confidence to min(0.99, 0.7 + 0.2).
        assert!(instructions[0].contains("\"A\""));
        let QuestionRecord::Mcq { answer, confidence, agreements, .. } = &result[0] else { panic!() };
        assert_eq!(*answer, FinalAnswer::Given("A".to_string()));
        assert!((confidence.unwrap() - 0.9).abs() < 1e-9);
        assert_eq!(*agreements, Some(2));
    }

    #[tokio::test]
    async fn scrape_final_test_review_pass_caps_confidence_on_disagreement() {
        let page = FakeFinal::new(&[1], vec![(1, mcq("Q1?", &[('A', "x"), ('B', "y")]))]);
        let options = ScrapeOptions {
            previous_answers: vec![json!({ "number": 1, "type": "mcq", "answer": "A", "confidence": 0.8 })],
            ..ScrapeOptions::default()
        };
        let result = records(scrape_final_test(&page, &mut answers("B", 0.9), &options).await.unwrap());
        let QuestionRecord::Mcq { answer, confidence, .. } = &result[0] else { panic!() };
        assert_eq!(*answer, FinalAnswer::Given("B".to_string()));
        // min(0.9, 0.8) * 0.5 = 0.4, which never auto-clicks.
        assert_eq!(*confidence, Some(0.4));
    }

    #[tokio::test]
    async fn scrape_final_test_auto_clicks_and_saves_a_confident_answer_when_auto_answer_enabled() {
        let page = FakeFinal::new(&[1, 2], vec![(1, mcq("Q1?", &[('A', "x")])), (2, mcq("Q2?", &[('B', "y")]))]);
        let mut answerer = |_: &str, qd: &ReadingComprehensionData, _: Option<&str>| {
            Ok(json!({ "answer": qd.options[0].letter.unwrap().to_string(), "confidence": 0.95 }))
        };
        let options = ScrapeOptions { auto_answer: true, confidence_threshold: 0.9, ..ScrapeOptions::default() };
        let result = records(scrape_final_test(&page, &mut answerer, &options).await.unwrap());
        assert_eq!(*page.clicked.borrow(), vec!["A", "B"]);
        assert_eq!(*page.saved.borrow(), vec![1, 2]);
        assert!(matches!(result[0], QuestionRecord::Mcq { auto_answered: true, .. }));
    }

    #[tokio::test]
    async fn scrape_final_test_leaves_answers_below_confidence_threshold_unclicked() {
        let page = FakeFinal::new(&[1], vec![(1, mcq("Q1?", &[('A', "x")]))]);
        let options = ScrapeOptions { auto_answer: true, confidence_threshold: 0.9, ..ScrapeOptions::default() };
        let result = records(scrape_final_test(&page, &mut answers("A", 0.5), &options).await.unwrap());
        assert!(page.clicked.borrow().is_empty());
        assert!(matches!(result[0], QuestionRecord::Mcq { auto_answered: false, .. }));
    }

    #[tokio::test]
    async fn scrape_final_test_never_auto_clicks_listening_questions() {
        let page = FakeFinal::new(&[1], vec![(1, listening("Q1?", &[]))]);
        let options = ScrapeOptions { auto_answer: true, ..ScrapeOptions::default() };
        let result = records(scrape_final_test(&page, &mut answers("A", 1.0), &options).await.unwrap());
        assert!(page.clicked.borrow().is_empty());
        assert_eq!(result, vec![QuestionRecord::Listening { number: 1, answer: None, auto_answered: false }]);
    }

    #[tokio::test]
    async fn scrape_final_test_guesses_and_saves_a_random_listening_answer_when_guess_listening_enabled() {
        let page = FakeFinal::new(&[1], vec![(1, listening("Q1?", &[('A', "x"), ('B', "y"), ('C', "z")]))]);
        let options =
            ScrapeOptions { guess_listening: true, random_letter: |letters| letters[1], ..ScrapeOptions::default() };
        let result = records(scrape_final_test(&page, &mut no_llm, &options).await.unwrap());
        assert_eq!(*page.clicked.borrow(), vec!["B"]);
        assert_eq!(*page.saved.borrow(), vec![1]);
        assert_eq!(
            result,
            vec![QuestionRecord::Listening { number: 1, answer: Some("B".to_string()), auto_answered: true }]
        );
    }

    #[tokio::test]
    async fn scrape_final_test_listening_guess_skipped_when_click_fails() {
        let mut page = FakeFinal::new(&[1], vec![(1, listening("Q1?", &[('A', "x")]))]);
        page.option_clicks_land = false;
        let options =
            ScrapeOptions { guess_listening: true, random_letter: |letters| letters[0], ..ScrapeOptions::default() };
        let result = records(scrape_final_test(&page, &mut no_llm, &options).await.unwrap());
        assert!(page.saved.borrow().is_empty());
        assert_eq!(result, vec![QuestionRecord::Listening { number: 1, answer: None, auto_answered: false }]);
    }

    #[tokio::test]
    async fn scrape_final_test_auto_answer_honors_reviews_required_no_click_below_the_pass_threshold() {
        let page = FakeFinal::new(&[1], vec![(1, mcq("Q1?", &[('A', "x"), ('B', "y")]))]);
        // High confidence and one agreeing pass (agreements would be 2), but three are required.
        let options = ScrapeOptions {
            previous_answers: vec![
                json!({ "number": 1, "type": "mcq", "answer": "A", "confidence": 0.7, "agreements": 1 }),
            ],
            auto_answer: true,
            confidence_threshold: 0.9,
            reviews_required: 3.0,
            ..ScrapeOptions::default()
        };
        let result = records(scrape_final_test(&page, &mut answers("A", 0.95), &options).await.unwrap());
        assert!(page.clicked.borrow().is_empty());
        assert_eq!(agreements(&result[0]), Some(2));
    }

    #[tokio::test]
    async fn scrape_final_test_auto_answer_clicks_when_agreements_clear_reviews_required() {
        let page = FakeFinal::new(&[1], vec![(1, mcq("Q1?", &[('A', "x"), ('B', "y")]))]);
        let options = ScrapeOptions {
            previous_answers: vec![
                json!({ "number": 1, "type": "mcq", "answer": "A", "confidence": 0.7, "agreements": 2 }),
            ],
            auto_answer: true,
            confidence_threshold: 0.9,
            reviews_required: 3.0,
            ..ScrapeOptions::default()
        };
        let result = records(scrape_final_test(&page, &mut answers("A", 0.95), &options).await.unwrap());
        assert_eq!(page.clicked.borrow().len(), 1);
        assert_eq!(agreements(&result[0]), Some(3));
    }

    #[tokio::test]
    async fn scrape_final_test_returns_no_pills_status_without_calling_the_llm_when_read_pills_is_empty() {
        let page = FakeFinal::new(&[], vec![]);
        let result = scrape_final_test(&page, &mut no_llm, &ScrapeOptions::default()).await.unwrap();
        assert_eq!(result, ScrapeOutcome::NoPills);
    }

    #[test]
    fn scrape_options_read_the_env_opt_ins_and_default_to_read_only() {
        let env = |pairs: &[(&str, &str)]| -> HashMap<String, String> {
            pairs.iter().map(|&(k, v)| (k.to_string(), v.to_string())).collect()
        };
        let default = ScrapeOptions::from_env(&env(&[]));
        assert!(!default.auto_answer && !default.guess_listening);
        assert_eq!((default.confidence_threshold, default.reviews_required), (0.9, 1.0));

        let on = ScrapeOptions::from_env(&env(&[
            ("FINAL_TEST_AUTO_ANSWER", "1"),
            ("FINAL_TEST_GUESS_LISTENING", "1"),
            ("FINAL_TEST_CONFIDENCE_THRESHOLD", "0.75"),
            ("FINAL_TEST_REVIEWS_REQUIRED", "2"),
        ]));
        assert!(on.auto_answer && on.guess_listening);
        assert_eq!((on.confidence_threshold, on.reviews_required), (0.75, 2.0));

        let junk =
            ScrapeOptions::from_env(&env(&[("FINAL_TEST_AUTO_ANSWER", "true"), ("FINAL_TEST_REVIEWS_REQUIRED", "x")]));
        assert!(!junk.auto_answer);
        assert!(junk.reviews_required.is_nan());
    }

    #[test]
    fn answers_json_keeps_the_js_shape_and_round_trips_through_load_previous_answers() {
        let records = vec![
            QuestionRecord::Mcq {
                number: 1,
                text: "Q1?".to_string(),
                options: opts(&[('A', "x")]),
                answer: FinalAnswer::NeedsReview,
                confidence: Some(0.0),
                agreements: Some(1),
                auto_answered: false,
            },
            QuestionRecord::Listening { number: 2, answer: Some("B".to_string()), auto_answered: true },
            QuestionRecord::Unhandled { number: 3 },
        ];
        let text = answers_json("https://lms.binus.ac.id/final", &records);
        assert!(text.starts_with("{\n  \"testId\": \"https://lms.binus.ac.id/final\",\n  \"answers\": ["));
        let parsed: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            parsed["answers"],
            json!([
                { "number": 1, "type": "mcq", "text": "Q1?", "options": [{ "letter": "A", "text": "x" }],
                  "answer": "NEEDS REVIEW", "confidence": 0.0, "agreements": 1 },
                { "number": 2, "type": "listening", "answer": "B", "autoAnswered": true },
                { "number": 3, "type": "unhandled" },
            ])
        );
        let loaded = load_previous_answers("https://lms.binus.ac.id/final", Some(&text));
        assert!(!loaded.stale);
        assert_eq!(previous_by_number(&loaded.answers)[&1].answer, FinalAnswer::NeedsReview);
        assert!(load_previous_answers("https://lms.binus.ac.id/other", Some(&text)).stale);
    }
}
