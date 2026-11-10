//! Per-runner strategy tables and the non-LLM answerers from `src/runners/iframe.js`
//! and `src/runners/exercise.js`.

use serde_json::{json, Value};

use crate::answer_loop::Answerer;
use crate::llm::LlmError;
use crate::question_types::{ErrorAnalysisData, OptionButton, QuestionData, QuestionKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Solve {
    TryEachOption,
    LetterCombinations,
    Llm,
    FromPage,
    ClickThrough,
}

impl Solve {
    /// The strategy's name in the JS tables.
    pub fn name(self) -> &'static str {
        match self {
            Solve::TryEachOption => "try-each-option",
            Solve::LetterCombinations => "letter-combinations",
            Solve::Llm => "llm",
            Solve::FromPage => "from-page",
            Solve::ClickThrough => "click-through",
        }
    }
}

/// The iframe runner's table, in detection order: the first type whose detect matches wins.
pub const IFRAME_QUESTION_TYPES: [(QuestionKind, Solve); 4] = [
    (QuestionKind::FillInBlank, Solve::Llm),
    (QuestionKind::ErrorAnalysis, Solve::LetterCombinations),
    // Answer derived from the page's own data-drag/data-drag-target indexes.
    (QuestionKind::QuizMatching, Solve::FromPage),
    // Ungraded slides; the handler's answer clicks #/n itself, so the generic advance is skipped.
    (QuestionKind::VocabularyIntro, Solve::ClickThrough),
];

/// The exercise runner registers these two and tries each option in turn for both.
pub const EXERCISE_QUESTION_TYPES: [(QuestionKind, Solve); 2] = [
    (QuestionKind::AudioMultipleChoice, Solve::TryEachOption),
    (QuestionKind::ReadingComprehension, Solve::TryEachOption),
];

pub fn solve_for(table: &[(QuestionKind, Solve)], kind: QuestionKind) -> Option<Solve> {
    table.iter().find(|(k, _)| *k == kind).map(|(_, solve)| *solve)
}

/// The iframe runner's `retryLimit`: from-page answers are deterministic, so they get one attempt.
pub fn iframe_retry_limit(solve: Solve, max_answer_retries: u64) -> u64 {
    if solve == Solve::FromPage {
        0
    } else {
        max_answer_retries
    }
}

const FILL_IN_BLANK_FORMAT_NOTE: &str = "Return ONLY the word(s) that fill the blank(s) — never the surrounding sentence, even though the worked example above shows the answer inside a full sentence. If multiple blanks, return {\"answers\": [...]} in order (each entry just the words for that one blank); if one blank, return {\"answer\": \"...\"} with just those words.";

/// The on-screen instruction is used verbatim, because wording and the graded rule vary per question.
pub fn build_instruction(kind: QuestionKind, instruction_text: Option<&str>) -> String {
    let on_screen = match instruction_text {
        Some(text) if !text.is_empty() => text,
        _ => "Answer the question as shown.",
    };
    // The on-screen worked example answers in a full sentence, and the model copies that shape.
    let note = if kind == QuestionKind::FillInBlank { FILL_IN_BLANK_FORMAT_NOTE } else { "" };
    format!("{on_screen} {note}")
}

/// Letter subsets by size (A, B, ..., A+B, ...), one per retry.
pub fn get_letter_combinations<T: Clone>(letters: &[T]) -> Vec<Vec<T>> {
    fn build<T: Clone>(letters: &[T], start: usize, size: usize, current: &mut Vec<T>, out: &mut Vec<Vec<T>>) {
        if current.len() == size {
            out.push(current.clone());
            return;
        }
        let last_start = letters.len() - (size - current.len());
        for i in start..=last_start {
            current.push(letters[i].clone());
            build(letters, i + 1, size, current, out);
            current.pop();
        }
    }

    let mut combinations = Vec::new();
    for size in 1..=letters.len() {
        build(letters, 0, size, &mut Vec::new(), &mut combinations);
    }
    combinations
}

/// `createBruteForceAnswerer`: the combinations come from the first question data it sees.
#[derive(Debug, Clone, Default)]
pub struct BruteForceAnswerer {
    attempt_index: usize,
    combinations: Option<Vec<Vec<Option<char>>>>,
}

