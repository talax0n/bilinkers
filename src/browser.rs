use std::time::{Duration, Instant};

use chromiumoxide::{Browser, Page};
use futures::StreamExt;
use serde_json::{json, Value};
use tokio::task::JoinHandle;

use crate::devtools::{page_tabs, ListedTab};
use crate::frame::{evaluate_top, FrameEvaluator};
use crate::logger;
use crate::question_types::Dom;

pub const LMS_HOST: &str = "lms.binus.ac.id";

/// A CDP connection to the user's browser, focused on its LMS tab.
pub struct Lms {
    pub browser: Browser,
    pub page: Page,
    /// The listed tab `page` is.
    pub tab: ListedTab,
    frames: FrameEvaluator,
    events: JoinHandle<()>,
}

// A window can start many runs in one process, so each run's CDP event task ends with it.
impl Drop for Lms {
    fn drop(&mut self) {
        self.events.abort();
    }
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

impl From<chromiumoxide::error::CdpError> for AttachError {
    fn from(err: chromiumoxide::error::CdpError) -> Self {
        AttachError::Connect(err.to_string())
    }
}

/// What a tab says about itself: `document.visibilityState` and `document.hasFocus()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TabView {
    pub visible: bool,
    pub focused: bool,
}

/// Why a tab was picked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickReason {
    /// Visible in the window that has the keyboard focus.
    Focused,
    /// The active tab of some window.
    Visible,
    /// No tab said it was visible, so the first one listed.
    First,
}

impl PickReason {
    pub fn name(self) -> &'static str {
        match self {
            PickReason::Focused => "focused",
            PickReason::Visible => "visible",
            PickReason::First => "first",
        }
    }
}

/// The tab the user is looking at, from views in `/json/list` order (None: it did not answer).
/// Visible and focused wins, then visible, then the first; ties go to the first listed, which
/// Chromium orders most recently activated first.
pub fn choose_tab(views: &[Option<TabView>]) -> Option<(usize, PickReason)> {
    let find = |want: fn(&TabView) -> bool| views.iter().position(|v| v.as_ref().is_some_and(want));
    find(|v| v.visible && v.focused)
        .map(|i| (i, PickReason::Focused))
        .or_else(|| find(|v| v.visible).map(|i| (i, PickReason::Visible)))
        .or_else(|| (!views.is_empty()).then_some((0, PickReason::First)))
}

const TAB_VIEW: &str = "return {v: document.visibilityState, f: document.hasFocus()};";

// A frozen background tab can leave Runtime.evaluate unanswered (seen live on Brave).
const VIEW_TIMEOUT: Duration = Duration::from_millis(1500);

/// Reads a tab's visibility; evaluating neither focuses it nor brings it to front.
async fn tab_view(page: &Page) -> Option<TabView> {
    let value = tokio::time::timeout(VIEW_TIMEOUT, evaluate_top(page, TAB_VIEW, &[])).await.ok()?.ok()?;
    Some(TabView { visible: value["v"] == "visible", focused: value["f"] == true })
}

/// The LMS tab from `tabs` (listing order) that the user is looking at, as a page of `browser`.
pub async fn pick_lms_page(
    browser: &mut Browser,
    tabs: &[ListedTab],
) -> Result<Option<(Page, ListedTab, PickReason)>, AttachError> {
    let lms: Vec<&ListedTab> = tabs.iter().filter(|t| t.is_lms()).collect();
    if lms.is_empty() {
        return Ok(None);
    }
    let mut pages = browser.pages().await?;
    let known = |pages: &[Page], id: &str| pages.iter().any(|p| p.target_id().as_ref() == id);
    if lms.iter().any(|t| !known(&pages, &t.id)) {
        // A tab opened since the last fetch is only a page once the targets are fetched again.
        browser.fetch_targets().await?;
        tokio::time::sleep(Duration::from_millis(300)).await;
        pages = browser.pages().await?;
    }
    let found: Vec<Option<Page>> =
        lms.iter().map(|t| pages.iter().find(|p| p.target_id().as_ref() == t.id).cloned()).collect();
    let views = futures::future::join_all(found.iter().map(|page| async move {
        match page {
            Some(page) => tab_view(page).await,
            None => None,
        }
    }))
    .await;
    let Some((index, why)) = choose_tab(&views) else { return Ok(None) };
    let page = match &found[index] {
        Some(page) => page.clone(),
        None => return Err(AttachError::Connect(format!("tab {} is not attachable yet", lms[index].id))),
    };
    Ok(Some((page, lms[index].clone(), why)))
}

