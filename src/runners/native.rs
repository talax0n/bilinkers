//! Native questions that render without the lettered option buttons (the BiTS vocabulary, error
//! recognition and short answer items), answered from the question API instead of the markup.
//! The page is only used to click a word, type a blank, and press Check, all found by text.
//! Site behavior this relies on: docs/platform-notes.md#question-api

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::exercise::click_button;
use crate::answer_loop::{Outcome, QuestionResult};
use crate::browser::{Driver, FramedPage, PageError, PageResult};
use crate::llm::CodexClient;
use crate::logger;
use crate::question_api::{candidates, parse_questions, ApiKind, ApiQuestion, QUESTION_API};
use crate::question_types::option_buttons::wait_for_check_feedback;
use crate::runners::unit::CLICK_START_GATE;

/// A native question page: a top-level Check button. Lettered option pages have one too, but
/// they are matched earlier by their option buttons.
pub const CHECK_BUTTON: &str =
    "return [...document.querySelectorAll('button')].some((b) => b.textContent.trim() === 'Check');";

pub const PAGE_TEXT: &str = "return document.body ? document.body.innerText : '';";

/// Clicks the deepest visible element whose whole text is the word, preferring one inside a
/// button (a word-bank chip) over the same word elsewhere. Navigation pills are skipped.
const CLICK_TEXT: &str = r#"
  const want = arguments[0].replace(/\s+/g, ' ').trim().toLowerCase();
  const text = (el) => el.textContent.replace(/\s+/g, ' ').trim().toLowerCase();
  const hits = [...document.querySelectorAll('body *')].filter((el) =>
    el.offsetParent !== null &&
    !el.closest('header, nav, .bl-button__container') &&
    text(el) === want &&
    ![...el.children].some((child) => text(child) === want));
  const el = hits.find((e) => e.closest('button, [role=button]')) || hits[hits.length - 1];
  if (!el) return false;
  (el.closest('button, [role=button]') || el).click();
  return true;
"#;

/// Types into the first visible text field, through the native setter so React sees the change.
const TYPE_BLANK: &str = r#"
  const fields = [...document.querySelectorAll('input[type=text], input:not([type]), textarea')]
    .filter((f) => f.offsetParent !== null && !f.disabled && !f.closest('header, nav'));
  const field = fields[0];
  if (!field) return false;
  const proto = field instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
  Object.getOwnPropertyDescriptor(proto, 'value').set.call(field, arguments[0]);
  field.dispatchEvent(new Event('input', { bubbles: true }));
  field.dispatchEvent(new Event('change', { bubbles: true }));
  return true;
"#;

/// Reloads once to read the attempt's questions off the API, then waits for the question to come
/// back, passing the Start or Continue gate if the reload landed on it.
pub async fn load_questions<P: FramedPage>(page: &P) -> PageResult<Vec<ApiQuestion>> {
    let Some(body) = page.reload_capturing(QUESTION_API).await? else { return Ok(Vec::new()) };
    let questions = serde_json::from_str::<Value>(&body).map(|v| parse_questions(&v)).unwrap_or_default();
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if page.execute_script(CHECK_BUTTON, &[]).await? == json!(true) {
            break;
        }
        page.execute_script(CLICK_START_GATE, &[]).await?;
        page.sleep(300).await;
    }
    logger::info("Read the question API", [("questions", json!(questions.len()))]);
    Ok(questions)
}

const SHORT_ANSWER_INSTRUCTION: &str = "Fill the blank (____) in the sentence. Reply with JSON {\"answer\": \"<only the words that go in the blank>\"}. Never repeat an answer listed in alreadyTried.";

async fn ask_llm(
    llm: &CodexClient,
    q: &ApiQuestion,
    tried: &[String],
    hint: Option<&str>,
) -> PageResult<Option<String>> {
    let data = json!({ "sentence": q.text, "alreadyTried": tried });
    let reply =
        llm.answer_question(SHORT_ANSWER_INSTRUCTION, &data, hint).await.map_err(|e| PageError(e.to_string()))?;
    let answer = reply["answer"].as_str().or(reply["answers"][0].as_str()).map(str::trim);
    Ok(answer.filter(|a| !a.is_empty()).map(String::from))
}

