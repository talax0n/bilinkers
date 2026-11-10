//! Port of `src/llm.js`: the prompts, response parsing and the Codex CLI client.
//! Only the codex provider is implemented; openai and gemini stay JS-only.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::LazyLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use regex::Regex;
use serde::Serialize;
use serde_json::Value;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::config::{Config, LlmProvider};
use crate::js;

pub const SYSTEM_PROMPT: &str = concat!(
    "You are completing an English-course exercise. Respond with strict JSON only, ",
    "matching the shape implied by the question data (e.g. { \"answer\": \"...\" } for a ",
    "single answer, or { \"answers\": [...] } for multiple blanks/pills). No prose outside the JSON."
);

static OPENING_FENCE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"(?i)^```(?:json)?{}*", js::WS_CLASS)).unwrap());
static CLOSING_FENCE: LazyLock<Regex> = LazyLock::new(|| Regex::new(&format!(r"```{}*$", js::WS_CLASS)).unwrap());

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UserPrompt<'a, D> {
    instruction: &'a str,
    question_data: &'a D,
    feedback: Option<&'a str>,
}

/// `JSON.stringify({ instruction, questionData, feedback })`.
pub fn build_user_prompt<D: Serialize>(instruction: &str, question_data: &D, feedback: Option<&str>) -> String {
    serde_json::to_string(&UserPrompt { instruction, question_data, feedback })
        .expect("question data serializes to JSON")
}

pub const SIGN_IN_MESSAGE: &str = "Codex is not signed in. Run `codex login`, then rerun the bot.";

pub const CODEX_TIMEOUT: Duration = Duration::from_secs(120);

/// A rejected sign-in fails the same way on every call, so callers abort instead of retrying.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotSignedIn;

impl std::fmt::Display for NotSignedIn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(SIGN_IN_MESSAGE)
    }
}

impl std::error::Error for NotSignedIn {}

/// Why an answer could not be produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LlmError {
    NotSignedIn,
    /// Transient or per-question: worth another try.
    Failed(String),
}

impl LlmError {
    pub fn sign_in(&self) -> Option<NotSignedIn> {
        matches!(self, LlmError::NotSignedIn).then_some(NotSignedIn)
    }
}

impl std::fmt::Display for LlmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LlmError::NotSignedIn => f.write_str(SIGN_IN_MESSAGE),
            LlmError::Failed(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for LlmError {}

impl From<String> for LlmError {
    fn from(message: String) -> Self {
        LlmError::Failed(message)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnsupportedProvider(pub LlmProvider);

impl std::fmt::Display for UnsupportedProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "LLM provider {:?} is not supported by the Rust bot, use LLM_PROVIDER=codex", self.0)
    }
}

impl std::error::Error for UnsupportedProvider {}

// Text match on the CLI's message; switch to an exit code if codex ever documents one.
static SIGN_IN_STDERR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)log ?in|logged out|unauthori[sz]ed|401").unwrap());

/// Answers through the Codex CLI, so the call rides the user's own `codex login`
/// (ChatGPT OAuth). The bot never sees or stores a token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexClient {
    pub model: Option<String>,
}

impl CodexClient {
    pub fn from_config(config: &Config) -> Result<CodexClient, UnsupportedProvider> {
        match config.llm_provider {
            LlmProvider::Codex => Ok(CodexClient { model: config.codex.model.clone() }),
            other => Err(UnsupportedProvider(other)),
        }
    }

    fn args(&self, dir: &Path, out: &Path) -> Vec<OsString> {
        let mut args: Vec<OsString> =
            ["exec", "--skip-git-repo-check", "--ephemeral", "--sandbox", "read-only", "-C"].map(OsString::from).into();
        args.push(dir.into());
        args.extend(["--color", "never", "-o"].map(OsString::from));
        args.push(out.into());
        if let Some(model) = &self.model {
            args.extend(["-m".into(), model.into()]);
        }
        args.push("-".into());
        args
    }

