//! What this installation knows, holds, and may say on the wire.
//!
//! Stories 46 and 47 ask for the guarantees to be *checkable* rather than
//! asserted. "We respect your privacy" is worthless; an enumeration an
//! evaluator can compare against the binary is not.
//!
//! Two rules shape this module.
//!
//! **Enumerate, never reassure.** Every field here is a fact with a source —
//! a file on disk, a row in the database, a constant the guarantee tests
//! already assert against. Nothing is a promise about intent.
//!
//! **Read from the same place the tests do.** `SANCTIONED_TRAFFIC` below is
//! derived from the model registry and the settings rather than typed out,
//! because a hand-maintained list drifts from the binary and the difference
//! between those two is the entire point of the surface. The one thing that
//! *is* typed out — the foreclosed list — is checked by a test against the
//! framework audit that the guarantee suite runs against the real binary.

use serde::Serialize;

/// One thing this product may say on the network (ADR-0034).
///
/// Three, and the list is closed. Anything else appearing here is a
/// milestone-sized decision, not a feature.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SanctionedTraffic {
    pub name: &'static str,
    pub host: String,
    pub what_it_sends: &'static str,
    /// Whether it can happen right now, on this installation, as configured.
    pub enabled: bool,
    pub disableable: bool,
}

/// A capability this product has foreclosed.
///
/// Phrased as what it *cannot* do rather than what it will not, because the
/// first is checkable and the second is a promise. The proof column names
/// where an evaluator can verify it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Foreclosed {
    pub capability: &'static str,
    pub proof: &'static str,
}

/// The things ADR-0020 forecloses, and where each is checkable.
///
/// **Two of these were reversed, and the list says so.** ADR-0020 originally
/// forbade all ambient state observation and any calendar access at all;
/// ADR-0023 reversed the first and ADR-0036 the second. A guarantees page
/// that quietly reflected only the current state, with no sign that the
/// promise had moved, is exactly what an evaluator would find first and
/// trust least.
pub const FORECLOSED: &[Foreclosed] = &[
    Foreclosed {
        capability: "Reads screen content",
        proof: "No ScreenCaptureKit in the binary — asserted by the \
                permission-set audit in tests/guarantees.rs against the built \
                artifact, not the source",
    },
    Foreclosed {
        capability: "Indexes the filesystem",
        proof: "The only paths it opens are its own History folder and its \
                model directory",
    },
    Foreclosed {
        capability: "Reads contacts",
        proof: "No Contacts framework in the binary — same audit",
    },
    Foreclosed {
        capability: "Knows your location",
        proof: "No CoreLocation or MapKit in the binary — both were linked \
                by accident through a dependency's default features in M2, \
                found by that audit, and are now forbidden by name",
    },
    Foreclosed {
        capability: "Sends transcription or speaker recognition anywhere",
        proof: "Transcription and Diarization are Anchors (ADR-0002): they \
                are permanently local and have no cloud option to enable",
    },
    Foreclosed {
        capability: "Contains analytics or crash-reporting",
        proof: "Asserted absent in the binary by tests/guarantees.rs \
                (ADR-0034). Not opt-in — absent",
    },
];

/// Where a guarantee moved, and which decision moved it.
///
/// Shown beside the foreclosed list. A product whose privacy promises have
/// changed should say so itself rather than leave it to whoever reads the
/// ADRs.
pub const AMENDED: &[Foreclosed] = &[
    Foreclosed {
        capability: "It watches which app is running and whether the \
                     microphone is live",
        proof: "ADR-0020 originally forbade all ambient state observation; \
                ADR-0023 reversed that so Auto-Record could exist. It \
                observes state — which app, microphone busy or not — and \
                never content",
    },
    Foreclosed {
        capability: "It can read your calendar, if you grant it",
        proof: "ADR-0020 rejected calendar access outright; ADR-0036 \
                reversed that. The local calendar store only, never a cloud \
                calendar API, and only behind a grant you can decline — \
                declining costs meeting titles and nothing else",
    },
];

/// What this installation holds.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Holdings {
    /// Where the record lives, and the fact that it is the portable unit.
    pub history_dir: String,
    pub meetings: i64,
    pub speakers: i64,
    /// How many Speakers have a stored Voiceprint — the biometric count,
    /// stated as a number rather than a category.
    pub voiceprints: i64,
    /// Models on disk, by display name.
    pub models: Vec<String>,
    /// Whether the calendar grant has been given.
    pub calendar_granted: bool,
}

/// Everything the trust surface shows.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Posture {
    pub holdings: Holdings,
    pub sanctioned_traffic: Vec<SanctionedTraffic>,
    pub foreclosed: &'static [Foreclosed],
    pub amended: &'static [Foreclosed],
    /// Where to read the code that implements all of this (ADR-0033).
    pub source: &'static str,
}

