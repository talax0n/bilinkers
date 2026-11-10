//! Port of `src/config.js`. Reading `.env` and the process environment is left to the caller.

use std::collections::HashMap;

use crate::js;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmProvider {
    Codex,
    OpenAi,
    Gemini,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexConfig {
    /// `CODEX_MODEL`; unset or empty leaves the choice to the Codex CLI.
    pub model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenAiConfig {
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeminiConfig {
    pub api_key: Option<String>,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Timeouts {
    pub login_poll_ms: u64,
    pub login_timeout_ms: u64,
    pub element_wait_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Retry {
    pub max_answer_retries: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub unhandled_log_dir: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub llm_provider: LlmProvider,
    pub codex: CodexConfig,
    pub openai: OpenAiConfig,
    pub gemini: GeminiConfig,
    pub timeouts: Timeouts,
    pub retry: Retry,
    pub paths: Paths,
}

const MAX_SAFE_INTEGER: u64 = (1 << 53) - 1;

pub fn parse_non_negative_integer(value: Option<&str>, fallback: u64) -> u64 {
    let Some(value) = value else {
        return fallback;
    };
    let text = js::trim(value);
    if !js::is_ascii_digits(text) {
        return fallback;
    }
    match text.parse::<u64>() {
        Ok(parsed) if parsed <= MAX_SAFE_INTEGER => parsed,
        _ => fallback,
    }
}

/// `env.KEY || default`: a missing or empty value takes the default.
fn or_default(env: &HashMap<String, String>, key: &str, default: &str) -> String {
    match env.get(key) {
        Some(value) if !value.is_empty() => value.clone(),
        _ => default.to_string(),
    }
}

pub fn build_config(env: &HashMap<String, String>) -> Config {
    // createClient matches "codex" and "gemini" exactly; anything else is OpenAI.
    let llm_provider = match or_default(env, "LLM_PROVIDER", "codex").as_str() {
        "codex" => LlmProvider::Codex,
        "gemini" => LlmProvider::Gemini,
        _ => LlmProvider::OpenAi,
    };
    let codex_model = or_default(env, "CODEX_MODEL", "");
    Config {
        llm_provider,
        codex: CodexConfig { model: (!codex_model.is_empty()).then_some(codex_model) },
        openai: OpenAiConfig {
            base_url: env.get("OPENAI_BASE_URL").cloned(),
            api_key: env.get("OPENAI_API_KEY").cloned(),
            model: or_default(env, "OPENAI_MODEL", "gpt-4o"),
        },
        gemini: GeminiConfig {
            api_key: env.get("GEMINI_API_KEY").cloned(),
            model: or_default(env, "GEMINI_MODEL", "gemini-1.5-flash"),
        },
        timeouts: Timeouts { login_poll_ms: 2000, login_timeout_ms: 5 * 60 * 1000, element_wait_ms: 10000 },
        retry: Retry {
            max_answer_retries: parse_non_negative_integer(env.get("MAX_ANSWER_RETRIES").map(String::as_str), 25),
        },
        paths: Paths { unhandled_log_dir: "./logs/unhandled".to_string() },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn build_config_defaults_model_when_not_set() {
        let cfg = build_config(&env(&[("OPENAI_BASE_URL", "https://x"), ("OPENAI_API_KEY", "k")]));
        assert_eq!(cfg.openai.model, "gpt-4o");
        assert_eq!(cfg.openai.base_url.as_deref(), Some("https://x"));
        assert_eq!(cfg.openai.api_key.as_deref(), Some("k"));
    }

    #[test]
    fn build_config_defaults_to_the_codex_provider_with_no_model() {
        let cfg = build_config(&env(&[]));
        assert_eq!(cfg.llm_provider, LlmProvider::Codex);
        assert_eq!(cfg.codex.model, None);
        assert_eq!(build_config(&env(&[("CODEX_MODEL", "gpt-5")])).codex.model.as_deref(), Some("gpt-5"));
        assert_eq!(build_config(&env(&[("LLM_PROVIDER", "gemini")])).llm_provider, LlmProvider::Gemini);
        assert_eq!(build_config(&env(&[("LLM_PROVIDER", "other")])).llm_provider, LlmProvider::OpenAi);
    }

    #[test]
    fn build_config_uses_provided_model() {
        let cfg = build_config(&env(&[("OPENAI_MODEL", "custom-model")]));
        assert_eq!(cfg.openai.model, "custom-model");
    }

    #[test]
    fn build_config_sets_retry_and_path_defaults() {
        let cfg = build_config(&env(&[]));
        assert_eq!(cfg.retry.max_answer_retries, 25);
        assert_eq!(cfg.paths.unhandled_log_dir, "./logs/unhandled");
    }

    #[test]
    fn build_config_uses_max_answer_retries_env_override() {
        let cfg = build_config(&env(&[("MAX_ANSWER_RETRIES", "7")]));
        assert_eq!(cfg.retry.max_answer_retries, 7);
    }

    #[test]
    fn build_config_falls_back_for_invalid_max_answer_retries() {
        assert_eq!(build_config(&env(&[("MAX_ANSWER_RETRIES", "-1")])).retry.max_answer_retries, 25);
        assert_eq!(build_config(&env(&[("MAX_ANSWER_RETRIES", "1.5")])).retry.max_answer_retries, 25);
        assert_eq!(build_config(&env(&[("MAX_ANSWER_RETRIES", "nope")])).retry.max_answer_retries, 25);
    }
}