fn log_pick(tab: &ListedTab, why: PickReason) {
    logger::info("Picked LMS tab", [("url", json!(tab.url)), ("title", json!(tab.title)), ("why", json!(why.name()))]);
}

/// Connects to the browser started with --remote-debugging-port and picks the LMS tab the user
/// is looking at, without bringing it to front. CDP directly, so no chromedriver is involved.
pub async fn open_lms(port: u16) -> Result<Lms, AttachError> {
    let (mut browser, mut handler) =
        Browser::connect(format!("http://localhost:{port}")).await.map_err(|e| AttachError::Connect(e.to_string()))?;
    let events = tokio::spawn(async move { while handler.next().await.is_some() {} });

    // Tabs that existed before we connected are only known after a target fetch.
    browser.fetch_targets().await?;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let tabs = page_tabs(port).ok_or_else(|| AttachError::Connect("/json/list did not answer".into()))?;
    match pick_lms_page(&mut browser, &tabs).await? {
        Some((page, tab, why)) => {
            log_pick(&tab, why);
            Ok(Lms { browser, page, tab, frames: FrameEvaluator::default(), events })
        }
        None => {
            events.abort();
            Err(AttachError::NoLmsTab(tabs.into_iter().map(|t| t.url).collect()))
        }
    }
}

/// `open_lms`, then brings the picked tab to front, since pointer events stall on a background tab.
pub async fn attach_to_lms(port: u16) -> Result<Lms, AttachError> {
    let lms = open_lms(port).await?;
    lms.page.bring_to_front().await?;
    Ok(lms)
}

impl Lms {
    /// Picks again over this session and switches to the new tab when the pick moved; logs only
    /// a move. Fails when the session no longer answers.
    pub async fn repick(&mut self, tabs: &[ListedTab]) -> Result<Option<PickReason>, AttachError> {
        let Some((page, tab, why)) = pick_lms_page(&mut self.browser, tabs).await? else { return Ok(None) };
        if tab.id != self.tab.id {
            log_pick(&tab, why);
            self.page = page;
            self.frames = FrameEvaluator::default();
        }
        self.tab = tab;
        Ok(Some(why))
    }
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
        evaluate_top(self, body, args).await
    }

    async fn get(&self, url: &str) -> PageResult<()> {
        navigate(self, "location.href = arguments[0];", url).await
    }

    async fn sleep(&self, ms: u64) {
        tokio::time::sleep(Duration::from_millis(ms)).await;
    }
}

/// Marks the old document, runs `start` (which navigates), and waits for a document without
/// the mark to finish loading. chromiumoxide's own `goto` and `reload` wait on events Chrome 157
/// sends in a shape it cannot read, so they time out after 30 s although the page loaded.
async fn navigate(page: &Page, start: &str, url: &str) -> PageResult<()> {
    evaluate_top(page, &format!("window.__bilinkersOldDocument = true; {start}"), &[json!(url)]).await?;
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(200)).await;
        // A hash-only change keeps the document, so reaching the URL is enough there.
        // Evaluates fail while the old document unloads, which only means not yet.
        if let Ok(state) = evaluate_top(page, NAVIGATION_STATE, &[json!(url)]).await {
            if state == json!(true) {
                return Ok(());
            }
        }
    }
    Err(PageError(format!("navigation to {url} did not finish within 30 s")))
}