    /// Runs `codex exec` with the prompt on stdin and returns its last message.
    pub async fn ask(&self, prompt: &str) -> Result<String, LlmError> {
        let dir = TempDir::new("bilinkers-codex-").map_err(|e| LlmError::Failed(format!("codex exec failed: {e}")))?;
        let out = dir.0.join("answer.txt");
        let args = self.args(&dir.0, &out);
        let (program, path) = crate::setup::codex_program();
        let mut child = Command::new(program)
            .args(&args)
            .env("PATH", path)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| LlmError::Failed(format!("codex exec failed: spawn codex: {e}")))?;
        let mut stdin = child.stdin.take().expect("stdin is piped");
        let prompt = prompt.as_bytes().to_vec();
        // A child that exits without reading stdin is reported through its exit status, not here.
        let write = async move {
            let _ = stdin.write_all(&prompt).await;
        };
        let run = async { tokio::join!(write, child.wait_with_output()).1 };
        let output = match tokio::time::timeout(CODEX_TIMEOUT, run).await {
            Err(_) => {
                return Err(LlmError::Failed(format!(
                    "codex exec failed: timed out after {} s",
                    CODEX_TIMEOUT.as_secs()
                )))
            }
            Ok(Err(e)) => return Err(LlmError::Failed(format!("codex exec failed: {e}"))),
            Ok(Ok(output)) => output,
        };
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let detail = match js::trim(&stderr) {
                "" => format!(
                    "Command failed: codex {}",
                    args.iter().map(|a| a.to_string_lossy()).collect::<Vec<_>>().join(" ")
                ),
                text => text.to_string(),
            };
            if SIGN_IN_STDERR.is_match(&detail) {
                return Err(LlmError::NotSignedIn);
            }
            let last_line = detail.rsplit('\n').next().unwrap_or_default();
            return Err(LlmError::Failed(format!("codex exec failed: {last_line}")));
        }
        std::fs::read_to_string(&out)
            .map_err(|e| LlmError::Failed(format!("codex exec failed: could not read {}: {e}", out.display())))
    }

    /// The codex branch of `answerQuestion`.
    pub async fn answer_question<D: Serialize>(
        &self,
        instruction: &str,
        question_data: &D,
        feedback: Option<&str>,
    ) -> Result<Value, LlmError> {
        let prompt = format!("{SYSTEM_PROMPT}\n\n{}", build_user_prompt(instruction, question_data, feedback));
        let content = self.ask(&prompt).await?;
        Ok(parse_json_content(Some(&content), "LLM")?)
    }
}

/// `answerQuestion` as an answer-loop answerer.
pub struct LlmAnswerer<'a>(pub &'a CodexClient);

impl<D: Serialize> crate::answer_loop::Answerer<D> for LlmAnswerer<'_> {
    async fn answer(
        &mut self,
        instruction: &str,
        question_data: &D,
        feedback: Option<&str>,
    ) -> Result<Value, LlmError> {
        self.0.answer_question(instruction, question_data, feedback).await
    }
}

/// A fresh directory under the system temp dir, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(prefix: &str) -> std::io::Result<TempDir> {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or_default();
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("{prefix}{}-{nanos}-{n}", std::process::id()));
        std::fs::create_dir(&path)?;
        Ok(TempDir(path))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// No response_format is sent (one provider returns 502 on it), so models may wrap JSON in a fence.
