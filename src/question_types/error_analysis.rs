//! Port of `src/questionTypes/errorAnalysis.js`.

use std::sync::LazyLock;

use regex::{Captures, Regex};
use serde::Serialize;

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::lti_quiz::{extract_instruction_text, poll_with_submit_retry, strip_tags, CLICK_SUBMIT};
use super::option_buttons::letter_at;
use super::Dom;
use crate::answer_loop::{Check, Outcome};
use crate::browser::{get_current_question_dom, jittered, DomWait, Driver, FramedPage, PageError, Scoped};
use crate::js;

static BOLD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<b>([\s\S]*?)</b>").unwrap());
static UNDERLINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<u>([\s\S]*?)</u>").unwrap());

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Candidate {
    /// `None` past the sixth candidate, where the JS indexes past its letter list.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub letter: Option<char>,
    pub word: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorAnalysisData {
    pub sentence: String,
    pub candidates: Vec<Candidate>,
    pub instruction_text: String,
}

pub fn detect(dom: &Dom) -> bool {
    dom.inside_iframe && dom.outer_html.contains("quiz-input-radio")
}

fn letter_text(letter: Option<char>) -> String {
    letter.map_or_else(|| "undefined".to_string(), String::from)
}

pub fn parse(dom: &Dom) -> ErrorAnalysisData {
    let html = &dom.outer_html;

    // Candidates are <b><u> words on some pages and bare <u> on others.
    let bold_inners: Vec<&str> =
        BOLD.captures_iter(html).map(|c| c.get(1).unwrap().as_str()).filter(|inner| inner.contains("<u>")).collect();
    let source_html = if bold_inners.is_empty() { html.clone() } else { bold_inners.join(" ") };

    // Mark candidates inline so a repeated word keeps its position.
    let mut candidates = Vec::new();
    let marked_html = UNDERLINE.replace_all(&source_html, |c: &Captures| {
        let inner = &c[1];
        let word = strip_tags(inner);
        if word.is_empty() {
            return inner.to_string();
        }
        let letter = letter_at(candidates.len());
        let marked = format!(" [{0}]{word}[/{0}] ", letter_text(letter));
        candidates.push(Candidate { letter, word });
        marked
    });

    // Some pages mark candidates with overlay labels or skip a <u>, so pad to the radio count.
    let radio_count = html.matches("quiz-input-radio").count();
    for i in candidates.len()..radio_count {
        candidates.push(Candidate { letter: letter_at(i), word: String::new() });
    }

    ErrorAnalysisData {
        sentence: strip_tags(&marked_html),
        candidates,
        instruction_text: extract_instruction_text(html),
    }
}

const LETTERS: [&str; 6] = ["A", "B", "C", "D", "E", "F"];

const RADIOS_LOCKED: &str = r#"
    const inputs = document.querySelectorAll('.quiz-input-radio');
    return inputs.length > 0 && [...inputs].every((i) => i.disabled);
  "#;

// Not an exclusive radio group: several checks can show at once, so set exactly the requested ones.
const SET_RADIOS: &str = r#"
    const indexes = new Set(arguments[0]);
    const inputs = document.querySelectorAll('.quiz-input-radio');
    inputs.forEach((input, i) => {
      const shouldBeChecked = indexes.has(i);
      if (input.checked !== shouldBeChecked) input.click();
    });
    return [...indexes].every((index) => inputs[index]);
  "#;

/// Radios lock after one graded submit per page load. Reload and restore the hash to retry.
async fn reset_if_locked<P: FramedPage>(driver: &Scoped<'_, P>) -> Result<(), PageError> {
    if driver.execute_script(RADIOS_LOCKED, &[]).await? != json!(true) {
        return Ok(());
    }
    let hash = driver.execute_script("return window.location.hash", &[]).await?;
    driver.page.reload().await?;
    let reloaded = get_current_question_dom(driver.page, DomWait::default()).await?;
    if js::truthy(&hash) {
        Scoped::new(driver.page, reloaded.scope)
            .execute_script("window.location.hash = arguments[0];", &[hash])
            .await?;
        driver.sleep(1500).await;
    }
    Ok(())
}

/// `[...new Set(answers.length ? answers : [answer])]` keeping only the letters A to F.
fn requested_letters(llm_result: &Value) -> Vec<&'static str> {
    let picked = match llm_result.get("answers") {
        Some(Value::Array(answers)) if !answers.is_empty() => answers.iter().collect::<Vec<_>>(),
        _ => llm_result.get("answer").into_iter().collect(),
    };
    let mut letters = Vec::new();
    for letter in picked.into_iter().filter_map(Value::as_str) {
        if let Some(known) = LETTERS.iter().find(|l| **l == letter) {
            if !letters.contains(known) {
                letters.push(*known);
            }
        }
    }
    letters
}

