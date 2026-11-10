use std::time::{Duration, Instant};

use chromiumoxide::{Browser, Page};
use futures::StreamExt;
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
            AttachError::Connect(e) => write!(
                f,
                "Could not reach the browser's debug port ({e}). Is it running with --remote-debugging-port?"
            ),
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
    let (mut browser, mut handler) = Browser::connect(format!("http://localhost:{port}"))
        .await
        .map_err(|e| AttachError::Connect(e.to_string()))?;
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

/// Polls until the page URL differs from `previous`, like src/browser.js waitForUrlChange.
pub async fn wait_for_url_change(page: &Page, previous: &str, timeout: Duration) -> Option<String> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Ok(Some(url)) = page.url().await {
            if url != previous {
                return Some(url);
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    None
}
