//! The Watchlist: what Meeting Detection watches, and what it refuses to.
//!
//! ADR-0024's trigger is Watchlist membership AND microphone activity. This
//! module owns the membership half — the shipped defaults of ADR-0030, the
//! Operator's edits, and the two pieces of seed data that make the match
//! honest: helper processes mapped to the app responsible for them, and a
//! blocklist of things that hold a microphone without being a meeting.
//!
//! Membership is the per-app switch (ADR-0030). There is no enabled column
//! and no per-app toggle: an app is watched because it is on the list, and
//! the single Auto-Record switch stays the only thing that turns the
//! ambient behaviour off.

use std::collections::BTreeSet;

use super::AppIdentity;

/// What a row matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EntryKind {
    /// One application, by bundle id (macOS) or executable name (Windows).
    Process,
    /// Any browser holding a hot microphone (ADR-0030). One entry rather
    /// than a row per site, because it matches a browser *in a call* — which
    /// covers Google Meet and the web variants of Zoom, Teams and Webex
    /// without knowing any of their URLs.
    BrowserMeetings,
}

/// A row on the list.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct WatchlistEntry {
    /// Bundle id, executable name, or [`BROWSER_MEETINGS_ID`].
    pub id: String,
    pub name: String,
    pub kind: EntryKind,
}

impl WatchlistEntry {
    pub fn process(id: &str, name: &str) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            kind: EntryKind::Process,
        }
    }

    pub fn browser_meetings() -> Self {
        Self {
            id: BROWSER_MEETINGS_ID.to_string(),
            name: "Browser Meetings".to_string(),
            kind: EntryKind::BrowserMeetings,
        }
    }
}

/// The sentinel id for the Browser Meetings row.
pub const BROWSER_MEETINGS_ID: &str = "browser-meetings";

/// What ships watched (ADR-0030).
pub fn shipped_defaults() -> Vec<WatchlistEntry> {
    vec![
        WatchlistEntry::process("us.zoom.xos", "Zoom"),
        WatchlistEntry::process("com.microsoft.teams2", "Microsoft Teams"),
        // Both VooV builds: the international one and 腾讯会议.
        WatchlistEntry::process("com.tencent.meeting", "VooV Meeting"),
        WatchlistEntry::process("com.tencent.tencentmeeting", "腾讯会议"),
        WatchlistEntry::browser_meetings(),
    ]
}

/// Known call apps that ship *suggested*, off the list, one act to add
/// (ADR-0030).
///
/// WeChat is not a default because default-recording personal one-to-one
/// calls is the wiretap story ADR-0024's any-mic-use rejection named, and it
/// widens all-party-consent exposure well past meetings.
pub fn suggested_entries() -> Vec<WatchlistEntry> {
    vec![WatchlistEntry::process("com.tencent.xinWeChat", "WeChat")]
}

/// Things that hold a microphone and are not a meeting.
///
/// Negative seed data, and authoritative: a blocklisted app never triggers,
/// even when it would otherwise match Browser Meetings. That last clause is
/// the point — several of these are Electron apps that a naive browser test
/// would happily call a browser.
pub fn blocklist() -> Vec<&'static str> {
    vec![
        // Dictation — a hot mic by design.
        "com.superwhisper",
        "com.voiceink.app",
        "com.electron.wispr-flow",
        "com.goodsnooze.MacWhisper",
        "com.apple.VoiceMemos",
        // Editors and terminals with push-to-talk or voice features.
        "com.microsoft.VSCode",
        "dev.warp.Warp-Stable",
        "com.exafunction.windsurf",
        "com.todesktop.230313mzl4w4u92", // Cursor
        // Screen recording.
        "com.obsproject.obs-studio",
        "com.loom.desktop",
        "com.screen.studio",
        "com.apple.QuickTimePlayerX",
        // AI assistants.
        "com.openai.chat",
        "com.anthropic.claudefordesktop",
    ]
}

/// Browsers, for the Browser Meetings row.
pub fn known_browsers() -> Vec<&'static str> {
    vec![
        "com.google.Chrome",
        "com.apple.Safari",
        "company.thebrowser.Browser", // Arc
        "com.microsoft.edgemac",
        "org.mozilla.firefox",
        "com.brave.Browser",
        "com.vivaldi.Vivaldi",
        "com.operasoftware.Opera",
        "com.perplexity.comet",
        // Windows executables, so one list serves both platforms.
        "chrome.exe",
        "msedge.exe",
        "firefox.exe",
        "brave.exe",
        "arc.exe",
        "opera.exe",
    ]
}

