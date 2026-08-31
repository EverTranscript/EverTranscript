//! The update check: Sanctioned Traffic entry one, and the switch that
//! silences it.
//!
//! ADR-0016 chose direct download plus in-app updates precisely so shipping
//! never waits on an app store's opinion of system-audio capture. ADR-0034
//! then made the check one of exactly three things this product may ever say
//! on the wire, **disableable in Settings** — and the guarantee test's final
//! form depends on that switch: "with updates off and models downloaded,
//! literally zero".
//!
//! So the switch is checked before anything is constructed, not before
//! anything is sent. A disabled updater that still resolves a hostname has
//! already broken the promise, and DNS is traffic.
//!
//! What the check sends is a version and nothing else. No machine
//! identifier, no install id, no counter — those are telemetry with a
//! different name, and ADR-0034 says the binary contains none.

use std::time::Duration;

/// Where the update feed lives.
///
/// Named here rather than assembled at the call site so the trust surface
/// can show the exact host an Operator would see in a firewall log.
pub const UPDATE_FEED_HOST: &str = "https://github.com/EverTranscript/EverTranscript";

/// The feed itself.
pub const UPDATE_FEED_PATH: &str = "/releases/latest";

/// A check is a background nicety, not a thing worth waiting for.
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(10);

/// What a check found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateStatus {
    /// The Operator turned checks off, so none was made. **Not an error and
    /// not a failure** — it is the configuration working.
    Disabled,
    UpToDate {
        current: String,
    },
    Available {
        current: String,
        latest: String,
    },
    /// The check could not complete. Deliberately not surfaced as an error
    /// the Operator must dismiss: a laptop on a plane is not a problem to
    /// report, and an update check must never interrupt a recording.
    Unreachable {
        reason: String,
    },
}

impl UpdateStatus {
    /// Whether this is worth telling the Operator about unprompted.
    ///
    /// Only an available update is. The other three are states, and a
    /// product that popped a dialog because it could not reach GitHub would
    /// be interrupting a meeting to report the weather.
    pub fn worth_mentioning(&self) -> bool {
        matches!(self, Self::Available { .. })
    }
}

/// Checks for a newer release, if checks are enabled.
///
/// The `enabled` flag is the first thing read, and returning before any
/// client is built is the point: nothing is resolved, connected, or sent.
pub fn check(enabled: bool, current_version: &str) -> UpdateStatus {
    if !enabled {
        return UpdateStatus::Disabled;
    }

    let client = match reqwest::blocking::Client::builder()
        .timeout(CHECK_TIMEOUT)
        // A version string is the entire payload. It is also, deliberately,
        // the entire user agent: a fingerprint assembled from OS build,
        // architecture and locale would be telemetry with a different name.
        .user_agent(format!("EverTranscript/{current_version}"))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            return UpdateStatus::Unreachable {
                reason: error.to_string(),
            };
        }
    };

    let url = format!("{UPDATE_FEED_HOST}{UPDATE_FEED_PATH}");
    let response = match client.head(&url).send() {
        Ok(response) => response,
        Err(error) => {
            return UpdateStatus::Unreachable {
                reason: error.to_string(),
            };
        }
    };

    // The redirect target names the tag. Nothing is downloaded and no body
    // is read: the check is a question about a version, not a fetch.
    let latest = response
        .url()
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .unwrap_or_default()
        .trim_start_matches('v')
        .to_string();

    if latest.is_empty() || latest == "latest" {
        return UpdateStatus::Unreachable {
            reason: "the release feed did not name a version".into(),
        };
    }
    if latest == current_version {
        UpdateStatus::UpToDate {
            current: current_version.to_string(),
        }
    } else {
        UpdateStatus::Available {
            current: current_version.to_string(),
            latest,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_disabled_check_makes_no_call_at_all() {
        // The whole reason the switch exists. Returning before a client is
        // built is what makes "literally zero" true — a disabled updater
        // that still resolved a hostname would already have broken it, and
        // DNS is traffic.
        assert_eq!(check(false, "0.1.0"), UpdateStatus::Disabled);
    }

    #[test]
    fn being_disabled_is_a_state_rather_than_a_failure() {
        // An Operator who turned updates off has configured the product,
        // not broken it, and nothing should nag them about it.
        assert!(!UpdateStatus::Disabled.worth_mentioning());
    }

    #[test]
    fn only_an_available_update_is_worth_interrupting_for() {
        assert!(
            UpdateStatus::Available {
                current: "0.1.0".into(),
                latest: "0.2.0".into()
            }
            .worth_mentioning()
        );
        for quiet in [
            UpdateStatus::UpToDate {
                current: "0.1.0".into(),
            },
            UpdateStatus::Unreachable {
                reason: "offline".into(),
            },
            UpdateStatus::Disabled,
        ] {
            assert!(
                !quiet.worth_mentioning(),
                "{quiet:?} must not interrupt anyone"
            );
        }
    }

    #[test]
    fn an_unreachable_feed_is_not_an_error_the_operator_must_dismiss() {
        // A laptop on a plane is not a problem to report, and an update
        // check must never interrupt a recording.
        let offline = check(true, "0.1.0");
        // Either it reached GitHub or it did not; both are acceptable here,
        // and neither may be a thing that demands attention.
        assert!(!matches!(offline, UpdateStatus::Disabled));
        if let UpdateStatus::Unreachable { .. } = offline {
            assert!(!offline.worth_mentioning());
        }
    }

    #[test]
    fn the_feed_host_is_nameable_for_the_trust_surface() {
        // Story 46: an evaluator should be able to compare this against
        // what they see in a firewall log.
        assert!(UPDATE_FEED_HOST.starts_with("https://"));
        assert!(!UPDATE_FEED_HOST.contains("track"));
    }
}