const NAVIGATION_STATE: &str = r#"
  const target = arguments[0];
  const fresh = !window.__bilinkersOldDocument || (target.includes('#') && location.href === target);
  return fresh && document.readyState === 'complete';
"#;

/// A page whose activity may render inside an iframe.
#[allow(async_fn_in_trait)]
pub trait FramedPage: Driver {
    /// `executeScript` in the `index`th iframe's document, looked up fresh on every call.
    /// Fails when there is no such iframe or its document is between loads.
    async fn execute_in_frame_at(&self, index: usize, body: &str, args: &[Value]) -> PageResult<Value>;

    /// `switchTo().frame(iframes[0])` then `executeScript`.
    async fn execute_in_frame(&self, body: &str, args: &[Value]) -> PageResult<Value> {
        self.execute_in_frame_at(0, body, args).await
    }

    /// `driver.navigate().refresh()`.
    async fn reload(&self) -> PageResult<()>;

    /// Reloads the tab while listening, and returns the body of the first XHR or fetch reply
    /// whose URL contains `url_part`. `None` when none arrived, or on a page without a network.
    async fn reload_capturing(&self, _url_part: &str) -> PageResult<Option<String>> {
        Ok(None)
    }
}

/// Which document a script runs in, Selenium's current frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Top,
    Frame,
}

/// A page seen through one document: `driver` after `switchTo().frame()` or `defaultContent()`.
pub struct Scoped<'a, P> {
    pub page: &'a P,
    pub scope: Scope,
}

impl<'a, P> Scoped<'a, P> {
    pub fn new(page: &'a P, scope: Scope) -> Self {
        Scoped { page, scope }
    }

    pub fn top(&self) -> Scoped<'a, P> {
        Scoped { page: self.page, scope: Scope::Top }
    }
}

impl<P: FramedPage> Driver for Scoped<'_, P> {
    async fn current_url(&self) -> PageResult<String> {
        self.page.current_url().await
    }

    async fn execute_script(&self, body: &str, args: &[Value]) -> PageResult<Value> {
        match self.scope {
            Scope::Top => self.page.execute_script(body, args).await,
            Scope::Frame => self.page.execute_in_frame(body, args).await,
        }
    }

    async fn get(&self, url: &str) -> PageResult<()> {
        self.page.get(url).await
    }

    async fn sleep(&self, ms: u64) {
        self.page.sleep(ms).await
    }
}

impl Driver for Lms {
    async fn current_url(&self) -> PageResult<String> {
        self.page.current_url().await
    }

    async fn execute_script(&self, body: &str, args: &[Value]) -> PageResult<Value> {
        self.page.execute_script(body, args).await
    }

    async fn get(&self, url: &str) -> PageResult<()> {
        self.page.get(url).await
    }

    async fn sleep(&self, ms: u64) {
        self.page.sleep(ms).await
    }
}

impl FramedPage for Lms {
    async fn execute_in_frame_at(&self, index: usize, body: &str, args: &[Value]) -> PageResult<Value> {
        self.frames.execute(&self.browser, &self.page, index, body, args).await
    }

    async fn reload(&self) -> PageResult<()> {
        navigate(&self.page, "location.reload();", "").await
    }

    async fn reload_capturing(&self, url_part: &str) -> PageResult<Option<String>> {
        let replies = crate::dump::record_responses(
            &self.browser,
            &self.page,
            Duration::from_secs(20),
            crate::dump::Recording::ReloadUntil(url_part),
        )
        .await?;
        Ok(replies.into_iter().find(|r| r.url.contains(url_part)).map(|r| r.body))
    }
}

/// `base + Math.random() * jitter` milliseconds.
pub fn jittered(base_ms: u64, jitter_ms: u64) -> u64 {
    use std::hash::{BuildHasher, Hasher};
    if jitter_ms == 0 {
        return base_ms;
    }
    // A fresh RandomState is randomly seeded, which is all the randomness a pause needs.
    let random = std::collections::hash_map::RandomState::new().build_hasher().finish();
    base_ms + random % jitter_ms
}

