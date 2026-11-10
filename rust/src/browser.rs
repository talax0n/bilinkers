use std::time::{Duration, Instant};

use chromiumoxide::cdp::js_protocol::runtime::EvaluateParams;
use chromiumoxide::{Browser, Page};
use futures::StreamExt;
use serde_json::Value;
use tokio::task::JoinHandle;

pub const LMS_HOST: &str = "lms.binus.ac.id";

/// A CDP connection to the user's browser, focused on its LMS tab.
pub struct Lms {
    pub browser: Browser,
    pub page: Page,
    _events: JoinHandle<()>,
}

#[derive(Debug)]
pub enum AttachError {
    Connect(String),
    NoLmsTab(Vec<String>),
}

impl std::fmt::Display for AttachError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AttachError::Connect(e) => {
                write!(f, "Could not reach the browser's debug port ({e}). Is it running with --remote-debugging-port?")
            }
            AttachError::NoLmsTab(urls) => {
                write!(f, "No {LMS_HOST} tab found in the attached browser. Open tabs: {}", urls.join(", "))
            }
        }
    }
}

impl std::error::Error for AttachError {}

/// Attaches to the browser started with --remote-debugging-port and picks the first LMS tab,
/// like src/browser.js attachToLms. CDP directly, so no chromedriver is involved.
pub async fn attach_to_lms(port: u16) -> Result<Lms, AttachError> {
    let (mut browser, mut handler) =
        Browser::connect(format!("http://localhost:{port}")).await.map_err(|e| AttachError::Connect(e.to_string()))?;
    let events = tokio::spawn(async move { while handler.next().await.is_some() {} });

    // Tabs that existed before we connected are only known after a target fetch.
    browser.fetch_targets().await.map_err(|e| AttachError::Connect(e.to_string()))?;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let pages = browser.pages().await.map_err(|e| AttachError::Connect(e.to_string()))?;

    let mut urls = Vec::new();
    for page in pages {
        let url = page.url().await.ok().flatten().unwrap_or_default();
        if url.contains(LMS_HOST) {
            return Ok(Lms { browser, page, _events: events });
        }
        urls.push(url);
    }
    Err(AttachError::NoLmsTab(urls))
}

/// A browser call that failed: a CDP error, a page script exception, or a runner giving up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageError(pub String);

impl std::fmt::Display for PageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for PageError {}

impl From<chromiumoxide::error::CdpError> for PageError {
    fn from(err: chromiumoxide::error::CdpError) -> Self {
        PageError(err.to_string())
    }
}

pub type PageResult<T> = Result<T, PageError>;

/// The WebDriver calls the JS runners use, on the attached tab's top-level document.
/// Runners are generic over it so tests can swap in a fake page.
#[allow(async_fn_in_trait)]
pub trait Driver {
    async fn current_url(&self) -> PageResult<String>;

    /// `driver.executeScript(body, ...args)`: `body` may `return` and reads `arguments[i]`.
    /// The result comes back as JSON; `undefined` is null.
    async fn execute_script(&self, body: &str, args: &[Value]) -> PageResult<Value>;

    /// `driver.get(url)`: navigates and waits for the load.
    async fn get(&self, url: &str) -> PageResult<()>;

    async fn sleep(&self, ms: u64);

    /// Polls until the URL differs from `previous`, like src/browser.js waitForUrlChange.
    async fn wait_for_url_change(&self, previous: &str, timeout: Duration) -> Option<String> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if let Ok(url) = self.current_url().await {
                if url != previous {
                    return Some(url);
                }
            }
            self.sleep(200).await;
        }
        None
    }
}

impl Driver for Page {
    async fn current_url(&self) -> PageResult<String> {
        // location.href, as getCurrentUrl reports it, so SPA route changes show up at once.
        match self.execute_script("return location.href;", &[]).await {
            Ok(Value::String(url)) => Ok(url),
            _ => Ok(self.url().await?.unwrap_or_default()),
        }
    }

    async fn execute_script(&self, body: &str, args: &[Value]) -> PageResult<Value> {
        let expression = format!("(function() {{\n{body}\n}}).apply(null, {})", Value::from(args.to_vec()));
        let params = EvaluateParams::builder()
            .expression(expression)
            .return_by_value(true)
            .await_promise(false)
            .build()
            .map_err(PageError)?;
        let result = self.evaluate_expression(params).await?;
        Ok(result.value().cloned().unwrap_or(Value::Null))
    }

    async fn get(&self, url: &str) -> PageResult<()> {
        self.goto(url).await?;
        Ok(())
    }

    async fn sleep(&self, ms: u64) {
        tokio::time::sleep(Duration::from_millis(ms)).await;
    }
}
