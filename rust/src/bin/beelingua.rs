use std::time::Duration;

use beelingua::browser::{attach_to_lms, wait_for_url_change};

const USAGE: &str = "usage: beelingua <attach|check-url-change> (CDP_PORT, default 9222)";

#[tokio::main]
async fn main() {
    let port = std::env::var("CDP_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(9222);
    let mode = std::env::args().nth(1).unwrap_or_default();

    let lms = match attach_to_lms(port).await {
        Ok(lms) => lms,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    let url = lms.page.url().await.ok().flatten().unwrap_or_default();
    match mode.as_str() {
        "attach" => println!("attached to {url}"),
        "check-url-change" => {
            println!("attached to {url}");
            lms.page.evaluate("location.hash = 'moved'").await.expect("evaluate");
            println!("urlChange -> {:?}", wait_for_url_change(&lms.page, &url, Duration::from_secs(3)).await);
        }
        _ => {
            eprintln!("{USAGE}");
            std::process::exit(2);
        }
    }
}