impl BruteForceAnswerer {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Answerer<ErrorAnalysisData> for BruteForceAnswerer {
    async fn answer(
        &mut self,
        _instruction: &str,
        data: &ErrorAnalysisData,
        _feedback: Option<&str>,
    ) -> Result<Value, LlmError> {
        let combinations = self.combinations.get_or_insert_with(|| {
            let letters: Vec<Option<char>> = data.candidates.iter().map(|c| c.letter).collect();
            get_letter_combinations(&letters)
        });
        let candidate_letters = combinations.get(self.attempt_index).cloned();
        self.attempt_index += 1;
        match candidate_letters {
            Some(letters) => Ok(json!({ "answers": letters })),
            None => Err(LlmError::Failed(
                "errorAnalysis brute force: exhausted all candidate-letter combinations without a correct answer"
                    .to_string(),
            )),
        }
    }
}

/// The iframe runner hands every type the parsed enum; only errorAnalysis data has candidates.
impl Answerer<QuestionData> for BruteForceAnswerer {
    async fn answer(
        &mut self,
        instruction: &str,
        data: &QuestionData,
        feedback: Option<&str>,
    ) -> Result<Value, LlmError> {
        match data {
            QuestionData::ErrorAnalysis(data) => {
                Answerer::<ErrorAnalysisData>::answer(self, instruction, data, feedback).await
            }
            _ => Err(LlmError::Failed("errorAnalysis brute force: question data has no candidates".to_string())),
        }
    }
}

/// `tryEachOption`: cycles through the options captured at creation, whatever the question data.
#[derive(Debug, Clone)]
pub struct TryEachOption {
    options: Vec<OptionButton>,
    attempt: usize,
}

impl TryEachOption {
    pub fn new(options: Vec<OptionButton>) -> Self {
        TryEachOption { options, attempt: 0 }
    }
}

impl<D> Answerer<D> for TryEachOption {
    async fn answer(&mut self, _instruction: &str, _data: &D, _feedback: Option<&str>) -> Result<Value, LlmError> {
        if self.options.is_empty() {
            return Err(LlmError::Failed("undefined is not an object (evaluating 'option.letter')".to_string()));
        }
        let option = &self.options[self.attempt % self.options.len()];
        self.attempt += 1;
        // A missing letter is undefined in JS, which drops the key.
        Ok(match option.letter {
            Some(letter) => json!({ "answer": letter }),
            None => json!({}),
        })
    }
}

/// The from-page and click-through answerer: the handler finds the answer itself.
#[derive(Debug, Clone, Copy, Default)]
pub struct EmptyAnswer;

impl<D> Answerer<D> for EmptyAnswer {
    async fn answer(&mut self, _instruction: &str, _data: &D, _feedback: Option<&str>) -> Result<Value, LlmError> {
        Ok(json!({}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::question_types::Candidate;

    fn question_data(letters: &[char]) -> ErrorAnalysisData {
        ErrorAnalysisData {
            sentence: String::new(),
            candidates: letters.iter().map(|&l| Candidate { letter: Some(l), word: String::new() }).collect(),
            instruction_text: String::new(),
        }
    }

    async fn answers(answerer: &mut BruteForceAnswerer, data: &ErrorAnalysisData) -> Result<Value, LlmError> {
        answerer.answer("", data, None).await.map(|v| v["answers"].clone())
    }

    #[test]
    fn iframe_runner_uses_shared_max_answer_retry_config() {
        assert_eq!(iframe_retry_limit(Solve::FromPage, 25), 0);
        assert_eq!(iframe_retry_limit(Solve::Llm, 25), 25);
        assert_eq!(iframe_retry_limit(Solve::LetterCombinations, 7), 7);
    }

    #[tokio::test]
    async fn create_brute_force_answerer_tries_non_empty_candidate_letter_combinations_in_increasing_subset_size() {
        let mut answerer = BruteForceAnswerer::new();
        let data = question_data(&['A', 'B', 'C']);
        assert_eq!(answers(&mut answerer, &data).await, Ok(json!(["A"])));
        assert_eq!(answers(&mut answerer, &data).await, Ok(json!(["B"])));
        assert_eq!(answers(&mut answerer, &data).await, Ok(json!(["C"])));
        assert_eq!(answers(&mut answerer, &data).await, Ok(json!(["A", "B"])));
        assert_eq!(answers(&mut answerer, &data).await, Ok(json!(["A", "C"])));
        assert_eq!(answers(&mut answerer, &data).await, Ok(json!(["B", "C"])));
        assert_eq!(answers(&mut answerer, &data).await, Ok(json!(["A", "B", "C"])));
    }

    #[tokio::test]
    async fn create_brute_force_answerer_exhausts_only_after_all_candidate_letter_combinations_are_tried() {
        let mut answerer = BruteForceAnswerer::new();
        let data = question_data(&['A', 'B']);
        assert_eq!(answers(&mut answerer, &data).await, Ok(json!(["A"])));
        assert_eq!(answers(&mut answerer, &data).await, Ok(json!(["B"])));
        assert_eq!(answers(&mut answerer, &data).await, Ok(json!(["A", "B"])));
        let err = answers(&mut answerer, &data).await.unwrap_err();
        assert!(err.to_string().contains("exhausted all candidate-letter combinations"), "{err}");
    }

    #[test]
    fn iframe_runner_solves_each_question_type_with_its_fixed_strategy_in_detection_order() {
        let table: Vec<(&str, &str)> = IFRAME_QUESTION_TYPES.iter().map(|(k, s)| (k.name(), s.name())).collect();
        assert_eq!(
            table,
            vec![
                ("fillInBlank", "llm"),
                ("errorAnalysis", "letter-combinations"),
                ("quizMatching", "from-page"),
                ("vocabularyIntro", "click-through"),
            ]
        );
    }
}