/// The answer loop's sleeper, on the page clock so fake pages skip the waits.
pub struct PageSleeper<'a, D>(pub &'a D);

impl<D: Driver> crate::answer_loop::Sleeper for PageSleeper<'_, D> {
    async fn sleep(&mut self, base_ms: u64, jitter_ms: u64) {
        self.0.sleep(jittered(base_ms, jitter_ms)).await
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DomWait {
    pub iframe_wait_ms: u64,
    pub iframe_poll_ms: u64,
    pub launcher_wait_ms: u64,
}

impl Default for DomWait {
    fn default() -> Self {
        DomWait { iframe_wait_ms: 30000, iframe_poll_ms: 200, launcher_wait_ms: 30000 }
    }
}

impl DomWait {
    pub fn iframe_wait(ms: u64) -> Self {
        DomWait { iframe_wait_ms: ms, ..DomWait::default() }
    }
}

/// The question's DOM plus the document it was read from, which later clicks must use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionDom {
    pub dom: Dom,
    pub scope: Scope,
}

pub const IFRAME_COUNT: &str = "return document.querySelectorAll('iframe').length;";

/// Not ready while the LTI launcher form posts, the resume prompt shows, or `#content` is still empty.
const FRAME_NOT_READY: &str = r#"
  if (!document.documentElement || !document.body) return true;
  if (document.body.innerText.includes('Do you want to continue from the page your last visited?')) {
    const no = [...document.querySelectorAll('span')].find((el) => el.textContent.trim() === 'NO');
    if (no) no.parentElement.click();
    return true;
  }
  if (document.getElementById('ltiForm')) return true;
  const content = document.getElementById('content');
  if (!content) return true;
  return content.children.length === 0;
"#;

pub const OUTER_HTML: &str = "return document.documentElement ? document.documentElement.outerHTML : null";

// Earlier listening sliders stay mounted but hidden, so only count a visible one.
const VISIBLE_SLIDER: &str = r#"
  return [...document.querySelectorAll('.MuiSlider-root')].some((el) => el.offsetParent !== null);
"#;

async fn iframe_count<P: Driver>(page: &P) -> PageResult<u64> {
    Ok(page.execute_script(IFRAME_COUNT, &[]).await?.as_u64().unwrap_or(0))
}

/// Port of getCurrentQuestionDom. The iframe is injected late, and inside it a launcher form
/// and then an empty `#content` render before the question, so both are waited out.
pub async fn get_current_question_dom<P: FramedPage>(page: &P, wait: DomWait) -> PageResult<QuestionDom> {
    let mut iframes = iframe_count(page).await?;
    let deadline = Instant::now() + Duration::from_millis(wait.iframe_wait_ms);
    while iframes == 0 && Instant::now() < deadline {
        page.sleep(wait.iframe_poll_ms).await;
        iframes = iframe_count(page).await?;
    }
    let inside_iframe = iframes > 0;

    let mut scope = Scope::Top;
    if inside_iframe {
        scope = Scope::Frame;
        let launcher_deadline = Instant::now() + Duration::from_millis(wait.launcher_wait_ms);
        loop {
            // A frame between loads cannot evaluate, which is the same as not ready.
            let not_ready = page.execute_in_frame(FRAME_NOT_READY, &[]).await.map_or(true, |v| v != json!(false));
            if !not_ready || Instant::now() > launcher_deadline {
                break;
            }
            page.sleep(300).await;
            if iframe_count(page).await? == 0 {
                scope = Scope::Top;
                break;
            }
        }
    }

    let scoped = Scoped::new(page, scope);
    let outer_html = scoped.execute_script(OUTER_HTML, &[]).await?;
    let has_visible_slider = scoped.execute_script(VISIBLE_SLIDER, &[]).await?;
    Ok(QuestionDom {
        dom: Dom {
            outer_html: outer_html.as_str().unwrap_or_default().to_string(),
            inside_iframe,
            has_visible_slider: Some(has_visible_slider == json!(true)),
        },
        scope,
    })
}

