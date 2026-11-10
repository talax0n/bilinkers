//! Detect and parse from `src/questionTypes/audioMultipleChoice.js`.

use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;

use super::option_buttons::{first_rtf_block, parse_options, strip_tags, OptionButton};
use super::Dom;

static STRONG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<strong>([\s\S]*?)</strong>").unwrap());
static PARAGRAPH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<p>([\s\S]*?)</p>").unwrap());

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioMultipleChoiceData {
    pub instruction: String,
    pub question_text: String,
    pub options: Vec<OptionButton>,
}

/// Hidden sliders from earlier listening questions stay in outerHTML, hence `has_visible_slider`.
pub fn has_slider(dom: &Dom) -> bool {
    dom.has_visible_slider.unwrap_or_else(|| dom.outer_html.contains("MuiSlider-root"))
}

pub fn detect(dom: &Dom) -> bool {
    has_slider(dom) && dom.outer_html.contains("bl-w-full justify-content-start")
}

pub fn parse(dom: &Dom) -> AudioMultipleChoiceData {
    let html = &dom.outer_html;
    let instruction_block = first_rtf_block(html);

    let instruction_match = STRONG.captures(instruction_block);
    let instruction = instruction_match.as_ref().map_or_else(String::new, |c| strip_tags(&c[1]));

    let after_strong = match &instruction_match {
        Some(c) => &instruction_block[c.get(0).unwrap().end()..],
        None => instruction_block,
    };
    let question_text = match PARAGRAPH.captures(after_strong) {
        Some(c) => strip_tags(&c[1]),
        None => strip_tags(instruction_block),
    };

    AudioMultipleChoiceData { instruction, question_text, options: parse_options(html) }
}
