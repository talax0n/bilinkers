//! Script evaluation inside the activity iframe, which Selenium's `switchTo().frame()` hid.
//!
//! The LTI player is cross-site, so with site isolation it is an out-of-process iframe: its own
//! CDP target, unreachable from the tab's session. Its target id is the iframe element's frame id.
//! A same-origin iframe stays in the tab's process and is reached through `contentWindow`.

use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use chromiumoxide::cdp::browser_protocol::target::{
    AttachToTargetParams, DetachFromTargetParams, EventReceivedMessageFromTarget, GetTargetsParams, SessionId,
    TargetId,
};
use chromiumoxide::cdp::js_protocol::runtime::EvaluateParams;
use chromiumoxide::listeners::EventStream;
use chromiumoxide::{Browser, Command, Method, Page};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::browser::{PageError, PageResult};

/// The page's script, wrapped so `return` and `arguments[i]` work as in `executeScript`.
pub fn wrap_script(body: &str, args: &[Value]) -> String {
    format!("(function() {{\n{body}\n}}).apply(null, {})", Value::from(args.to_vec()))
}

/// Same-origin frames: build the function in the frame's own realm, so `document` is the frame's.
const SAME_ORIGIN_EVAL: &str = r#"
  const frame = document.querySelector('iframe');
  if (!frame) throw new Error('no iframe');
  return frame.contentWindow.Function(arguments[0]).apply(null, arguments[1]);
"#;

const FIRST_IFRAME_ORIGIN: &str = r#"
  const frame = document.querySelector('iframe');
  if (!frame) return 'none';
  try { if (frame.contentDocument) return 'same'; } catch (e) {}
  return 'cross';
"#;

/// `Target.sendMessageToTarget`, absent from the generated CDP types because it is deprecated.
/// Non-flattened sessions still accept it, and chromiumoxide can only send on its own sessions.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SendMessageToTarget {
    session_id: SessionId,
    message: String,
}

#[derive(Debug, Clone, Deserialize)]
struct Empty {}

impl Method for SendMessageToTarget {
    fn identifier(&self) -> chromiumoxide::types::MethodId {
        "Target.sendMessageToTarget".into()
    }
}

impl Command for SendMessageToTarget {
    type Response = Empty;
}

struct Session {
    target: TargetId,
    id: SessionId,
    messages: EventStream<EventReceivedMessageFromTarget>,
}

/// One attached session to the iframe target, re-attached when the iframe is replaced.
pub struct FrameEvaluator {
    session: Mutex<Option<Session>>,
    next_id: AtomicI64,
}

const REPLY_TIMEOUT: Duration = Duration::from_secs(30);

impl Default for FrameEvaluator {
    fn default() -> Self {
        FrameEvaluator { session: Mutex::new(None), next_id: AtomicI64::new(1) }
    }
}

impl FrameEvaluator {
    /// Runs `body` in the first iframe's document of `page`.
    pub async fn execute(&self, browser: &Browser, page: &Page, body: &str, args: &[Value]) -> PageResult<Value> {
        match evaluate_top(page, FIRST_IFRAME_ORIGIN, &[]).await?.as_str() {
            Some("same") => evaluate_top(page, SAME_ORIGIN_EVAL, &[json!(body), Value::from(args.to_vec())]).await,
            Some("cross") => self.execute_cross(browser, page, &wrap_script(body, args)).await,
            _ => Err(PageError("no iframe".to_string())),
        }
    }

    async fn execute_cross(&self, browser: &Browser, page: &Page, expression: &str) -> PageResult<Value> {
        let target = frame_target(browser, page).await?;
        let mut guard = self.session.lock().await;
        if guard.as_ref().is_none_or(|s| s.target != target) {
            if let Some(old) = guard.take() {
                let _ = browser.execute(DetachFromTargetParams::builder().session_id(old.id).build()).await;
            }
            *guard = Some(attach(browser, target.clone()).await?);
        }
        let session = guard.as_mut().expect("attached above");
        match self.evaluate(browser, session, expression).await {
            Ok(value) => Ok(value),
            // A cross-process navigation inside the iframe ends the session; attach once more.
            Err(_) => {
                *guard = Some(attach(browser, target).await?);
                let session = guard.as_mut().expect("attached above");
                self.evaluate(browser, session, expression).await
            }
        }
    }