/// `goToNextQuestion`: a top-level "Next" button.
pub async fn go_to_next_question<P: Driver>(page: &P) -> PageResult<bool> {
    let clicked = page
        .execute_script(
            r#"
    const buttons = Array.from(document.querySelectorAll('button'));
    const btn = buttons.find((b) => b.textContent.trim() === 'Next');
    if (btn) { btn.click(); return true; }
    return false;
  "#,
            &[],
        )
        .await?;
    Ok(clicked == json!(true))
}

#[cfg(test)]
pub(crate) mod fake {
    //! A scripted page for runner tests: each script is answered by what it contains,
    //! like the JS tests' `executeScript` fakes.
    use super::*;
    use std::cell::RefCell;

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum Call {
        Top(String),
        Frame(String),
        Sleep(u64),
        Reload,
        Get(String),
    }

    pub struct FakePage<F> {
        pub script: RefCell<F>,
        pub calls: RefCell<Vec<Call>>,
        pub url: RefCell<String>,
    }

    impl<F: FnMut(Scope, &str, &[Value]) -> PageResult<Value>> FakePage<F> {
        pub fn new(script: F) -> Self {
            FakePage {
                script: RefCell::new(script),
                calls: RefCell::default(),
                url: RefCell::new("https://lms.binus.ac.id/page".to_string()),
            }
        }

        pub fn sleeps(&self) -> Vec<u64> {
            self.calls.borrow().iter().filter_map(|c| if let Call::Sleep(ms) = c { Some(*ms) } else { None }).collect()
        }

        pub fn scripts_containing(&self, needle: &str) -> usize {
            self.calls
                .borrow()
                .iter()
                .filter(|c| matches!(c, Call::Top(s) | Call::Frame(s) if s.contains(needle)))
                .count()
        }
    }

    impl<F: FnMut(Scope, &str, &[Value]) -> PageResult<Value>> Driver for FakePage<F> {
        async fn current_url(&self) -> PageResult<String> {
            Ok(self.url.borrow().clone())
        }
        async fn execute_script(&self, body: &str, args: &[Value]) -> PageResult<Value> {
            self.calls.borrow_mut().push(Call::Top(body.to_string()));
            (self.script.borrow_mut())(Scope::Top, body, args)
        }
        async fn get(&self, url: &str) -> PageResult<()> {
            self.calls.borrow_mut().push(Call::Get(url.to_string()));
            *self.url.borrow_mut() = url.to_string();
            Ok(())
        }
        async fn sleep(&self, ms: u64) {
            self.calls.borrow_mut().push(Call::Sleep(ms));
        }
    }

