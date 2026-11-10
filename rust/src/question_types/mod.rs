//! Detection and parsing for each question type. Answering and result checks drive the
//! browser and come in a later phase.

pub mod audio_multiple_choice;
pub mod error_analysis;
pub mod fill_in_blank;
pub mod lti_quiz;
pub mod option_buttons;
pub mod quiz_matching;
pub mod reading_comprehension;
pub mod vocabulary_intro;

use serde::Serialize;

pub use audio_multiple_choice::AudioMultipleChoiceData;
pub use error_analysis::{Candidate, ErrorAnalysisData};
pub use fill_in_blank::FillInBlankData;
pub use option_buttons::OptionButton;
pub use quiz_matching::QuizMatchingData;
pub use reading_comprehension::ReadingComprehensionData;
pub use vocabulary_intro::VocabularyIntroData;

/// What the JS handlers read off the `dom` object built by `getCurrentQuestionDom`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Dom {
    pub outer_html: String,
    pub inside_iframe: bool,
    /// `None` is a fixture DOM without the field, which falls back to a substring check.
    pub has_visible_slider: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum QuestionKind {
    AudioMultipleChoice,
    ReadingComprehension,
    FillInBlank,
    ErrorAnalysis,
    QuizMatching,
    VocabularyIntro,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum QuestionData {
    AudioMultipleChoice(AudioMultipleChoiceData),
    ReadingComprehension(ReadingComprehensionData),
    FillInBlank(FillInBlankData),
    ErrorAnalysis(ErrorAnalysisData),
    QuizMatching(QuizMatchingData),
    VocabularyIntro(VocabularyIntroData),
}

impl QuestionKind {
    /// The JS module's `name`.
    pub fn name(self) -> &'static str {
        match self {
            QuestionKind::AudioMultipleChoice => "audioMultipleChoice",
            QuestionKind::ReadingComprehension => "readingComprehension",
            QuestionKind::FillInBlank => "fillInBlank",
            QuestionKind::ErrorAnalysis => "errorAnalysis",
            QuestionKind::QuizMatching => "quizMatching",
            QuestionKind::VocabularyIntro => "vocabularyIntro",
        }
    }

    pub fn detect(self, dom: &Dom) -> bool {
        match self {
            QuestionKind::AudioMultipleChoice => audio_multiple_choice::detect(dom),
            QuestionKind::ReadingComprehension => reading_comprehension::detect(dom),
            QuestionKind::FillInBlank => fill_in_blank::detect(dom),
            QuestionKind::ErrorAnalysis => error_analysis::detect(dom),
            QuestionKind::QuizMatching => quiz_matching::detect(dom),
            QuestionKind::VocabularyIntro => vocabulary_intro::detect(dom),
        }
    }

    pub fn parse(self, dom: &Dom) -> QuestionData {
        match self {
            QuestionKind::AudioMultipleChoice => QuestionData::AudioMultipleChoice(audio_multiple_choice::parse(dom)),
            QuestionKind::ReadingComprehension => QuestionData::ReadingComprehension(reading_comprehension::parse(dom)),
            QuestionKind::FillInBlank => QuestionData::FillInBlank(fill_in_blank::parse(dom)),
            QuestionKind::ErrorAnalysis => QuestionData::ErrorAnalysis(error_analysis::parse(dom)),
            QuestionKind::QuizMatching => QuestionData::QuizMatching(quiz_matching::parse(dom)),
            QuestionKind::VocabularyIntro => QuestionData::VocabularyIntro(vocabulary_intro::parse(dom)),
        }
    }
}

/// Port of `src/questionTypes/registry.js`. `find_handler` returns the first match, so order matters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registry<T> {
    types: Vec<T>,
}

impl<T> Default for Registry<T> {
    fn default() -> Self {
        Registry { types: Vec::new() }
    }
}

impl<T> Registry<T> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, type_module: T) {
        self.types.push(type_module);
    }

    pub fn find_handler(&self, detect: impl Fn(&T) -> bool) -> Option<&T> {
        self.types.iter().find(|t| detect(t))
    }

    pub fn list(&self) -> &[T] {
        &self.types
    }
}

impl Registry<QuestionKind> {
    pub fn find_kind(&self, dom: &Dom) -> Option<QuestionKind> {
        self.find_handler(|kind| kind.detect(dom)).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq)]
    struct FakeModule {
        name: &'static str,
        matches_kind: Option<&'static str>,
    }

    impl FakeModule {
        fn detect(&self, dom_kind: &str) -> bool {
            self.matches_kind == Some(dom_kind)
        }
    }

    #[test]
    fn find_handler_returns_the_first_module_whose_detect_matches() {
        let mut registry = Registry::new();
        registry.register(FakeModule { name: "multipleChoice", matches_kind: Some("mcq") });
        registry.register(FakeModule { name: "fillBlank", matches_kind: Some("blank") });

        assert_eq!(registry.find_handler(|m| m.detect("blank")).map(|m| m.name), Some("fillBlank"));
        assert_eq!(registry.find_handler(|m| m.detect("mcq")).map(|m| m.name), Some("multipleChoice"));
    }

    #[test]
    fn find_handler_returns_null_when_nothing_matches() {
        let mut registry = Registry::new();
        registry.register(FakeModule { name: "mcq", matches_kind: None });
        assert_eq!(registry.find_handler(|m| m.detect("unknown")), None);
    }

    #[test]
    fn list_returns_registered_modules_in_registration_order() {
        let mut registry = Registry::new();
        registry.register(FakeModule { name: "a", matches_kind: None });
        registry.register(FakeModule { name: "b", matches_kind: None });
        assert_eq!(
            registry.list(),
            &[FakeModule { name: "a", matches_kind: None }, FakeModule { name: "b", matches_kind: None }]
        );
    }
}
