//! The Beelingua question API (`.../Student/Question`): what each native question asks, read from
//! the app's own JSON instead of its markup. Pure.
//! Site behavior this relies on: docs/platform-notes.md#question-api

use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use crate::question_types::option_buttons::strip_tags;

/// The attempt payload's URL ends here. The review payload ends in `/Student/Review`.
pub const QUESTION_API: &str = "/Student/Question";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiKind {
    MultipleChoice,
    /// A word bank for a `____` blank.
    Vocabulary,
    /// Pick the wrong word in the sentence.
    ErrorRecognition,
    /// Type the words for a `____` blank.
    ShortAnswer,
    Other(String),
}

impl ApiKind {
    fn parse(question_type: &str) -> ApiKind {
        match question_type {
            "Multiple Choice" => ApiKind::MultipleChoice,
            "BiTS Vocabulary" => ApiKind::Vocabulary,
            "BiTS Error Recognition" => ApiKind::ErrorRecognition,
            "BiTS Short Answer" => ApiKind::ShortAnswer,
            other => ApiKind::Other(other.to_string()),
        }
    }

    pub fn name(&self) -> &str {
        match self {
            ApiKind::MultipleChoice => "multipleChoice",
            ApiKind::Vocabulary => "bitsVocabulary",
            ApiKind::ErrorRecognition => "bitsErrorRecognition",
            ApiKind::ShortAnswer => "bitsShortAnswer",
            ApiKind::Other(name) => name,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiQuestion {
    pub no: u32,
    pub kind: ApiKind,
    /// Plain text, with the blank kept as `____`.
    pub text: String,
    /// Plain text, in API order. Empty for short answers.
    pub choices: Vec<String>,
    pub correct_feedback: String,
}

/// The questions of a `Student/Question` body, or of a `Student/Review` body (which has no choices).
pub fn parse_questions(body: &Value) -> Vec<ApiQuestion> {
    let Some(items) = body["question"].as_array() else { return Vec::new() };
    items
        .iter()
        .map(|q| ApiQuestion {
            no: q["no"].as_u64().unwrap_or_default() as u32,
            kind: ApiKind::parse(q["questionType"].as_str().unwrap_or_default()),
            text: plain(q["questionText"].as_str().unwrap_or_default()),
            choices: q["answer"]
                .as_array()
                .map(|a| a.iter().map(|c| plain(c["text"].as_str().unwrap_or_default())).collect())
                .unwrap_or_default(),
            correct_feedback: q["correctFeedback"].as_str().or(q["feedback"].as_str()).unwrap_or_default().to_string(),
        })
        .collect()
}

fn plain(html: &str) -> String {
    strip_tags(html).replace("&amp;", "&").replace("&#39;", "'").replace("&quot;", "\"")
}

/// A blank is any run of underscores: `____` in BiTS items, `___________` in checkpoints.
static BLANK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"_{3,}").unwrap());

static QUOTED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"["'‘’“”]([^"'‘’“”]{1,60})["'‘’“”]"#).unwrap());

/// Answers to try, best first. The correct-answer feedback ships with the question and often
/// names the answer: measured on 2103 reviewed questions, it names the right vocabulary word 87%
/// of the time and quotes the right short answer first 48% of the time. Choices it names go
/// first, then the rest in API order, so every choice is still tried. Short answers get the
/// quoted phrases only, and the caller asks the LLM when they run out.
pub fn candidates(q: &ApiQuestion) -> Vec<String> {
    let feedback = q.correct_feedback.to_lowercase();
    match q.kind {
        ApiKind::ShortAnswer => {
            let mut quoted: Vec<String> = Vec::new();
            for c in QUOTED.captures_iter(&q.correct_feedback) {
                let phrase = c[1].trim().to_string();
                if !phrase.is_empty() && !quoted.contains(&phrase) {
                    quoted.push(phrase);
                }
            }
            quoted
        }
        _ => {
            let (named, rest): (Vec<&String>, Vec<&String>) =
                q.choices.iter().partition(|c| !c.is_empty() && feedback.contains(&c.to_lowercase()));
            named.into_iter().chain(rest).cloned().collect()
        }
    }
}

fn words(text: &str) -> String {
    text.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ").to_lowercase()
}

/// The question on screen: the one whose sentence, split at its blank, appears in the page text.
/// Matching on text, not on a counter, keeps a resumed draft that opens on any pill in step.
pub fn find_on_page<'a>(questions: &'a [ApiQuestion], page_text: &str) -> Option<&'a ApiQuestion> {
    let page = words(page_text);
    questions
        .iter()
        .filter(|q| {
            let parts: Vec<String> = BLANK.split(&q.text).map(words).filter(|p| !p.is_empty()).collect();
            !parts.is_empty() && parts.iter().all(|p| page.contains(p.as_str()))
        })
        .max_by_key(|q| q.text.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn vocab() -> ApiQuestion {
        ApiQuestion {
            no: 1,
            kind: ApiKind::Vocabulary,
            text: "The historic building had a certain ____ that drew in tourists.".into(),
            choices: ["exposure", "oversaturated", "whitewash", "paparazzi", "cachet"].map(String::from).into(),
            correct_feedback: "Great. Cachet Refers to a particular quality or charm".into(),
        }
    }

    #[test]
    fn parses_an_attempt_payload() {
        let body = json!({ "question": [{
            "no": 1,
            "questionText": "<p>The historic building had a certain ____ that drew in tourists.</p>",
            "questionType": "BiTS Vocabulary",
            "answerType": "Single Answer",
            "answer": [{ "id": "a", "text": "<p>exposure</p>" }, { "id": "b", "text": "<p>cachet</p>" }],
            "correctFeedback": "Great. Cachet Refers to a particular quality or charm",
        }, { "no": 2, "questionText": "<p>x</p>", "questionType": "Drag Sort", "answer": null }] });
        let qs = parse_questions(&body);
        assert_eq!(qs[0].kind, ApiKind::Vocabulary);
        assert_eq!(qs[0].text, "The historic building had a certain ____ that drew in tourists.");
        assert_eq!(qs[0].choices, vec!["exposure".to_string(), "cachet".to_string()]);
        assert_eq!(qs[1].kind, ApiKind::Other("Drag Sort".into()));
        assert!(qs[1].choices.is_empty());
    }

    #[test]
    fn tries_the_choice_the_feedback_names_first_then_every_other_choice() {
        assert_eq!(candidates(&vocab()), ["cachet", "exposure", "oversaturated", "whitewash", "paparazzi"]);
    }

    #[test]
    fn a_short_answer_tries_the_quoted_phrases_in_order() {
        let q = ApiQuestion {
            kind: ApiKind::ShortAnswer,
            choices: Vec::new(),
            correct_feedback: "Great. The suitable answer for the gap is 'had taken'. Not 'took' or 'had taken'."
                .into(),
            ..vocab()
        };
        assert_eq!(candidates(&q), ["had taken", "took"]);
        let silent = ApiQuestion { correct_feedback: "Correct. Simple present.".into(), ..q };
        assert!(candidates(&silent).is_empty());
    }

    #[test]
    fn finds_the_question_on_screen_by_its_sentence() {
        let other = ApiQuestion { no: 2, text: "The politician attempted to ____ the scandal.".into(), ..vocab() };
        let qs = [other, vocab()];
        let page = "1 2 3\nMatch the appropriate vocabulary...\nThe historic building had a certain\nthat drew in tourists.\nexposure cachet\nSkip Check";
        assert_eq!(find_on_page(&qs, page).map(|q| q.no), Some(1));
        assert_eq!(find_on_page(&qs, "Unit 2 Skip Check"), None);
        let long_blank =
            ApiQuestion { no: 3, text: "Her determination enabled her to ___________ .".into(), ..vocab() };
        assert_eq!(find_on_page(&[long_blank], "3.\nHer determination enabled her to\n.").map(|q| q.no), Some(3));
    }
}