/// The three entries, as this installation is configured.
///
/// Derived rather than listed: the model host comes from the registry, and
/// the Summary entry's `enabled` comes from the actual Knob. A typed-out
/// list would keep claiming a cloud Backend was off after somebody turned
/// it on.
pub fn sanctioned_traffic(
    updates_enabled: bool,
    summary_backend: Option<&str>,
    summary_base_url: Option<&str>,
) -> Vec<SanctionedTraffic> {
    let summary_host = match summary_backend {
        None | Some("local") => None,
        Some(id) => crate::summary::cloud::preset(id)
            .map(|preset| preset.base_url.to_string())
            .or_else(|| summary_base_url.map(str::to_string)),
    };

    vec![
        SanctionedTraffic {
            name: "Update check",
            host: crate::updates::UPDATE_FEED_HOST.to_string(),
            what_it_sends: "The version you are running. Nothing about your \
                            meetings, your machine, or you.",
            enabled: updates_enabled,
            disableable: true,
        },
        SanctionedTraffic {
            name: "Model downloads",
            host: crate::models::registry::base_url(),
            what_it_sends: "A request for a named file, at a moment you \
                            asked for it. Each one is checked against a \
                            pinned checksum.",
            // Only when something is missing and the Operator asks. Never
            // in the background.
            enabled: false,
            disableable: true,
        },
        SanctionedTraffic {
            name: "Cloud Summary",
            host: summary_host
                .clone()
                .unwrap_or_else(|| "not configured".into()),
            what_it_sends: "The full text of a meeting, to the provider you \
                            chose. This is the only thing that ever sends \
                            meeting content anywhere.",
            enabled: summary_host.is_some(),
            disableable: true,
        },
    ]
}

/// True when this installation, right now, would make no network call at all.
///
/// The final form of the zero-network guarantee (ADR-0034): "with updates
/// off and models downloaded, literally zero". Stated as a computed fact so
/// the surface can show it rather than claim it.
pub fn currently_silent(traffic: &[SanctionedTraffic], models_all_present: bool) -> bool {
    models_all_present && traffic.iter().all(|entry| !entry.enabled)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanctioned_traffic_has_exactly_three_entries() {
        // ADR-0034 makes this list closed. A fourth entry is a
        // milestone-sized decision, and this test is what makes adding one
        // deliberate rather than incidental.
        assert_eq!(sanctioned_traffic(true, None, None).len(), 3);
    }

    #[test]
    fn with_local_summary_and_updates_off_nothing_is_enabled() {
        // The guarantee in its final form, computed rather than asserted.
        let traffic = sanctioned_traffic(false, Some("local"), None);
        assert!(traffic.iter().all(|entry| !entry.enabled));
        assert!(currently_silent(&traffic, true));
    }

    #[test]
    fn choosing_a_cloud_backend_shows_up_here_immediately() {
        // The surface must not keep saying "nothing leaves this machine"
        // after somebody turned on the one thing that does.
        let traffic = sanctioned_traffic(false, Some("openai"), None);
        let summary = traffic
            .iter()
            .find(|entry| entry.name == "Cloud Summary")
            .expect("the entry exists");
        assert!(summary.enabled);
        assert!(summary.host.contains("openai"));
        assert!(!currently_silent(&traffic, true));
    }

    #[test]
    fn a_custom_endpoint_is_named_rather_than_hidden() {
        let traffic = sanctioned_traffic(false, Some("my-box"), Some("https://llm.example/v1"));
        let summary = traffic
            .iter()
            .find(|entry| entry.name == "Cloud Summary")
            .expect("the entry exists");
        assert_eq!(summary.host, "https://llm.example/v1");
        assert!(summary.enabled);
    }

    #[test]
    fn updates_on_means_not_silent_even_with_everything_local() {
        // The update check is the reason the switch exists. Claiming
        // silence while it is on would be false in the one direction that
        // matters.
        let traffic = sanctioned_traffic(true, Some("local"), None);
        assert!(!currently_silent(&traffic, true));
    }

    #[test]
    fn a_missing_model_means_not_silent_because_it_will_be_fetched() {
        let traffic = sanctioned_traffic(false, Some("local"), None);
        assert!(!currently_silent(&traffic, false));
    }

    #[test]
    fn the_foreclosed_list_matches_what_the_guarantee_audit_forbids() {
        // The list must not drift from the binary. These four framework
        // names are what tests/guarantees.rs asserts absent, and two of them
        // are there because M2 found them linked by accident through a
        // dependency's default features.
        let text = FORECLOSED
            .iter()
            .map(|item| item.proof)
            .collect::<Vec<_>>()
            .join(" ");
        for framework in ["ScreenCaptureKit", "Contacts", "CoreLocation", "MapKit"] {
            assert!(
                text.contains(framework),
                "{framework} is forbidden by the audit but unmentioned here"
            );
        }
    }

    #[test]
    fn the_amendments_are_shown_rather_than_quietly_absorbed() {
        // ADR-0020 promised two things it no longer promises. A guarantees
        // page reflecting only the current state, with no sign the promise
        // moved, is what an evaluator finds first and trusts least.
        assert_eq!(AMENDED.len(), 2);
        let text = AMENDED
            .iter()
            .map(|item| item.proof)
            .collect::<Vec<_>>()
            .join(" ");
        assert!(text.contains("ADR-0023"), "the detection reversal");
        assert!(text.contains("ADR-0036"), "the calendar reversal");
    }

    #[test]
    fn nothing_here_reassures() {
        // Same rule as the Briefing: enumerate, never comfort. A guarantees
        // page that says "your data is safe" has replaced a checkable claim
        // with an unfalsifiable one.
        let everything = FORECLOSED
            .iter()
            .chain(AMENDED)
            .map(|item| format!("{} {}", item.capability, item.proof))
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        for softener in [
            "rest assured",
            "peace of mind",
            "completely secure",
            "100% private",
        ] {
            assert!(!everything.contains(softener), "found {softener:?}");
        }
    }
}