/// Bundle ids that are a different app's helper, where the name does not
/// say so.
///
/// **Derived, not ported.** ADR-0030 and the absorption catalog both point
/// at an upstream table of ~25 helper ids as the "fragile edge pre-solved";
/// none of the reference rigs were present on the machine this was written
/// on, so it could not be ported and is not claimed as one. Reconcile it
/// against the upstream table — with attribution and a `PORTS.md` entry —
/// on a machine that has the sources.
const HELPER_EXCEPTIONS: &[(&str, &str)] = &[
    // Zoom's in-meeting process is a separate bundle, not a `.helper` suffix.
    ("us.zoom.ZoomHybridConf", "us.zoom.xos"),
    ("us.zoom.ZoomClips", "us.zoom.xos"),
    ("com.microsoft.teams2.helper", "com.microsoft.teams2"),
];

/// The app responsible for a process.
///
/// A rule first, a table second. Chromium and Electron name their helpers by
/// suffixing the app's own bundle id — `com.google.Chrome.helper.Renderer`,
/// `com.brave.Browser.helper.GPU` — so stripping at `.helper` maps every one
/// of them, including apps that did not exist when this was written. The
/// table is only for the cases the rule cannot reach, which is why it is
/// short rather than long.
pub fn responsible_app(process_id: &str) -> String {
    for (helper, app) in HELPER_EXCEPTIONS {
        if process_id == *helper {
            return (*app).to_string();
        }
    }
    match process_id.find(".helper") {
        Some(cut) => process_id[..cut].to_string(),
        None => process_id.to_string(),
    }
}

/// The list this installation watches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Watchlist {
    entries: Vec<WatchlistEntry>,
    blocked: BTreeSet<String>,
    browsers: BTreeSet<String>,
}

impl Default for Watchlist {
    fn default() -> Self {
        Self::shipped()
    }
}

impl Watchlist {
    /// What a fresh installation watches.
    pub fn shipped() -> Self {
        Self::from_entries(shipped_defaults())
    }

    pub fn from_entries(entries: Vec<WatchlistEntry>) -> Self {
        Self {
            entries,
            blocked: blocklist().into_iter().map(str::to_string).collect(),
            browsers: known_browsers().into_iter().map(str::to_string).collect(),
        }
    }

    pub fn entries(&self) -> &[WatchlistEntry] {
        &self.entries
    }

    /// Suggested rows not already on the list.
    pub fn suggestions(&self) -> Vec<WatchlistEntry> {
        suggested_entries()
            .into_iter()
            .filter(|entry| !self.contains(&entry.id))
            .collect()
    }

    pub fn contains(&self, id: &str) -> bool {
        self.entries.iter().any(|entry| entry.id == id)
    }

