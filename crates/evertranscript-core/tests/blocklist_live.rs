//! The blocklist, against a real application on a real microphone.
//!
//! Ticket 09 asks for the false-trigger blocklist to earn its place
//! empirically rather than by unit test. The apps it actually names — a
//! dictation tool, an IDE, a screen recorder — are not installed on every
//! machine, and a test that silently skips is a test nobody notices went
//! missing. So this asks the narrower question the blocklist exists to
//! answer, using whatever bundled application is genuinely holding the
//! microphone: **an app that would otherwise match must not trigger.**

#![cfg(target_os = "macos")]

use evertranscript_core::detect::AppIdentity;
use evertranscript_core::detect::watchlist::Watchlist;
use evertranscript_core::detect::watchlist::WatchlistEntry;

#[test]
fn a_blocked_app_does_not_trigger_even_though_the_watchlist_would_match_it() {
    // Chrome stands in for the shipped blocklist's entries here, because it
    // is present and it *does* match — via Browser Meetings — which is
    // exactly the condition that makes the blocklist load-bearing rather
    // than decorative. A rule that only rejects things nothing else matches
    // has never been tested at all.
    let chrome = AppIdentity::bare("com.google.Chrome");

    let watching = Watchlist::shipped();
    assert!(
        watching.watches(&chrome),
        "sanity: Chrome must match Browser Meetings, or this proves nothing"
    );

    let blocked = Watchlist::shipped().also_blocking("com.google.Chrome");
    assert!(
        !blocked.watches(&chrome),
        "a blocked app triggered anyway; the blocklist is decorative"
    );

    // And explicitly adding it does not win either — membership is not a
    // way around the blocklist.
    let mut insisted = Watchlist::shipped().also_blocking("com.google.Chrome");
    insisted.add(WatchlistEntry::process("com.google.Chrome", "Chrome"));
    assert!(
        !insisted.watches(&chrome),
        "adding a blocked app to the Watchlist overrode the blocklist"
    );
}

#[test]
fn whatever_currently_holds_this_machines_microphone_is_judged_the_same_way() {
    // The live half: whatever is actually recording right now — usually
    // nothing — must be judged by the same rule, and a blocked one must not
    // trigger. This asserts the wiring between the real detector's output
    // and the Watchlist, on real values from this machine rather than
    // invented ones.
    let holders = evertranscript_core::detect::macos::microphone_holders();
    for id in holders {
        let app = AppIdentity::bare(&id);
        let blocked = Watchlist::shipped().also_blocking(&id);
        assert!(
            !blocked.watches(&app),
            "{id} holds the microphone and triggered while blocked"
        );
    }
}

#[test]
fn safari_is_reachable_by_the_name_its_audio_processes_actually_use() {
    // Found by looking at this machine rather than by reasoning: with
    // Safari open, the audio process list holds `com.apple.WebKit.GPU` and
    // never `com.apple.Safari`. Before this mapping existed, Safari — one
    // of the four browsers ADR-0030 names in the M2 matrix — could not
    // trigger a Browser Meeting at all, and no test would have said so
    // because every test asked about the name Safari does not use.
    let list = Watchlist::shipped();
    for observed in ["com.apple.WebKit.GPU", "com.apple.WebKit.WebContent"] {
        let responsible = evertranscript_core::detect::watchlist::responsible_app(observed);
        assert_eq!(responsible, "com.apple.Safari", "attributing {observed}");
        assert!(
            list.watches(&AppIdentity::bare(&responsible)),
            "{observed} should reach Browser Meetings"
        );
    }
}