pub async fn answer<P: FramedPage>(driver: &Scoped<'_, P>, llm_result: &Value) -> Result<(), String> {
    reset_if_locked(driver).await.map_err(|e| e.0)?;

    let letters = requested_letters(llm_result);
    let indexes: Vec<usize> = letters.iter().filter_map(|l| LETTERS.iter().position(|x| x == l)).collect();

    // Radios may not be rendered yet on the first attempt, and a throw would skip this letter for good.
    let mut clicked = false;
    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline {
        clicked = driver.execute_script(SET_RADIOS, &[json!(indexes)]).await.map_err(|e| e.0)? == json!(true);
        if clicked {
            break;
        }
        driver.sleep(300).await;
    }
    if !clicked {
        return Err(format!(
            "errorAnalysis.answer: no radio option found for requested letters \"{}\"",
            letters.join(",")
        ));
    }

    driver.sleep(jittered(100, 100)).await;
    driver.execute_script(CLICK_SUBMIT, &[]).await.map_err(|e| e.0)?;
    Ok(())
}

/// Only a visible .quiz-next-btn (or no radios left) is trusted. The banners are unreliable.
pub async fn check_result<D: Driver>(driver: &D) -> Result<Check, String> {
    let poll = r#"
      const nextBtn = document.querySelector('.quiz-next-btn');
      if (nextBtn && getComputedStyle(nextBtn).display !== 'none') return true;
      return document.querySelectorAll('.quiz-input-radio').length === 0;
    "#;
    let outcome = poll_with_submit_retry(driver, Duration::from_secs(5), poll, |v| {
        (*v == json!(true)).then_some(Outcome::Correct)
    })
    .await
    .map_err(|e| e.0)?;
    Ok(Check::new(outcome.unwrap_or(Outcome::Incorrect)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dom(html: &str) -> Dom {
        Dom { outer_html: html.to_string(), inside_iframe: true, has_visible_slider: None }
    }

    fn letters(data: &ErrorAnalysisData) -> Vec<Option<char>> {
        data.candidates.iter().map(|c| c.letter).collect()
    }

    const UNDERLINE_SAMPLE: &str = r#"
<div>
  <b><u>had went</u></b> to the store, and <b><u>bought</u></b> some milk,
  then <b><u>drove</u></b> home before it <b><u>rained</u></b>.
</div>
<div class="quiz-input-radio"></div>
<div class="quiz-input-radio"></div>
<div class="quiz-input-radio"></div>
<div class="quiz-input-radio"></div>
"#;

    const OVERLAY_SAMPLE: &str = r#"
<div>By the time the guests arrived, the organizers have already set up all the chairs.</div>
<div id="container-f" data-options="label: A"></div>
<div id="container-g" data-options="label: B"></div>
<div id="container-h" data-options="label: C"></div>
<div id="container-i" data-options="label: D"></div>
<div id="container-j" data-options="label: E"></div>
<input class="quiz-input-radio">
<input class="quiz-input-radio">
<input class="quiz-input-radio">
<input class="quiz-input-radio">
<input class="quiz-input-radio">
"#;

    const PARTIAL_UNDERLINE_SAMPLE: &str = r#"
<div>
  She left <b><u>her umbrella at</u></b> the cafe, so she <b><u>should have gotten</u></b> wet
  <b><u>on</u></b> <b><u>her way</u></b> home.
</div>
<input class="quiz-input-radio">
<input class="quiz-input-radio">
<input class="quiz-input-radio">
<input class="quiz-input-radio">
<input class="quiz-input-radio">
"#;

    #[test]
    fn detect_matches_an_error_analysis_quiz_input_radio_page() {
        assert!(detect(&dom(UNDERLINE_SAMPLE)));
    }

    #[test]
    fn parse_extracts_one_candidate_per_underlined_word() {
        let parsed = parse(&dom(UNDERLINE_SAMPLE));
        assert_eq!(letters(&parsed), vec![Some('A'), Some('B'), Some('C'), Some('D')]);
        assert_eq!(parsed.candidates[0].word, "had went");
    }

    #[test]
    fn parse_falls_back_to_one_candidate_per_radio_when_no_u_tags_are_present() {
        let parsed = parse(&dom(OVERLAY_SAMPLE));
        assert_eq!(letters(&parsed), vec![Some('A'), Some('B'), Some('C'), Some('D'), Some('E')]);
    }

    #[test]
    fn parse_pads_candidates_up_to_the_real_radio_count_when_fewer_u_tags_are_found_than_options() {
        let parsed = parse(&dom(PARTIAL_UNDERLINE_SAMPLE));
        assert_eq!(letters(&parsed), vec![Some('A'), Some('B'), Some('C'), Some('D'), Some('E')]);
        assert_eq!(parsed.candidates[0].word, "her umbrella at");
        assert_eq!(parsed.candidates[3].word, "her way");
        assert_eq!(parsed.candidates[4].word, "");
    }

    use crate::browser::fake::{Call, FakePage};
    use crate::browser::{PageResult, Scope, IFRAME_COUNT};
    use std::cell::{Cell, RefCell};

    /// This quiz grades one submit per page load; the radios then stay disabled for good.
    fn error_analysis_page<'a>(
        locked: &'a Cell<bool>,
        hash_set: &'a RefCell<Vec<Value>>,
    ) -> FakePage<impl FnMut(Scope, &str, &[Value]) -> PageResult<Value> + 'a> {
        let hash = RefCell::new(json!("#/question-8"));
        FakePage::new(move |_: Scope, code: &str, args: &[Value]| -> PageResult<Value> {
            if code == IFRAME_COUNT {
                return Ok(json!(1));
            }
            if code.contains("ltiForm") {
                return Ok(json!(false));
            }
            if code.contains("window.location.hash") && code.contains("return") {
                return Ok(hash.borrow().clone());
            }
            if code.contains("window.location.hash = arguments[0]") {
                *hash.borrow_mut() = args[0].clone();
                hash_set.borrow_mut().push(args[0].clone());
                return Ok(Value::Null);
            }
            if code.contains("document.documentElement.outerHTML") {
                return Ok(json!("<html></html>"));
            }
            if code.contains("every((i) => i.disabled)") {
                return Ok(json!(locked.get()));
            }
            if code.contains("const indexes = new Set(arguments[0])") {
                return Ok(json!(true));
            }
            Ok(Value::Null)
        })
    }

    fn refreshes<F>(page: &FakePage<F>, locked: &Cell<bool>) -> usize {
        let n = page.calls.borrow().iter().filter(|c| **c == Call::Reload).count();
        if n > 0 {
            locked.set(false);
        }
        n
    }

    #[tokio::test]
    async fn answer_clicks_directly_when_the_radio_group_is_not_locked() {
        let locked = Cell::new(false);
        let hash_set = RefCell::new(Vec::new());
        let page = error_analysis_page(&locked, &hash_set);
        answer(&Scoped::new(&page, Scope::Frame), &json!({ "answer": "B" })).await.unwrap();
        assert_eq!(refreshes(&page, &locked), 0);
        assert!(hash_set.borrow().is_empty());
    }

    #[tokio::test]
    async fn answer_selects_every_requested_letter_and_clears_previously_checked_ones() {
        let inputs = RefCell::new(vec![true, false, false]);
        let click_log = RefCell::new(Vec::new());
        let page = FakePage::new(|_: Scope, code: &str, args: &[Value]| -> PageResult<Value> {
            if code.contains("every((i) => i.disabled)") {
                return Ok(json!(false));
            }
            if code.contains("const indexes = new Set(arguments[0])") {
                let indexes: Vec<usize> = serde_json::from_value(args[0].clone()).unwrap();
                for (i, checked) in inputs.borrow_mut().iter_mut().enumerate() {
                    if *checked != indexes.contains(&i) {
                        *checked = !*checked;
                        click_log.borrow_mut().push(["A", "B", "C"][i]);
                    }
                }
                return Ok(json!(indexes.iter().all(|&i| i < inputs.borrow().len())));
            }
            Ok(Value::Null)
        });
        answer(&Scoped::new(&page, Scope::Frame), &json!({ "answers": ["B", "C"] })).await.unwrap();
        assert_eq!(*inputs.borrow(), vec![false, true, true]);
        assert_eq!(*click_log.borrow(), vec!["A", "B", "C"]);
    }

    // A one-shot check threw before the radios rendered, and the brute-forcer skipped a correct A.
    #[tokio::test]
    async fn answer_polls_for_the_radio_group_to_render_instead_of_throwing_on_the_first_not_ready_check() {
        let mut click_attempts = 0;
        let page = FakePage::new(|_: Scope, code: &str, _: &[Value]| -> PageResult<Value> {
            if code.contains("every((i) => i.disabled)") {
                return Ok(json!(false));
            }
            if code.contains("const indexes = new Set(arguments[0])") {
                click_attempts += 1;
                return Ok(json!(click_attempts >= 3));
            }
            Ok(Value::Null)
        });
        answer(&Scoped::new(&page, Scope::Frame), &json!({ "answer": "A" })).await.unwrap();
        drop(page);
        assert_eq!(click_attempts, 3);
    }

    #[tokio::test]
    async fn answer_reloads_and_re_navigates_to_the_same_hash_when_the_radio_group_is_already_locked() {
        let locked = Cell::new(true);
        let hash_set = RefCell::new(Vec::new());
        let page = error_analysis_page(&locked, &hash_set);
        answer(&Scoped::new(&page, Scope::Frame), &json!({ "answer": "C" })).await.unwrap();
        assert_eq!(refreshes(&page, &locked), 1);
        assert_eq!(*hash_set.borrow(), vec![json!("#/question-8")]);
    }

    #[tokio::test]
    async fn check_result_resolves_correct_once_quiz_next_btn_becomes_visible() {
        let mut calls = 0;
        let page = FakePage::new(|_: Scope, code: &str, _: &[Value]| -> PageResult<Value> {
            calls += 1;
            Ok(if code.contains("quiz-next-btn") { json!(calls >= 3) } else { Value::Null })
        });
        assert_eq!(check_result(&page).await, Ok(Check::new(Outcome::Correct)));
    }

    #[tokio::test]
    async fn check_result_resolves_incorrect_after_the_deadline_if_quiz_next_btn_never_appears_retrying_submit_once() {
        let mut submit_clicks = 0;
        let page = FakePage::new(|_: Scope, code: &str, _: &[Value]| -> PageResult<Value> {
            if code.contains("quiz-next-btn") {
                return Ok(json!(false));
            }
            if code.contains("quiz-submit-btn") {
                submit_clicks += 1;
            }
            Ok(Value::Null)
        });
        assert_eq!(check_result(&page).await, Ok(Check::new(Outcome::Incorrect)));
        drop(page);
        assert_eq!(submit_clicks, 1);
    }
}