pub fn parse_json_content(content: Option<&str>, label: &str) -> Result<Value, String> {
    let content = match content {
        Some(c) if !c.is_empty() => c,
        _ => return Err(format!("answerQuestion: {label} response had no message content")),
    };
    let unfenced = OPENING_FENCE.replace(content, "");
    let unfenced = CLOSING_FENCE.replace(&unfenced, "");
    serde_json::from_str(js::trim(&unfenced))
        .map_err(|err| format!("answerQuestion: {label} response was not valid JSON: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn answer_question_returns_parsed_json_from_the_llm_response() {
        let content = serde_json::to_string(&json!({ "answer": "went" })).unwrap();
        assert_eq!(parse_json_content(Some(&content), "LLM").unwrap(), json!({ "answer": "went" }));
    }

    #[test]
    fn answer_question_includes_feedback_in_the_prompt_when_retrying() {
        let prompt =
            build_user_prompt("Fill the blank", &json!({ "text": "She ___ home." }), Some("previous answer was wrong"));
        assert!(prompt.contains("previous answer was wrong"));
    }

    #[test]
    fn answer_question_strips_a_json_fence_before_parsing() {
        let result = parse_json_content(Some("```json\n{\"answer\":\"went\"}\n```"), "LLM").unwrap();
        assert_eq!(result, json!({ "answer": "went" }));
    }

    #[test]
    fn answer_question_throws_a_clear_error_when_llm_response_is_not_valid_json() {
        let err = parse_json_content(Some("not json"), "LLM").unwrap_err();
        assert!(err.contains("LLM response was not valid JSON"), "{err}");
    }

    #[test]
    fn answer_question_throws_a_clear_error_when_llm_response_has_no_content() {
        let err = parse_json_content(None, "LLM").unwrap_err();
        assert!(err.contains("LLM response had no message content"), "{err}");
    }

    // PATH is process-wide, so the fake-codex tests take turns.
    static PATH_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// Writes `script` as an executable `codex` and runs `f` with its directory first on PATH.
    /// `$LOG` in the script names a file in that directory, read back through the returned dir.
    async fn with_fake_codex<T>(script: &str, f: impl std::future::Future<Output = T>) -> (T, TempDir) {
        let _guard = PATH_LOCK.lock().await;
        let bin = TempDir::new("fake-codex-").unwrap();
        let codex = bin.0.join("codex");
        std::fs::write(&codex, script.replace("$LOG", &bin.0.join("calls.txt").display().to_string())).unwrap();
        let mut perms = std::fs::metadata(&codex).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
        std::fs::set_permissions(&codex, perms).unwrap();
        let saved = std::env::var_os("PATH").unwrap_or_default();
        let mut paths = vec![bin.0.clone()];
        paths.extend(std::env::split_paths(&saved));
        std::env::set_var("PATH", std::env::join_paths(paths).unwrap());
        let result = f.await;
        std::env::set_var("PATH", saved);
        (result, bin)
    }

    #[tokio::test]
    async fn answer_question_with_the_codex_client_runs_codex_exec_and_parses_its_last_message() {
        let script = "#!/bin/sh\necho \"$@\" > $LOG\ncat >> $LOG\nwhile [ \"$1\" != \"-o\" ]; do shift; done\nprintf '{\"answer\":\"had started\"}' > \"$2\"\n";
        let client = CodexClient { model: Some("gpt-5".to_string()) };
        let (result, bin) = with_fake_codex(
            script,
            client.answer_question("Fill the blank", &json!({ "text": "the film ___ (start)" }), None),
        )
        .await;
        assert_eq!(result.unwrap(), json!({ "answer": "had started" }));
        let call = std::fs::read_to_string(bin.0.join("calls.txt")).unwrap();
        let argv = Regex::new(
            r"^exec --skip-git-repo-check --ephemeral --sandbox read-only -C (\S+) --color never -o (\S+) -m gpt-5 -\n",
        )
        .unwrap();
        let caps = argv.captures(&call).unwrap_or_else(|| panic!("unexpected argv: {call}"));
        assert_eq!(Path::new(&caps[2]), Path::new(&caps[1]).join("answer.txt"));
        assert!(!Path::new(&caps[1]).exists(), "the temp dir is removed after the call");
        let stdin = &call[caps[0].len()..];
        assert_eq!(
            stdin,
            format!(
                "{SYSTEM_PROMPT}\n\n{}",
                r#"{"instruction":"Fill the blank","questionData":{"text":"the film ___ (start)"},"feedback":null}"#
            )
        );
    }

    #[tokio::test]
    async fn the_codex_client_omits_the_model_flag_without_a_model() {
        let script = "#!/bin/sh\necho \"$@\" > $LOG\nwhile [ \"$1\" != \"-o\" ]; do shift; done\nprintf '{\"answer\":\"x\"}' > \"$2\"\n";
        let client = CodexClient { model: None };
        let (result, bin) = with_fake_codex(script, client.answer_question("i", &json!({}), None)).await;
        assert_eq!(result.unwrap(), json!({ "answer": "x" }));
        let call = std::fs::read_to_string(bin.0.join("calls.txt")).unwrap();
        assert!(!call.contains(" -m ") && call.trim_end().ends_with("answer.txt -"), "{call}");
    }

    #[tokio::test]
    async fn a_logged_out_codex_is_a_sign_in_error() {
        let script = "#!/bin/sh\ncat > /dev/null\necho 'Error: Not logged in. Run codex login.' >&2\nexit 1\n";
        let client = CodexClient { model: None };
        let (result, _bin) = with_fake_codex(script, client.answer_question("i", &json!({}), None)).await;
        let err = result.unwrap_err();
        assert_eq!(err, LlmError::NotSignedIn);
        assert_eq!(err.to_string(), "Codex is not signed in. Run `codex login`, then rerun the bot.");
    }

    #[tokio::test]
    async fn other_codex_failures_report_the_last_stderr_line_and_are_not_sign_in() {
        let script = "#!/bin/sh\ncat > /dev/null\nprintf 'starting\\nstream error: model overloaded\\n' >&2\nexit 2\n";
        let client = CodexClient { model: None };
        let (result, _bin) = with_fake_codex(script, client.answer_question("i", &json!({}), None)).await;
        assert_eq!(
            result.unwrap_err(),
            LlmError::Failed("codex exec failed: stream error: model overloaded".to_string())
        );
    }

    /// Needs a signed-in `codex` on PATH: `cargo test -- --ignored real_codex`.
    #[tokio::test]
    #[ignore]
    async fn real_codex_answers_a_fill_in_blank() {
        let client = CodexClient { model: None };
        let answer = client
            .answer_question(
                "Fill the blank with the past perfect of the verb in brackets. Return {\"answer\": \"...\"}.",
                &json!({ "text": "By the time we arrived, the film ___ (start)." }),
                None,
            )
            .await
            .unwrap();
        println!("real codex answer: {answer}");
        assert!(answer["answer"].is_string(), "{answer}");
    }

    #[test]
    fn only_the_codex_provider_builds_a_client() {
        let mut config = crate::config::build_config(&Default::default());
        assert_eq!(CodexClient::from_config(&config), Ok(CodexClient { model: None }));
        config.llm_provider = LlmProvider::OpenAi;
        assert_eq!(CodexClient::from_config(&config), Err(UnsupportedProvider(LlmProvider::OpenAi)));
    }
}
