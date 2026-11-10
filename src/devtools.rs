//! The browser's DevTools HTTP endpoint on the debug port. Listing tabs here opens no CDP
//! session and never focuses a tab.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use serde_json::Value;

use crate::browser::LMS_HOST;

/// One GET on the debug port: the status line's code and the body, None when nothing answers.
pub fn http_get(port: u16, path: &str) -> Option<(u16, Vec<u8>)> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_millis(500)).ok()?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let request = format!("GET {path} HTTP/1.1\r\nHost: localhost:{port}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).ok()?;
    let mut data = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let n = stream.read(&mut chunk).unwrap_or(0);
        if n == 0 {
            break;
        }
        data.extend_from_slice(&chunk[..n]);
        // The DevTools server may keep the connection open, so stop once Content-Length is in.
        if let Some((head, body)) = split_response(&data) {
            if content_length(head).is_some_and(|len| body.len() >= len) {
                break;
            }
        }
    }
    let (head, body) = split_response(&data)?;
    let status = std::str::from_utf8(head).ok()?.split_whitespace().nth(1)?.parse().ok()?;
    let body = match content_length(head) {
        Some(len) => body[..len.min(body.len())].to_vec(),
        None => body.to_vec(),
    };
    Some((status, body))
}

fn split_response(data: &[u8]) -> Option<(&[u8], &[u8])> {
    let end = data.windows(4).position(|w| w == b"\r\n\r\n")?;
    Some((&data[..end], &data[end + 4..]))
}

fn content_length(head: &[u8]) -> Option<usize> {
    std::str::from_utf8(head).ok()?.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.trim().eq_ignore_ascii_case("content-length").then(|| value.trim().parse().ok())?
    })
}

/// A page target as `/json/list` shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedTab {
    /// The CDP target id.
    pub id: String,
    pub title: String,
    pub url: String,
}

impl ListedTab {
    pub fn is_lms(&self) -> bool {
        self.url.contains(LMS_HOST)
    }
}

/// The page targets in listing order, which is most recently activated first. Workers,
/// iframes and extension pages are left out.
pub fn page_tabs_from(targets: &Value) -> Vec<ListedTab> {
    let text = |t: &Value, key: &str| t[key].as_str().unwrap_or_default().to_string();
    targets
        .as_array()
        .into_iter()
        .flatten()
        .filter(|t| t["type"] == "page")
        .map(|t| ListedTab { id: text(t, "id"), title: text(t, "title"), url: text(t, "url") })
        .collect()
}

/// The browser's page targets, None when nothing answers on the port.
pub fn page_tabs(port: u16) -> Option<Vec<ListedTab>> {
    let (_, body) = http_get(port, "/json/list")?;
    Some(page_tabs_from(&serde_json::from_slice(&body).unwrap_or(Value::Null)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_response_is_cut_at_its_content_length() {
        let data = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\ncontent-length: 2\r\n\r\n[]extra";
        let (head, body) = split_response(data).unwrap();
        assert_eq!(content_length(head), Some(2));
        assert_eq!(&body[..2], b"[]");
        assert_eq!(split_response(b"HTTP/1.1 200 OK\r\n"), None);
    }

    #[test]
    fn only_page_targets_are_listed_in_order() {
        let targets = json!([
            { "type": "service_worker", "id": "sw", "title": "sw", "url": "https://lms.binus.ac.id/sw.js" },
            { "type": "page", "id": "A", "title": "New Tab", "url": "chrome://newtab/" },
            { "type": "page", "id": "B", "title": "Unit 5", "url": "https://lms.binus.ac.id/course/5" },
            { "type": "iframe", "id": "C", "title": "", "url": "https://lms.binus.ac.id/frame" },
        ]);
        let tabs = page_tabs_from(&targets);
        assert_eq!(tabs.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["A", "B"]);
        assert!(!tabs[0].is_lms() && tabs[1].is_lms());
        assert_eq!(page_tabs_from(&Value::Null), vec![], "an unreadable list has no tabs");
    }
}