    impl<F: FnMut(Scope, &str, &[Value]) -> PageResult<Value>> FramedPage for FakePage<F> {
        async fn execute_in_frame_at(&self, _index: usize, body: &str, args: &[Value]) -> PageResult<Value> {
            self.calls.borrow_mut().push(Call::Frame(body.to_string()));
            (self.script.borrow_mut())(Scope::Frame, body, args)
        }
        async fn reload(&self) -> PageResult<()> {
            self.calls.borrow_mut().push(Call::Reload);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::*;
    use super::*;

    #[tokio::test]
    async fn get_current_question_dom_treats_absent_iframe_body_during_reload_as_not_ready_and_retries_fresh_iframe_lookup(
    ) {
        let mut readiness_checks = 0;
        let page = FakePage::new(|scope: Scope, script: &str, _: &[Value]| {
            if script == IFRAME_COUNT {
                assert_eq!(scope, Scope::Top);
                return Ok(json!(1));
            }
            if script.contains("Do you want to continue from the page your last visited?") {
                assert_eq!(scope, Scope::Frame);
                readiness_checks += 1;
                return Ok(json!(readiness_checks == 1));
            }
            if script == OUTER_HTML {
                assert_eq!(scope, Scope::Frame);
                return Ok(json!("<html><body><main id=\"content\"><div>Ready</div></main></body></html>"));
            }
            if script.contains("MuiSlider-root") {
                assert_eq!(scope, Scope::Frame);
                return Ok(json!(false));
            }
            panic!("Unexpected script: {script}");
        });

        let wait = DomWait { iframe_wait_ms: 100, iframe_poll_ms: 1, launcher_wait_ms: 1000 };
        let result = get_current_question_dom(&page, wait).await.unwrap();

        assert!(result.dom.inside_iframe);
        assert_eq!(result.scope, Scope::Frame);
        assert_eq!(result.dom.outer_html, "<html><body><main id=\"content\"><div>Ready</div></main></body></html>");
        assert_eq!(result.dom.has_visible_slider, Some(false));
        assert_eq!(page.scripts_containing("querySelectorAll('iframe')"), 2);
        assert_eq!(page.scripts_containing("Do you want to continue"), 2);
        assert_eq!(page.sleeps(), vec![300]);
        let order: Vec<&str> = page
            .calls
            .borrow()
            .iter()
            .filter_map(|c| match c {
                Call::Top(s) if s == IFRAME_COUNT => Some("default"),
                Call::Frame(s) if s.contains("Do you want") => Some("iframe"),
                _ => None,
            })
            .collect::<Vec<_>>()
            .into_iter()
            .collect();
        assert_eq!(order, vec!["default", "iframe", "default", "iframe"]);
    }

    const HIDDEN: Option<TabView> = Some(TabView { visible: false, focused: false });
    const VISIBLE: Option<TabView> = Some(TabView { visible: true, focused: false });
    const FOCUSED: Option<TabView> = Some(TabView { visible: true, focused: true });

    #[test]
    fn the_focused_tab_wins_then_a_visible_one_then_the_first_listed() {
        assert_eq!(choose_tab(&[HIDDEN, VISIBLE, FOCUSED]), Some((2, PickReason::Focused)));
        assert_eq!(choose_tab(&[HIDDEN, None, VISIBLE]), Some((2, PickReason::Visible)));
        assert_eq!(choose_tab(&[HIDDEN, HIDDEN]), Some((0, PickReason::First)));
        assert_eq!(choose_tab(&[None, None]), Some((0, PickReason::First)), "no tab answered");
        assert_eq!(choose_tab(&[]), None);
    }

    #[test]
    fn among_visible_tabs_the_first_listed_wins() {
        assert_eq!(choose_tab(&[HIDDEN, VISIBLE, VISIBLE]), Some((1, PickReason::Visible)));
        assert_eq!(choose_tab(&[FOCUSED, FOCUSED]), Some((0, PickReason::Focused)));
        // A focused document that is hidden can't be the one in view.
        let odd = Some(TabView { visible: false, focused: true });
        assert_eq!(choose_tab(&[odd, VISIBLE]), Some((1, PickReason::Visible)));
        assert_eq!(PickReason::First.name(), "first");
    }

    #[tokio::test]
    async fn get_current_question_dom_reads_the_top_document_when_no_iframe_appears() {
        let page = FakePage::new(|scope: Scope, script: &str, _: &[Value]| {
            assert_eq!(scope, Scope::Top);
            Ok(match script {
                IFRAME_COUNT => json!(0),
                OUTER_HTML => json!("<html>top</html>"),
                _ => json!(true),
            })
        });
        let result =
            get_current_question_dom(&page, DomWait { iframe_poll_ms: 1, ..DomWait::iframe_wait(0) }).await.unwrap();
        assert_eq!(result.scope, Scope::Top);
        assert!(!result.dom.inside_iframe);
        assert_eq!(result.dom.outer_html, "<html>top</html>");
        assert_eq!(result.dom.has_visible_slider, Some(true));
    }
}
