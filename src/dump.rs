//! `bilinkers dump`: saves what a question page is made of, so an unknown layout can be studied
//! offline. Read-only on the page. It never clicks.

use std::collections::HashMap;
use std::time::Duration;

use chromiumoxide::cdp::browser_protocol::target::DetachFromTargetParams;
use chromiumoxide::{Browser, Page};
use futures::StreamExt;
use serde::Serialize;
use serde_json::{json, Value};
use tokio::time::Instant;

use crate::browser::{FramedPage, PageResult, IFRAME_COUNT, OUTER_HTML};
use crate::frame::{attach, SendMessageToTarget};

/// The loaded resources the page already fetched, so API endpoints show up even when the
/// network capture starts too late to see their bodies.
const RESOURCES: &str = r#"
    return performance.getEntriesByType('resource').map((e) => ({ url: e.name, initiator: e.initiatorType }));
  "#;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FrameHtml {
    pub index: usize,
    pub html: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Snapshot {
    pub url: String,
    pub top_html: String,
    pub frames: Vec<FrameHtml>,
    pub resources: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CapturedResponse {
    pub url: String,
    pub status: i64,
    pub mime: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Dump {
    pub captured_at_unix: u64,
    pub before: Snapshot,
    /// Taken when the recording window ends, so a Check or Skip clicked meanwhile shows its feedback DOM.
    pub after: Option<Snapshot>,
    pub responses: Vec<CapturedResponse>,
}

/// Every document the page shows. A frame between loads is recorded with its error instead of failing the dump.
pub async fn snapshot<P: FramedPage>(page: &P) -> PageResult<Snapshot> {
    let url = page.current_url().await?;
    let top_html = page.execute_script(OUTER_HTML, &[]).await?.as_str().unwrap_or_default().to_string();
    let count = page.execute_script(IFRAME_COUNT, &[]).await?.as_u64().unwrap_or(0) as usize;
    let mut frames = Vec::with_capacity(count);
    for index in 0..count {
        frames.push(match page.execute_in_frame_at(index, OUTER_HTML, &[]).await {
            Ok(html) => FrameHtml { index, html: html.as_str().map(str::to_string), error: None },
            Err(e) => FrameHtml { index, html: None, error: Some(e.to_string()) },
        });
    }
    let resources = page.execute_script(RESOURCES, &[]).await.unwrap_or(json!([]));
    Ok(Snapshot { url, top_html, frames, resources })
}

/// Only XHR and fetch replies carry question data. Scripts, styles, images and CORS preflights are noise.
pub fn is_data_response(kind: &str, mime: &str) -> bool {
    matches!(kind, "XHR" | "Fetch") && (mime.contains("json") || mime.starts_with("text/"))
}

#[derive(Debug, Clone, PartialEq)]
struct Meta {
    url: String,
    status: i64,
    mime: String,
}

/// Turns raw Network messages into captured bodies. Pure, so it is tested on the exact JSON
/// Chrome sends. Raw JSON is read on purpose: chromiumoxide 0.9.1 drops a whole
/// `Network.responseReceived` it cannot type, which hid the Beelingua question API on Chrome 157.
#[derive(Debug, Default)]
pub struct Recorder {
    pending: HashMap<String, Meta>,
    asked: HashMap<i64, Meta>,
    next_id: i64,
    pub captured: Vec<CapturedResponse>,
}

impl Recorder {
    /// Feeds one session message. Returns the `Network.getResponseBody` command to send when a
    /// data reply has finished loading.
    pub fn on_message(&mut self, msg: &Value) -> Option<Value> {
        let params = &msg["params"];
        match msg["method"].as_str() {
            Some("Network.responseReceived") => {
                let response = &params["response"];
                let mime = response["mimeType"].as_str().unwrap_or_default();
                if is_data_response(params["type"].as_str().unwrap_or_default(), mime) {
                    let meta = Meta {
                        url: response["url"].as_str().unwrap_or_default().to_string(),
                        status: response["status"].as_i64().unwrap_or_default(),
                        mime: mime.to_string(),
                    };
                    self.pending.insert(params["requestId"].as_str().unwrap_or_default().to_string(), meta);
                }
                None
            }
            Some("Network.loadingFinished") => {
                let request_id = params["requestId"].as_str().unwrap_or_default();
                let meta = self.pending.remove(request_id)?;
                self.next_id += 1;
                let id = BODY_IDS + self.next_id;
                self.asked.insert(id, meta);
                Some(json!({ "id": id, "method": "Network.getResponseBody", "params": { "requestId": request_id } }))
            }
            _ => {
                let meta = self.asked.remove(&msg["id"].as_i64()?)?;
                let body = match msg["result"]["body"].as_str() {
                    Some(body) => body.to_string(),
                    None => format!("<body unavailable: {}>", msg["error"]),
                };
                self.captured.push(CapturedResponse { url: meta.url, status: meta.status, mime: meta.mime, body });
                None
            }
        }
    }

    pub fn waiting_for_bodies(&self) -> bool {
        !self.asked.is_empty()
    }
}

/// Body requests are numbered from here, clear of the `Network.enable` call.
const BODY_IDS: i64 = 1000;

/// What a recording does besides listen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recording<'a> {
    /// Listens for the whole window while the user clicks.
    Listen,
    /// Reloads the tab once listening, and ends early once a reply whose URL contains this arrives.
    ReloadUntil(&'a str),
}

/// Records the tab's XHR and fetch bodies for `window`, over a raw session to the tab's own
/// target. An out-of-process iframe's requests belong to another target and are not seen.
pub async fn record_responses(
    browser: &Browser,
    page: &Page,
    window: Duration,
    recording: Recording<'_>,
) -> PageResult<Vec<CapturedResponse>> {
    let mut session = attach(browser, page.target_id().clone()).await?;
    let send = |message: Value| SendMessageToTarget { session_id: session.id.clone(), message: message.to_string() };
    browser.execute(send(json!({ "id": 1, "method": "Network.enable" }))).await?;
    let stop_at = match recording {
        Recording::Listen => None,
        Recording::ReloadUntil(url_part) => {
            browser.execute(send(json!({ "id": 2, "method": "Page.reload" }))).await?;
            Some(url_part)
        }
    };

    let mut recorder = Recorder::default();
    let deadline = Instant::now() + window;
    loop {
        // Bodies asked for just before the window closed still get a few seconds to arrive.
        let until = if Instant::now() < deadline { deadline } else { Instant::now() + Duration::from_secs(3) };
        if Instant::now() >= deadline && !recorder.waiting_for_bodies() {
            break;
        }
        let Ok(Some(event)) = tokio::time::timeout_at(until, session.messages.next()).await else { break };
        if event.session_id != session.id {
            continue;
        }
        let Ok(message) = serde_json::from_str::<Value>(&event.message) else { continue };
        if let Some(command) = recorder.on_message(&message) {
            browser.execute(send(command)).await?;
        }
        if stop_at.is_some_and(|part| recorder.captured.iter().any(|r| r.url.contains(part))) {
            break;
        }
    }
    let _ = browser.execute(DetachFromTargetParams::builder().session_id(session.id.clone()).build()).await;
    Ok(recorder.captured)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::fake::FakePage;
    use crate::browser::{PageError, Scope};

    #[test]
    fn captures_an_xhr_body_and_skips_its_preflight() {
        let mut rec = Recorder::default();
        let api = "https://apim-bl-prod.azure-api.net/assessment-fe/Assessment/x";
        let preflight = json!({ "method": "Network.responseReceived", "params": {
            "requestId": "P1", "type": "Preflight", "response": { "url": api, "status": 200, "mimeType": "" } } });
        let xhr = json!({ "method": "Network.responseReceived", "params": {
            "requestId": "10699.1023", "type": "XHR",
            "response": { "url": api, "status": 200, "mimeType": "application/json", "fieldChromiumoxideLacks": 1 } } });
        assert_eq!(rec.on_message(&preflight), None);
        assert_eq!(rec.on_message(&xhr), None);
        assert_eq!(
            rec.on_message(&json!({ "method": "Network.loadingFinished", "params": { "requestId": "P1" } })),
            None
        );

        let ask =
            rec.on_message(&json!({ "method": "Network.loadingFinished", "params": { "requestId": "10699.1023" } }));
        assert_eq!(
            ask,
            Some(json!({ "id": 1001, "method": "Network.getResponseBody", "params": { "requestId": "10699.1023" } }))
        );
        assert!(rec.waiting_for_bodies());
        rec.on_message(&json!({ "id": 1001, "result": { "body": "{\"questions\":[]}", "base64Encoded": false } }));
        assert!(!rec.waiting_for_bodies());
        assert_eq!(
            rec.captured,
            vec![CapturedResponse {
                url: api.to_string(),
                status: 200,
                mime: "application/json".to_string(),
                body: "{\"questions\":[]}".to_string(),
            }]
        );
    }

    #[test]
    fn ignores_scripts_and_images() {
        assert!(!is_data_response("Script", "application/json"));
        assert!(!is_data_response("Fetch", "image/png"));
        assert!(is_data_response("Fetch", "text/plain"));
    }

    #[tokio::test]
    async fn snapshot_keeps_every_frame_and_records_a_frame_that_cannot_be_read() {
        let page = FakePage::new(|scope: Scope, code: &str, _: &[Value]| match (scope, code) {
            (Scope::Top, OUTER_HTML) => Ok(json!("<html>top</html>")),
            (Scope::Top, IFRAME_COUNT) => Ok(json!(1)),
            (Scope::Top, RESOURCES) => Ok(json!([{ "url": "https://api/x", "initiator": "fetch" }])),
            (Scope::Frame, OUTER_HTML) => Err(PageError("frame between loads".into())),
            other => panic!("unexpected script {other:?}"),
        });
        let snap = snapshot(&page).await.unwrap();
        assert_eq!(snap.top_html, "<html>top</html>");
        assert_eq!(snap.frames, vec![FrameHtml { index: 0, html: None, error: Some("frame between loads".into()) }]);
        assert_eq!(snap.resources, json!([{ "url": "https://api/x", "initiator": "fetch" }]));
    }
}
