use std::time::Duration;

use beelingua::browser::{attach_to_lms, Driver};
use beelingua::logger;
use beelingua::runners::course::{self, CourseOptions};
use beelingua::runners::{unit, Ported, Report};
use serde_json::json;

const USAGE: &str = "usage: beelingua <course|unit|attach|check-url-change> (CDP_PORT, default 9222)";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Course,
    Unit,
    Attach,
    CheckUrlChange,
}

impl Mode {
    fn parse(arg: &str) -> Option<Mode> {
        match arg {
            "course" => Some(Mode::Course),
            "unit" => Some(Mode::Unit),
            "attach" => Some(Mode::Attach),
            "check-url-change" => Some(Mode::CheckUrlChange),
            _ => None,
        }
    }
}

#[tokio::main]
async fn main() {
    let Some(mode) = std::env::args().nth(1).as_deref().and_then(Mode::parse) else {
        eprintln!("{USAGE}");
        std::process::exit(2);
    };
    let port = std::env::var("CDP_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(9222);

    let lms = match attach_to_lms(port).await {
        Ok(lms) => lms,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    let page = &lms.page;
    let url = page.current_url().await.unwrap_or_default();

    let fields = match mode {
        Mode::Attach => {
            println!("attached to {url}");
            return;
        }
        Mode::CheckUrlChange => {
            println!("attached to {url}");
            page.execute_script("location.hash = 'moved';", &[]).await.expect("evaluate");
            println!("urlChange -> {:?}", page.wait_for_url_change(&url, Duration::from_secs(3)).await);
            return;
        }
        Mode::Course => course::run(page, &mut Ported, CourseOptions::default()).await.map(|o| o.fields()),
        Mode::Unit => unit::run(page, &mut Ported).await.map(|o| o.fields()),
    };
    match fields {
        Ok(fields) => logger::info("Run finished", fields),
        Err(e) => {
            logger::error("Run failed", [("error", json!(e.to_string()))]);
            std::process::exit(1);
        }
    }
}
