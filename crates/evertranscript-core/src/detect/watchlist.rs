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
        // Two ids for Comet: ours and the one Granola's table carries. No
        // machine here has Comet to settle which is current, and an extra
        // browser id can only ever over-match a browser, which is what the
        // Browser Meetings row wants anyway.
        "com.perplexity.comet",
        "ai.perplexity.comet",
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
    // Teams does not record from any of its `.helper` processes. Observed
    // live during a Teams call: the only process reporting running input is
    // `com.microsoft.teams2.modulehost`, which the `.helper` rule cannot
    // reach — so the shipped Teams row never matched and Auto-Record never
    // fired for it. Found the same way the WebKit case below was.
    ("com.microsoft.teams2.modulehost", "com.microsoft.teams2"),
];

/// WebKit's out-of-process children, which carry no hint of their host.
///
/// Observed on this machine: with Safari open, the audio process list holds
/// `com.apple.WebKit.GPU` and never `com.apple.Safari`. Nothing in the
/// `.helper` rule reaches that, so **Safari would never have triggered a
/// Browser Meeting** — one of the four browsers ADR-0030 names in the M2
/// matrix, silently missed.
///
/// The cost of the fix is a known, bounded false positive: any WKWebView
/// app holding the microphone is attributed to Safari. ADR-0030 already
/// accepts that shape — "a browser voice app that isn't a meeting can
/// trigger", with the removable row and whole-Meeting delete as the triage
/// — and missing Safari entirely is far worse than labelling a rare
/// non-Safari WebView as Safari.
const WEBKIT_PREFIX: &str = "com.apple.WebKit";

/// Windows executables mapped to the row id their app ships under.
///
/// The absorption catalog calls for "a Windows exe→id twin" of the helper
/// table above. It was never built, and nothing noticed: the Windows
/// detector reports a lowercased executable name, [`Watchlist::watches`]
/// compares ids exactly, and the shipped rows for Zoom, Teams and VooV are
/// macOS bundle ids. Three of the four meeting rows could not match anything
/// on Windows — the platform ADR-0025 makes a ship gate.
///
/// Browsers were unaffected only by accident: [`known_browsers`] lists
/// executables beside bundle ids, so `chrome.exe` reaches Browser Meetings
/// without any mapping. Those names deliberately stay out of this table so
/// the two mechanisms cannot disagree about a browser.
///
/// **Read off a running machine, after a detour through a shipping product.**
/// These were first written from memory, which is the mistake the whole
/// module is a record of, so they were checked against the exe→bundle table
/// in Granola's shipped bundle — the source the absorption catalog names for
/// exactly this. That looked like it had found one error: `wemeetapp.exe`
/// was replaced with `voovmeetingapp.exe`.
///
/// Running both Tencent builds on Windows showed the replacement was itself
/// the error. Granola's table carries only the *international* VooV build,
/// and 腾讯会议 is a different build with a different launcher, so a name
/// that was right for one product was swapped for the name of the other.
/// Neither was wrong; there were simply two. The cost was live: 腾讯会议
/// held the microphone through a whole meeting with the Core at Idle.
///
/// The lesson is about provenance rather than about Tencent. Checking a
/// name against a second source establishes that the name exists somewhere;
/// it cannot tell you which of a vendor's products it belongs to, or that
/// the detector reports it. Only running it does that, which is why the
/// entries here now name what was seen and `examples/mic-holders.rs` exists
/// to see it.
const WINDOWS_EXECUTABLES: &[(&str, &str)] = &[
    ("zoom.exe", "us.zoom.xos"),
    // New Teams ships as `ms-teams.exe`; classic Teams as `teams.exe`. Both
    // map, since which one an Operator runs is not ours to choose.
    ("ms-teams.exe", "com.microsoft.teams2"),
    ("teams.exe", "com.microsoft.teams2"),
    // Tencent ships this product as two builds with two launchers, and both
    // names are real. 腾讯会议 installs `WeMeetApp.exe` under
    // `Program Files\Tencent\WeMeet`; VooV Meeting installs
    // `VooVMeetingApp.exe` under `Program Files (x86)\Tencent\VooVMeeting`.
    // Both were observed holding their own capture session in that
    // top-level process. They resolve to one row on purpose:
    // `com.tencent.tencentmeeting` is also the id VooV was observed under
    // live on macOS.
    ("wemeetapp.exe", "com.tencent.tencentmeeting"),
    ("voovmeetingapp.exe", "com.tencent.tencentmeeting"),
];

