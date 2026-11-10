//! Port of `src/runners/reading.js`.
//! Site behavior this relies on: docs/platform-notes.md#grouped-reading-exercise

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::json;

use super::exercise::read_result_and_click_next;
use super::{ActivityOutcome, ActivityReport};
use crate::browser::{get_current_question_dom, DomWait, Driver, FramedPage, PageError, PageResult, Scoped};
use crate::logger;

const LETTERS: [&str; 6] = ["A", "B", "C", "D", "E", "F"];

/// Correct answers lock and wrong ones re-enable, so all-locked means correct. Passes are capped.
const MAX_PASSES: usize = 12;

/// Letters tried per option group, keyed by pill, group index and the group's first option text.
pub type Tried = HashMap<String, Vec<String>>;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Acted {
    pub key: String,
    pub letter: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckOutcome {
    Correct,
    Exhausted,
}

impl CheckOutcome {
    fn name(self) -> &'static str {
        match self {
            CheckOutcome::Correct => "correct",
            CheckOutcome::Exhausted => "exhausted",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmitResult {
    NoSubmit,
    /// "All answers must be correct", or no confirm appeared.
    Blocked,
    Done,
}

impl SubmitResult {
    fn name(self) -> &'static str {
        match self {
            SubmitResult::NoSubmit => "no-submit",
            SubmitResult::Blocked => "blocked",
            SubmitResult::Done => "done",
        }
    }
}

const READ_PILLS: &str = r#"
    return [...document.querySelectorAll('.bl-button__container')]
      .map((pill) => Number(pill.textContent.trim()))
      .filter((pill) => Number.isInteger(pill) && pill > 0)
      .filter((pill, index, all) => all.indexOf(pill) === index)
      .sort((a, b) => a - b);
  "#;

const CLICK_PILL: &str = r#"
    const target = String(arguments[0]);
    const pill = [...document.querySelectorAll('.bl-button__container')]
      .find((element) => element.textContent.trim() === target);
    if (pill) { pill.click(); return true; }
    return false;
  "#;

// Options are one flat list. A new group starts at each "A".
const ANSWER_OPEN_GROUPS: &str = r#"
    const tried = arguments[0] || {};
    const btns = [...document.querySelectorAll('button.bl-w-full.justify-content-start')]
      .filter((button) => button.offsetParent !== null);
    const groups = [];
    let cur = null;
    for (const b of btns) {
      const letter = b.textContent.trim()[0];
      if (letter === 'A') { cur = []; groups.push(cur); }
      if (cur) cur.push(b);
    }
    const acted = [];
    for (const [groupIndex, g] of groups.entries()) {
      const enabled = g.filter(b => !b.disabled);
      if (!enabled.length) continue;
      const key = [arguments[1], groupIndex, g[0].textContent.trim().slice(0, 60)].join(':');
      const done = tried[key] || [];
      const pick = enabled.find(b => !done.includes(b.textContent.trim()[0])) || enabled[0];
      const letter = pick.textContent.trim()[0];
      pick.click();
      acted.push({ key, letter });
    }
    return acted;
  "#;

const HAS_CHECK_BUTTON: &str = r#"
    return !![...document.querySelectorAll('button')].find(b => /^Check( My Answer)?$/i.test(b.textContent.trim()) && !b.disabled);
  "#;

const COUNT_ENABLED_OPTIONS: &str = r#"
    return [...document.querySelectorAll('button.bl-w-full.justify-content-start')]
      .filter((button) => button.offsetParent !== null && !button.disabled).length;
  "#;

const SELECT_LETTER: &str = r#"
      const letter = arguments[0];
      const opts = [...document.querySelectorAll('button.bl-w-full.justify-content-start')];
      if (!opts.some(b => !b.disabled)) return 'locked';
      const target = opts.find(b => !b.disabled && b.textContent.trim()[0] === letter);
      if (!target) return 'no-letter';
      target.click();
      return 'selected';
    "#;

const CLICK_CHECK: &str = r#"const c = [...document.querySelectorAll('button')].find(b => /^Check( My Answer)?$/i.test(b.textContent.trim()) && !b.disabled); if (c) c.click();"#;

const CHECK_HEADING: &str = r#"
      const h = [...document.querySelectorAll('h1,h2,h3,h4,h5,h6')].find(x => /^(Correct!|Incorrect!)$/i.test(x.textContent.trim()));
      return h ? h.textContent.trim() : null;
    "#;

const OPEN_SUBMIT: &str = r#"
    const s = [...document.querySelectorAll('button')].find(b => b.textContent.trim() === 'Submit' && !b.disabled);
    if (s) { s.click(); return true; }
    return false;
  "#;

const SUBMIT_BLOCKED: &str = "return /All answers must be correct/i.test(document.body.innerText);";

const CLICK_CLOSE: &str =
    "const c = [...document.querySelectorAll('button')].find(b => b.textContent.trim() === 'Close'); if (c) c.click();";

const CONFIRM_YES: &str = r#"
    const y = [...document.querySelectorAll('button')].find(b => b.textContent.trim() === 'Yes' && b.offsetParent !== null);
    if (y) { y.click(); return true; }
    return false;
  "#;

pub async fn read_pills<D: Driver>(driver: &D) -> PageResult<Vec<u32>> {
    let pills = driver.execute_script(READ_PILLS, &[]).await?;
    serde_json::from_value(pills).map_err(|e| PageError(format!("reading pills: {e}")))
}

pub async fn click_pill<D: Driver>(driver: &D, n: u32) -> PageResult<bool> {
    Ok(driver.execute_script(CLICK_PILL, &[json!(n.to_string())]).await? == json!(true))
}

/// Questions without Check are graded by leaving them, so the next pill (wrapping) is clicked.
pub async fn leave_by_next_pill<D: Driver>(driver: &D, pills: &[u32], current: u32) -> PageResult<bool> {
    if pills.is_empty() {
        return Ok(false);
    }
    let next_index = pills.iter().position(|&p| p == current).map_or(0, |i| (i + 1) % pills.len());
    let next = pills[next_index];
    if next == current {
        return Ok(false);
    }
    let clicked = click_pill(driver, next).await?;
    if clicked {
        driver.sleep(1000).await;
    }
    Ok(clicked)
}

/// Clicks one untried letter in every open group on the pill and records it in `tried`,
/// so repeat visits walk A, B, C, D.
pub async fn answer_open_groups<D: Driver>(driver: &D, tried: &mut Tried, pill: u32) -> PageResult<Vec<Acted>> {
    let acted = driver.execute_script(ANSWER_OPEN_GROUPS, &[json!(tried), json!(pill)]).await?;
    let acted: Vec<Acted> = serde_json::from_value(acted).map_err(|e| PageError(format!("reading groups: {e}")))?;
    for Acted { key, letter } in &acted {
        let letters = tried.entry(key.clone()).or_default();
        if !letters.contains(letter) {
            letters.push(letter.clone());
        }
    }
    Ok(acted)
}

async fn has_check_button<D: Driver>(driver: &D) -> PageResult<bool> {
    Ok(driver.execute_script(HAS_CHECK_BUTTON, &[]).await? == json!(true))
}

async fn count_enabled_options<D: Driver>(driver: &D) -> PageResult<u64> {
    Ok(driver.execute_script(COUNT_ENABLED_OPTIONS, &[]).await?.as_u64().unwrap_or(0))
}

/// A Check question grades in place: wrong answers keep options enabled, so cycle letters until "Correct!".
async fn answer_check_question<D: Driver>(driver: &D) -> PageResult<CheckOutcome> {
    for letter in LETTERS {
        match driver.execute_script(SELECT_LETTER, &[json!(letter)]).await?.as_str() {
            // Already solved this visit.
            Some("locked") => return Ok(CheckOutcome::Correct),
            // Fewer options than letters.
            Some("no-letter") => continue,
            _ => {}
        }
        driver.sleep(400).await;
        driver.execute_script(CLICK_CHECK, &[]).await?;
        driver.sleep(1400).await;
        let outcome = driver.execute_script(CHECK_HEADING, &[]).await?;
        logger::info("Reading exercise: check attempt", [("letter", json!(letter)), ("outcome", outcome.clone())]);
        if outcome == json!("Correct!") {
            return Ok(CheckOutcome::Correct);
        }
    }
    Ok(CheckOutcome::Exhausted)
}

/// Submit refuses with "All answers must be correct" until every item is right.
async fn try_submit<D: Driver>(driver: &D) -> PageResult<SubmitResult> {
    if driver.execute_script(OPEN_SUBMIT, &[]).await? != json!(true) {
        return Ok(SubmitResult::NoSubmit);
    }
    driver.sleep(1200).await;

    if driver.execute_script(SUBMIT_BLOCKED, &[]).await? == json!(true) {
        driver.execute_script(CLICK_CLOSE, &[]).await?;
        driver.sleep(600).await;
        return Ok(SubmitResult::Blocked);
    }

    if driver.execute_script(CONFIRM_YES, &[]).await? != json!(true) {
        driver.execute_script(CLICK_CLOSE, &[]).await?;
        return Ok(SubmitResult::Blocked);
    }
    driver.sleep(3000).await;
    Ok(SubmitResult::Done)
}

pub async fn run<P: FramedPage>(page: &P) -> PageResult<ActivityReport> {
    let mut tried = Tried::new();

    // Skip the long LTI iframe wait; the layout renders in an iframe or at top level.
    let question = get_current_question_dom(page, DomWait::iframe_wait(300)).await?;
    let driver = Scoped::new(page, question.scope);
    let pills = read_pills(&driver).await?;
    let tile_count = pills.len();
    if tile_count < 2 {
        logger::warn("Reading exercise: grouped pill layout not found", [("tileCount", json!(tile_count))]);
        return Ok(ActivityReport::new(ActivityOutcome::Unhandled));
    }
    logger::info("Reading exercise: starting", [("tileCount", json!(tile_count))]);

    for pass in 0..MAX_PASSES {
        let mut answered_this_pass = 0;

        for &n in &pills {
            click_pill(&driver, n).await?;
            driver.sleep(800).await;

            // All locked means correct.
            if count_enabled_options(&driver).await? == 0 {
                continue;
            }

            // Check questions resolve in one visit; the others are seen locked on a later pass.
            if has_check_button(&driver).await? {
                let outcome = answer_check_question(&driver).await?;
                answered_this_pass += 1;
                driver.sleep(600).await;
                leave_by_next_pill(&driver, &pills, n).await?;
                logger::info(
                    "Reading exercise: check question",
                    [("pass", json!(pass)), ("tile", json!(n)), ("outcome", json!(outcome.name()))],
                );
                continue;
            }

            let acted = answer_open_groups(&driver, &mut tried, n).await?;
            if acted.is_empty() {
                continue;
            }
            answered_this_pass += acted.len();
            driver.sleep(400).await;
            leave_by_next_pill(&driver, &pills, n).await?;
            let letters: Vec<&str> = acted.iter().map(|a| a.letter.as_str()).collect();
            logger::info(
                "Reading exercise: answered",
                [
                    ("pass", json!(pass)),
                    ("tile", json!(n)),
                    ("groups", json!(acted.len())),
                    ("letters", json!(letters)),
                ],
            );
        }

        let result = try_submit(&driver).await?;
        logger::info(
            "Reading exercise: pass done",
            [("pass", json!(pass)), ("answeredThisPass", json!(answered_this_pass)), ("submit", json!(result.name()))],
        );
        if result == SubmitResult::Done {
            let score = read_result_and_click_next(page).await?;
            logger::info("Reading exercise: complete", [("questions", json!(tile_count)), ("score", json!(score))]);
            return Ok(ActivityReport::new(ActivityOutcome::Complete).questions(tile_count).score(score));
        }

        if answered_this_pass == 0 {
            // Everything is locked but Submit still refuses, so a locked item is wrong.
            logger::warn("Reading exercise: stuck — all items locked but Submit blocked", [("pass", json!(pass))]);
            return Ok(ActivityReport::new(ActivityOutcome::Stuck));
        }
    }

    logger::warn("Reading exercise: hit pass cap without finishing", [("passes", json!(MAX_PASSES))]);
    Ok(ActivityReport::new(ActivityOutcome::Incomplete))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::fake::FakePage;
    use crate::browser::Scope;
    use serde_json::Value;
    use std::cell::RefCell;

    #[tokio::test]
    async fn deferred_grouped_reading_traversal_leaves_each_pill_through_the_next_numbered_pill_including_the_final_pill(
    ) {
        let visits = RefCell::new(Vec::new());
        let mut submissions = Vec::new();
        let page = FakePage::new(|_: Scope, _: &str, args: &[Value]| -> PageResult<Value> {
            visits.borrow_mut().push(args[0].as_str().unwrap().parse::<u32>().unwrap());
            Ok(json!(true))
        });

        visits.borrow_mut().push(1);
        leave_by_next_pill(&page, &[1, 2, 3], 1).await.unwrap();
        leave_by_next_pill(&page, &[1, 2, 3], 2).await.unwrap();
        leave_by_next_pill(&page, &[1, 2, 3], 3).await.unwrap();
        submissions.push(visits.borrow().clone());

        assert_eq!(*visits.borrow(), vec![1, 2, 3, 1]);
        assert_eq!(submissions, vec![vec![1, 2, 3, 1]], "the final pill is left before submit");
    }

    #[test]
    fn deferred_grouped_reading_path_contains_no_save_or_generic_next_progression_controls() {
        let source = include_str!("reading.rs");
        assert!(!source.contains(concat!("grade", "Current")));
        assert!(!source.contains(concat!("=== '", "Save'")));
        assert!(!source.contains(concat!("=== '", "Save & Next'")));
    }

    #[tokio::test]
    async fn grouped_reading_tried_letters_stay_isolated_per_run_local_map() {
        let picks = RefCell::new(Vec::new());
        let page = FakePage::new(|_: Scope, _: &str, args: &[Value]| -> PageResult<Value> {
            let pill = args[1].as_u64().unwrap();
            let key = format!("{pill}:0:A option");
            let done: Vec<String> =
                args[0].get(&key).map(|d| serde_json::from_value(d.clone()).unwrap()).unwrap_or_default();
            let letter = if done.iter().any(|l| l == "A") { "B" } else { "A" };
            picks.borrow_mut().push((pill, done, letter));
            Ok(json!([{ "key": key, "letter": letter }]))
        });

        let mut first_run_tried = Tried::new();
        answer_open_groups(&page, &mut first_run_tried, 1).await.unwrap();
        answer_open_groups(&page, &mut first_run_tried, 1).await.unwrap();

        let mut second_run_tried = Tried::new();
        answer_open_groups(&page, &mut second_run_tried, 1).await.unwrap();

        assert_eq!(*picks.borrow(), vec![(1, vec![], "A"), (1, vec!["A".to_string()], "B"), (1, vec![], "A")]);
    }
}