    async fn evaluate(&self, browser: &Browser, session: &mut Session, expression: &str) -> PageResult<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let message = json!({
            "id": id,
            "method": "Runtime.evaluate",
            "params": { "expression": expression, "returnByValue": true, "awaitPromise": false },
        });
        browser.execute(SendMessageToTarget { session_id: session.id.clone(), message: message.to_string() }).await?;
        let reply = tokio::time::timeout(REPLY_TIMEOUT, async {
            while let Some(event) = session.messages.next().await {
                if event.session_id != session.id {
                    continue;
                }
                let Ok(reply) = serde_json::from_str::<Value>(&event.message) else { continue };
                if reply["id"] == json!(id) {
                    return Some(reply);
                }
            }
            None
        })
        .await
        .ok()
        .flatten()
        .ok_or_else(|| PageError("iframe did not answer the evaluate".to_string()))?;
        evaluation_value(&reply)
    }
}

/// The first iframe element's frame id, which is also its out-of-process target id.
async fn frame_target(browser: &Browser, page: &Page) -> PageResult<TargetId> {
    let node = page.find_element("iframe").await?.description().await?;
    let frame_id = node.frame_id.ok_or_else(|| PageError("iframe has no frame id".to_string()))?;
    let target = TargetId::new(frame_id.inner().clone());
    let targets = browser.execute(GetTargetsParams::default()).await?.result.target_infos;
    if targets.iter().any(|t| t.target_id == target) {
        Ok(target)
    } else {
        Err(PageError("the cross-origin iframe has no debugger target yet".to_string()))
    }
}

async fn attach(browser: &Browser, target: TargetId) -> PageResult<Session> {
    // Subscribe first so no reply can arrive before the stream exists.
    let messages = browser.event_listener::<EventReceivedMessageFromTarget>().await?;
    let params = AttachToTargetParams::builder().target_id(target.clone()).flatten(false).build().map_err(PageError)?;
    let id = browser.execute(params).await?.result.session_id;
    Ok(Session { target, id, messages })
}

/// A `Runtime.evaluate` reply as JSON: the by-value result, or the page exception as an error.
pub fn evaluation_value(reply: &Value) -> PageResult<Value> {
    if let Some(error) = reply.get("error") {
        return Err(PageError(format!("iframe evaluate failed: {error}")));
    }
    let result = &reply["result"];
    if let Some(details) = result.get("exceptionDetails") {
        let text = details["exception"]["description"].as_str().or(details["text"].as_str()).unwrap_or("exception");
        return Err(PageError(text.to_string()));
    }
    Ok(result["result"].get("value").cloned().unwrap_or(Value::Null))
}

/// Evaluates a wrapped script in the tab's top document.
pub async fn evaluate_top(page: &Page, body: &str, args: &[Value]) -> PageResult<Value> {
    let params = EvaluateParams::builder()
        .expression(wrap_script(body, args))
        .return_by_value(true)
        .await_promise(false)
        .build()
        .map_err(PageError)?;
    let result = page.evaluate_expression(params).await?;
    Ok(result.value().cloned().unwrap_or(Value::Null))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evaluation_value_reads_the_by_value_result_and_surfaces_exceptions() {
        let ok = json!({ "id": 1, "result": { "result": { "type": "string", "value": "hi" } } });
        assert_eq!(evaluation_value(&ok), Ok(json!("hi")));
        let undefined = json!({ "id": 2, "result": { "result": { "type": "undefined" } } });
        assert_eq!(evaluation_value(&undefined), Ok(Value::Null));
        let thrown = json!({ "id": 3, "result": {
            "result": { "type": "object" },
            "exceptionDetails": { "text": "Uncaught", "exception": { "description": "TypeError: x is null" } },
        } });
        assert_eq!(evaluation_value(&thrown), Err(PageError("TypeError: x is null".to_string())));
    }
}