/// One application identifier, in the form ids are compared in.
///
/// Bundle ids are case-insensitive to LaunchServices, and Windows executable
/// names are case-insensitive to Windows, so comparing them exactly was a
/// defect rather than a strictness. It cost Arc: the app ships as
/// `company.thebrowser.Browser` and every one of its helper processes as
/// `company.thebrowser.browser.*`, so stripping at `.helper` produced an id
/// that differed from the Watchlist row in one letter and matched nothing.
///
/// Chrome and Edge hid it by lowercasing nothing — `com.google.Chrome.helper`
/// and `com.microsoft.edgemac.helper` are exactly their apps plus a suffix,
/// and both were observed live under those ids. An alias row for Arc would
/// have fixed Arc and left the next vendor to do this undiscovered, so the
/// comparison is what changes.
fn normalized(id: &str) -> String {
    // ASCII-only on purpose: it is length-preserving, which is what lets
    // `responsible_app` search a lowercased copy and slice the original.
    id.to_ascii_lowercase()
}

/// The app responsible for a process.
///
/// A rule first, a table second. Chromium and Electron name their helpers by
/// suffixing the app's own bundle id — `com.google.Chrome.helper.Renderer`,
/// `com.brave.Browser.helper.GPU` — so stripping at `.helper` maps every one
/// of them, including apps that did not exist when this was written. The
/// table is only for the cases the rule cannot reach, which is why it is
/// short rather than long.
pub fn responsible_app(process_id: &str) -> String {
    let lowered = normalized(process_id);
    for (helper, app) in HELPER_EXCEPTIONS {
        if lowered == normalized(helper) {
            return (*app).to_string();
        }
    }
    if lowered.starts_with(&normalized(WEBKIT_PREFIX)) {
        return "com.apple.Safari".to_string();
    }
    for (executable, app) in WINDOWS_EXECUTABLES {
        if lowered == normalized(executable) {
            return (*app).to_string();
        }
    }
    match lowered.find(".helper") {
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
            blocked: blocklist().into_iter().map(normalized).collect(),
            browsers: known_browsers().into_iter().map(normalized).collect(),
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
        let id = normalized(id);
        self.entries.iter().any(|entry| normalized(&entry.id) == id)
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
        let id = normalized(id);
        self.entries.retain(|entry| normalized(&entry.id) != id);
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
        self.blocked.insert(normalized(id));
        self
    }

    pub fn watches(&self, app: &AppIdentity) -> bool {
        let id = normalized(&app.id);
        if self.blocked.contains(&id) {
            return false;
        }
        self.entries.iter().any(|entry| match entry.kind {
            EntryKind::Process => normalized(&entry.id) == id,
            EntryKind::BrowserMeetings => self.browsers.contains(&id),
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
    fn every_browser_in_the_matrix_attributes_its_helpers_to_itself() {
        // ADR-0030's M2 test matrix is Chrome, Safari, Arc and Edge. Three
        // of the four cannot be driven on this machine — Safari has no
        // microphone grant and Arc and Edge are not installed — so the half
        // that does not need them running is asserted here: a renderer
        // belonging to any of them resolves to the browser, and the browser
        // matches Browser Meetings. What is left for a live run is whether
        // the platform reports them at all.
        let list = Watchlist::shipped();
        for (helper, browser) in [
            // Read off the shipped Chrome framework on this machine, not
            // invented: the helper ids Chromium actually builds are
            // `<bundle id>.helper` for the main and GPU helpers and
            // `<bundle id>.helper.renderer` — lowercase — for renderers.
            // `com.google.Chrome.helper` is the one observed holding the
            // microphone during a live Browser Meetings trigger.
            ("com.google.Chrome.helper", "com.google.Chrome"),
            ("com.google.Chrome.helper.renderer", "com.google.Chrome"),
            ("com.google.Chrome.helper.Renderer", "com.google.Chrome"),
            (
                "com.microsoft.edgemac.helper.Renderer",
                "com.microsoft.edgemac",
            ),
            (
                "company.thebrowser.Browser.helper.Renderer",
                "company.thebrowser.Browser",
            ),
            // Safari never appears under its own bundle id in the audio
            // process list — its children do, and they say only "WebKit".
            // Observed on a real machine with Safari open.
            ("com.apple.WebKit.GPU", "com.apple.Safari"),
            ("com.apple.WebKit.WebContent", "com.apple.Safari"),
            ("com.apple.Safari", "com.apple.Safari"),
        ] {
            let responsible = responsible_app(helper);
            assert_eq!(responsible, browser, "attributing {helper}");
            assert!(
                list.watches(&app(&responsible)),
                "{browser} should match Browser Meetings"
            );
        }
    }

    #[test]
    fn teams_holds_the_microphone_under_a_bundle_id_of_its_own() {
        // Observed live: during a Microsoft Teams call the process holding
        // the microphone is `com.microsoft.teams2.modulehost`. It carries no
        // `.helper`, so the rule leaves it alone and the Watchlist row for
        // `com.microsoft.teams2` never matches — Teams, one of the three
        // named meeting apps ADR-0030 ships, would never have triggered.
        //
        // The sibling `.helper` processes exist too but are never the ones
        // recording, which is why the table already having them was not
        // enough. Both are asserted so a future rewrite cannot fix one and
        // silently drop the other.
        let list = Watchlist::shipped();
        for process in [
            "com.microsoft.teams2.modulehost",
            "com.microsoft.teams2.helper",
            "com.microsoft.teams2",
        ] {
            let responsible = responsible_app(process);
            assert_eq!(responsible, "com.microsoft.teams2", "attributing {process}");
            assert!(
                list.watches(&app(&responsible)),
                "{process} should reach the Teams row"
            );
        }
    }

    #[test]
    fn the_windows_executables_reach_the_rows_they_belong_to() {
        // Found by reading, after Teams was found by running. The Windows
        // detector reports a lowercased *executable name*, `watches` is an
        // exact string match, and the shipped rows for Zoom, Teams and VooV
        // are macOS bundle ids — so on Windows all three could never match
        // anything, and the parity gate ADR-0025 sets would have shipped
        // with three of its four meeting rows dead.
        //
        // Browsers escaped this only because `known_browsers` happens to
        // list executables beside bundle ids. The meeting rows have no such
        // list, so the mapping goes where the Windows detector already
        // sends every holder: `responsible_app`.
        let list = Watchlist::shipped();
        for (executable, expected) in [
            ("zoom.exe", "us.zoom.xos"),
            ("ms-teams.exe", "com.microsoft.teams2"),
            ("teams.exe", "com.microsoft.teams2"),
            ("voovmeetingapp.exe", "com.tencent.tencentmeeting"),
        ] {
            let responsible = responsible_app(executable);
            assert_eq!(responsible, expected, "attributing {executable}");
            assert!(
                list.watches(&app(&responsible)),
                "{executable} should reach a shipped row"
            );
        }

        // Browser executables must keep reaching Browser Meetings by the
        // route they already use, not through the new table.
        for browser in ["chrome.exe", "msedge.exe"] {
            let responsible = responsible_app(browser);
            assert_eq!(responsible, browser, "{browser} is responsible for itself");
            assert!(list.watches(&app(&responsible)), "{browser} still matches");
        }
    }

    /// 腾讯会议 and VooV Meeting are two products, and both names were right.
    ///
    /// Observed on Windows 11 on 2026-08-31 with both builds installed side
    /// by side. They ship different launchers from different directories —
    /// `C:\Program Files\Tencent\WeMeet\WeMeetApp.exe` for 腾讯会议 and
    /// `C:\Program Files (x86)\Tencent\VooVMeeting\VooVMeetingApp.exe` for
    /// VooV — and each holds its capture session in that top-level process
    /// rather than in a helper, which is what the run checked.
    ///
    /// `wemeetapp.exe` was watched holding the microphone through a live
    /// meeting while the Core sat at Idle and logged nothing, because this
    /// table carried only VooV's name. Q23 had replaced `wemeetapp.exe` with
    /// `voovmeetingapp.exe` against a competitor's table that covers only the
    /// international build, reading two products as one name to correct.
    /// Both are real, so both belong here.
    ///
    /// They deliberately resolve to the same row: `com.tencent.tencentmeeting`
    /// is the id VooV was observed under live on macOS, so one row serves the
    /// product under either name. Whether the separate shipped `VooV Meeting`
    /// row (`com.tencent.meeting`) is reachable by anything at all is *not*
    /// settled here and is not guessed at.
    #[test]
    fn both_tencent_meeting_builds_reach_a_row() {
        let list = Watchlist::shipped();
        for executable in ["wemeetapp.exe", "voovmeetingapp.exe"] {
            let responsible = responsible_app(executable);
            assert_eq!(
                responsible, "com.tencent.tencentmeeting",
                "attributing {executable}"
            );
            assert!(
                list.watches(&app(&responsible)),
                "{executable} should reach a shipped row"
            );
        }
    }

    #[test]
    fn arc_ships_its_helpers_under_a_different_case_than_its_app() {
        // Read off the installed Arc bundle, not assumed. Arc's app is
        // `company.thebrowser.Browser` — capital B — and every one of its
        // helpers is `company.thebrowser.browser.*`, lowercase. Stripping at
        // `.helper` therefore yields `company.thebrowser.browser`, which is
        // not the string `known_browsers` holds, and `watches` compares
        // exactly. Arc could never have matched Browser Meetings.
        //
        // Chrome and Edge hid this because their vendors happen to lowercase
        // nothing: `com.google.Chrome.helper`, `com.microsoft.edgemac.helper`.
        // Both were observed live holding the microphone under exactly those
        // ids, so the rule looked complete.
        //
        // Bundle ids are case-insensitive to LaunchServices, so matching them
        // case-sensitively was the defect, and a lowercase alias for Arc
        // would only have papered over the next vendor to do this.
        let list = Watchlist::shipped();
        for process in [
            "company.thebrowser.browser.helper",
            "company.thebrowser.browser.helper.renderer",
            "company.thebrowser.Browser.helper.Renderer",
            "company.thebrowser.Browser",
        ] {
            let responsible = responsible_app(process);
            assert!(
                list.watches(&app(&responsible)),
                "{process} resolved to {responsible}, which should match Arc"
            );
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
