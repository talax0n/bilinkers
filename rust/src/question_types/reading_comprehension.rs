//! Detect and parse from `src/questionTypes/readingComprehension.js`.

use serde::Serialize;

use super::audio_multiple_choice::has_slider;
use super::option_buttons::{first_rtf_block, parse_options, strip_tags, OptionButton};
use super::Dom;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReadingComprehensionData {
    pub text: String,
    pub options: Vec<OptionButton>,
}

pub fn detect(dom: &Dom) -> bool {
    !has_slider(dom) && dom.outer_html.contains("bl-w-full justify-content-start")
}

pub fn parse(dom: &Dom) -> ReadingComprehensionData {
    let html = &dom.outer_html;
    ReadingComprehensionData { text: strip_tags(first_rtf_block(html)), options: parse_options(html) }
}