    pub fn add(&mut self, entry: WatchlistEntry) -> bool {
        if self.contains(&entry.id) {
            return false;
        }
        self.entries.push(entry);
        self.entries.sort();
        true
    }

    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|entry| entry.id != id);
        self.entries.len() != before
    }

    /// Whether this app is one the Operator asked to have watched.
    ///
    /// The blocklist is checked first and wins: an app that holds a
    /// microphone without being a meeting must not become one by being
    /// Electron-shaped.
    /// Blocks an app for the life of this list. Test-facing: the shipped
    /// blocklist is seed data, and this is how a test can ask whether the
    /// mechanism works using an application that actually exists on the
    /// machine running it.
    pub fn also_blocking(mut self, id: &str) -> Self {
        self.blocked.insert(id.to_string());
        self
    }

    pub fn watches(&self, app: &AppIdentity) -> bool {
        if self.blocked.contains(&app.id) {
            return false;
        }
        self.entries.iter().any(|entry| match entry.kind {
            EntryKind::Process => entry.id == app.id,
            EntryKind::BrowserMeetings => self.browsers.contains(&app.id),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(id: &str) -> AppIdentity {
        AppIdentity::bare(id)
    }

    #[test]
    fn the_shipped_list_is_exactly_what_adr_0030_says() {
        let list = Watchlist::shipped();
        for watched in [
            "us.zoom.xos",
            "com.microsoft.teams2",
            "com.tencent.meeting",
            "com.tencent.tencentmeeting",
        ] {
            assert!(list.watches(&app(watched)), "{watched} ships watched");
        }
        assert!(
            list.contains(BROWSER_MEETINGS_ID),
            "Browser Meetings ships as one entry"
        );
    }

    #[test]
    fn wechat_ships_suggested_rather_than_watched() {
        // ADR-0030: reachable by the Operator's own act, never the shipped
        // posture. Recording personal one-to-one calls by default is the
        // wiretap story the any-mic-use rejection named.
        let mut list = Watchlist::shipped();
        assert!(!list.watches(&app("com.tencent.xinWeChat")));
        assert!(
            list.suggestions().iter().any(|e| e.name == "WeChat"),
            "but it is offered"
        );

        let wechat = list.suggestions().remove(0);
        list.add(wechat);
        assert!(
            list.watches(&app("com.tencent.xinWeChat")),
            "one act to add"
        );
        assert!(
            list.suggestions().is_empty(),
            "and then no longer suggested"
        );
    }

    #[test]
    fn any_browser_with_a_hot_mic_is_a_browser_meeting() {
        // One entry covering Google Meet plus the web variants of the
        // desktop apps, which the process rows never did.
        let list = Watchlist::shipped();
        for browser in [
            "com.google.Chrome",
            "com.apple.Safari",
            "company.thebrowser.Browser",
            "com.microsoft.edgemac",
        ] {
            assert!(list.watches(&app(browser)), "{browser} is a browser");
        }
    }

    #[test]
    fn a_hot_microphone_that_is_not_a_meeting_never_triggers() {
        // The blocklist earning its place: each of these holds a microphone
        // by design and none of them is a meeting.
        let list = Watchlist::shipped();
        for stranger in [
            "com.superwhisper",
            "com.todesktop.230313mzl4w4u92",
            "com.obsproject.obs-studio",
            "com.openai.chat",
            "com.anthropic.claudefordesktop",
        ] {
            assert!(!list.watches(&app(stranger)), "{stranger} is not a meeting");
        }
    }

    #[test]
    fn the_blocklist_beats_the_browser_rule() {
        // The case a naive browser test gets wrong: several blocklisted apps
        // are Electron, and Electron is Chromium. Membership must not be
        // won by being browser-shaped.
        let mut list = Watchlist::shipped();
        list.add(WatchlistEntry::process("com.openai.chat", "ChatGPT"));
        assert!(
            !list.watches(&app("com.openai.chat")),
            "blocklisted apps stay blocked even when explicitly added"
        );
    }

    #[test]
    fn helpers_resolve_to_the_app_responsible_for_them() {
        // Policy must never see a renderer. The rule covers every Chromium
        // and Electron app by construction.
        for (process, expected) in [
            ("com.google.Chrome.helper.Renderer", "com.google.Chrome"),
            ("com.google.Chrome.helper", "com.google.Chrome"),
            ("com.brave.Browser.helper.GPU", "com.brave.Browser"),
            (
                "company.thebrowser.Browser.helper.Plugin",
                "company.thebrowser.Browser",
            ),
            // The exception the rule cannot reach.
            ("us.zoom.ZoomHybridConf", "us.zoom.xos"),
            // An ordinary app is already responsible for itself.
            ("us.zoom.xos", "us.zoom.xos"),
        ] {
            assert_eq!(responsible_app(process), expected, "mapping {process}");
        }
    }

    #[test]
    fn a_chrome_renderer_holding_the_mic_is_chrome_in_a_meeting() {
        // The two pieces together, which is the fragile edge ADR-0030 named.
        let list = Watchlist::shipped();
        let responsible = responsible_app("com.google.Chrome.helper.Renderer");
        assert!(list.watches(&app(&responsible)));
    }

    #[test]
    fn adding_and_removing_is_the_per_app_switch() {
        let mut list = Watchlist::shipped();
        assert!(list.add(WatchlistEntry::process("com.webex.meetingmanager", "Webex")));
        assert!(
            !list.add(WatchlistEntry::process("com.webex.meetingmanager", "Webex")),
            "no duplicates"
        );
        assert!(list.watches(&app("com.webex.meetingmanager")));
        assert!(list.remove("com.webex.meetingmanager"));
        assert!(!list.watches(&app("com.webex.meetingmanager")));
        assert!(
            !list.remove("com.webex.meetingmanager"),
            "removing twice is not an error"
        );
    }
}