/// Tries answers best first until Check says correct. Unhandled when the page has nothing to click
/// or type for an answer, which means markup this code has not seen.
pub async fn solve<D: Driver>(
    driver: &D,
    q: &ApiQuestion,
    llm: Option<&CodexClient>,
    max_llm_calls: u64,
) -> PageResult<QuestionResult> {
    let mut queue: VecDeque<String> = candidates(q).into();
    let mut tried: Vec<String> = Vec::new();
    let mut hint: Option<String> = None;
    let mut llm_calls = 0;
    loop {
        let next = match queue.pop_front() {
            Some(answer) => answer,
            None if q.kind == ApiKind::ShortAnswer && llm.is_some() && llm_calls < max_llm_calls => {
                llm_calls += 1;
                match ask_llm(llm.expect("checked above"), q, &tried, hint.as_deref()).await? {
                    Some(answer) => answer,
                    None => continue,
                }
            }
            None => break,
        };
        if tried.iter().any(|t| t.eq_ignore_ascii_case(&next)) {
            continue;
        }
        let script = match q.kind {
            ApiKind::ShortAnswer => TYPE_BLANK,
            ApiKind::Other(_) => return Ok(QuestionResult::Unhandled),
            _ => CLICK_TEXT,
        };
        if driver.execute_script(script, &[json!(next)]).await? != json!(true) {
            logger::warn(
                "Nothing on the page to answer with",
                [("answer", json!(next)), ("type", json!(q.kind.name()))],
            );
            return Ok(QuestionResult::Unhandled);
        }
        tried.push(next);
        click_button(driver, "Check").await?;
        let check = wait_for_check_feedback(driver).await.map_err(PageError)?;
        if check.outcome == Outcome::Correct {
            return Ok(QuestionResult::Finished { outcome: Outcome::Correct, attempts: tried.len() as u32 });
        }
        hint = check.hint.or(hint);
    }
    Ok(QuestionResult::Finished { outcome: Outcome::Incorrect, attempts: tried.len() as u32 })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::fake::FakePage;
    use crate::browser::Scope;
    use std::cell::RefCell;

    fn vocab(feedback: &str) -> ApiQuestion {
        ApiQuestion {
            no: 1,
            kind: ApiKind::Vocabulary,
            text: "The historic building had a certain ____ that drew in tourists.".into(),
            choices: ["exposure", "oversaturated", "cachet"].map(String::from).into(),
            correct_feedback: feedback.into(),
        }
    }

    /// A page where `right` is the correct answer. Records every word clicked or typed.
    fn page<'a>(
        right: &'a str,
        answered: &'a RefCell<Vec<String>>,
    ) -> FakePage<impl FnMut(Scope, &str, &[Value]) -> PageResult<Value> + 'a> {
        FakePage::new(move |_: Scope, code: &str, args: &[Value]| {
            Ok(if code == CLICK_TEXT || code == TYPE_BLANK {
                answered.borrow_mut().push(args[0].as_str().unwrap().to_string());
                json!(true)
            } else if code.contains("'Correct!'") {
                let last = answered.borrow().last().cloned().unwrap_or_default();
                json!({ "outcome": if last == right { "Correct!" } else { "Incorrect!" }, "hint": null })
            } else {
                json!(true)
            })
        })
    }

    #[tokio::test]
    async fn the_word_the_feedback_names_is_right_on_the_first_check() {
        let answered = RefCell::new(Vec::new());
        let result = solve(&page("cachet", &answered), &vocab("Great. Cachet refers to charm"), None, 0).await;
        assert_eq!(result, Ok(QuestionResult::Finished { outcome: Outcome::Correct, attempts: 1 }));
        assert_eq!(*answered.borrow(), ["cachet"]);
    }

    #[tokio::test]
    async fn without_a_named_word_every_choice_is_tried_in_order_until_correct() {
        let answered = RefCell::new(Vec::new());
        let result = solve(&page("cachet", &answered), &vocab("Well done."), None, 0).await;
        assert_eq!(result, Ok(QuestionResult::Finished { outcome: Outcome::Correct, attempts: 3 }));
        assert_eq!(*answered.borrow(), ["exposure", "oversaturated", "cachet"]);
    }

    #[tokio::test]
    async fn a_word_missing_from_the_page_stops_as_unhandled() {
        let page = FakePage::new(|_: Scope, code: &str, _: &[Value]| Ok(json!(code != CLICK_TEXT)));
        assert_eq!(solve(&page, &vocab("Well done."), None, 0).await, Ok(QuestionResult::Unhandled));
    }

    #[tokio::test]
    async fn a_short_answer_types_the_quoted_phrase_and_gives_up_without_a_model() {
        let q = ApiQuestion {
            kind: ApiKind::ShortAnswer,
            choices: Vec::new(),
            correct_feedback: "The suitable answer for the gap is 'had taken'.".into(),
            ..vocab("")
        };
        let answered = RefCell::new(Vec::new());
        let result = solve(&page("had taken", &answered), &q, None, 0).await;
        assert_eq!(result, Ok(QuestionResult::Finished { outcome: Outcome::Correct, attempts: 1 }));

        let silent = ApiQuestion { correct_feedback: "Correct.".into(), ..q };
        let answered = RefCell::new(Vec::new());
        let result = solve(&page("had taken", &answered), &silent, None, 0).await;
        assert_eq!(result, Ok(QuestionResult::Finished { outcome: Outcome::Incorrect, attempts: 0 }));
    }
}

/// Needs a debug browser on CDP_PORT (default 9222) showing a native question:
/// `cargo test live_question_api -- --ignored --nocapture`. Reloads the tab once; never answers.
#[cfg(test)]
mod live {
    use super::*;
    use crate::question_api::find_on_page;

    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn live_question_api_finds_the_question_on_screen() {
        let port = std::env::var("CDP_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(9222);
        let lms = crate::browser::open_lms(port).await.expect("a debug browser with an LMS tab");
        let questions = load_questions(&lms).await.unwrap();
        assert!(!questions.is_empty(), "the reload carried the question API");
        let text = lms.execute_script(PAGE_TEXT, &[]).await.unwrap();
        let q = find_on_page(&questions, text.as_str().unwrap()).expect("the on-screen question is in the API");
        println!(
            "on screen: no {} {:?} {:?}\nchoices {:?}\ntry order {:?}",
            q.no,
            q.kind,
            q.text,
            q.choices,
            candidates(q)
        );
    }
}
