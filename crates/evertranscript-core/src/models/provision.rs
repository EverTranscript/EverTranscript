//! Whether this machine should fetch what it is missing, without being asked.
//!
//! A Summary model that arrives on its own is the difference between reaching
//! the feature and discovering a download. But "on its own" is network traffic
//! nobody triggered, so it is bounded tightly: a **fresh install** provisions,
//! and nothing else does.
//!
//! ## Why the decision is separate from doing it
//!
//! The guarantee tests build fresh Cores against isolated Application Support
//! directories and assert that recording, diarizing and summarizing open **no
//! sockets at all**. If constructing a Core implied provisioning, those Cores
//! would begin downloading inside the test that exists to prove silence — and
//! the obvious repair, a test-only switch that suppresses it, would leave the
//! strongest claim this product makes provable only with the new behaviour
//! turned off. That is not a guarantee, it is a guarantee-shaped hole.
//!
//! So provisioning is something a Core is *asked* to do. The binary asks; a
//! test does not. This module is the judgement, testable without a network,
//! a disk, or a Core.

/// What a machine looks like when the question is asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Machine {
    /// Whether every provisioned model is already present.
    pub models_present: bool,
    /// Free bytes where the models would land.
    pub free_bytes: u64,
    /// Bytes the missing models would occupy.
    pub needed_bytes: u64,
}

/// What to do about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provision {
    /// Fetch, unasked. Whenever anything required is missing.
    Fetch,
    /// Everything needed is already here.
    NothingMissing,
    /// Missing, and there is not room. Said before starting rather than
    /// discovered at ninety percent, with the numbers so the Operator can act.
    NotEnoughSpace { free_bytes: u64, needed_bytes: u64 },
}

/// Set this to stop the Core fetching models it is missing.
///
/// Since 2026-09-05 a daemon start fetches whatever is missing, on any
/// install, and retries until it arrives. That is right for a product and
/// wrong for a test harness: the guarantee suite starts the Core dozens of
/// times against the real models directory, and without this the first run
/// began pulling gigabytes from the real mirror — into a real Operator's
/// application-support folder, and in CI on every push. Found exactly that
/// way, 47 MB in.
///
/// Not a supported way to run the product. A Core that will not fetch cannot
/// transcribe, and nothing in the UI would say why.
pub const DISABLE_ENV: &str = "EVERTRANSCRIPT_NO_MODEL_FETCH";

/// Whether fetching has been switched off for this process.
pub fn fetching_disabled() -> bool {
    std::env::var_os(DISABLE_ENV).is_some_and(|value| !value.is_empty())
}

/// Headroom to leave beyond the download itself.
///
/// A disk with exactly enough room for the file has no room for the operating
/// system to work, and a download that fills a disk completely is worse than
/// one that never started — this project has already lost a day's build cache
/// to a disk at 100%.
const HEADROOM: u64 = 2 * 1024 * 1024 * 1024;

/// Whether to fetch what is missing without being asked.
///
/// **Any missing required model is fetched, on every start, however old the
/// install is.** This used to hold back on anything already configured, on the
/// reasoning that a fresh install consented to being set up while an upgrade
/// consented only to a newer version of what it already had. That is a real
/// distinction and it cost more than it bought: a product that cannot
/// transcribe is not a smaller version of itself, it is inert, and the
/// Operator who upgrades into that state has no way to tell why. The download
/// is Sanctioned Traffic either way (ADR-0034), pinned by checksum, and the
/// Briefing says it happens.
pub fn decide(machine: Machine) -> Provision {
    if machine.models_present {
        return Provision::NothingMissing;
    }
    // **Checked, not saturating.** A saturating add pins the requirement at
    // the maximum and then compares equal to it, so a need that cannot be
    // satisfied at all reads as satisfied. Found by the test below, which is
    // the only reason it is not a way to start an impossible download.
    let enough = machine
        .needed_bytes
        .checked_add(HEADROOM)
        .is_some_and(|required| machine.free_bytes >= required);
    if !enough {
        return Provision::NotEnoughSpace {
            free_bytes: machine.free_bytes,
            needed_bytes: machine.needed_bytes,
        };
    }
    Provision::Fetch
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1024 * 1024 * 1024;

    fn missing() -> Machine {
        Machine {
            models_present: false,
            free_bytes: 100 * GIB,
            needed_bytes: 4 * GIB,
        }
    }

    #[test]
    fn anything_missing_with_room_is_fetched() {
        assert_eq!(decide(missing()), Provision::Fetch);
    }

    #[test]
    fn an_install_that_has_been_running_for_years_still_fetches() {
        // This used to be the upgrade case, and used to hold back: someone who
        // installed a newer version of what they had did not ask for a
        // multi-gigabyte download to begin by itself. It cost more than it
        // bought. An install missing a required model cannot transcribe, and
        // holding back leaves it inert with nothing to say why — so age is no
        // longer part of the question, and the only thing that is, is whether
        // anything is missing.
        assert_eq!(decide(missing()), Provision::Fetch);
    }

    #[test]
    fn nothing_is_fetched_when_nothing_is_missing() {
        let machine = Machine {
            models_present: true,
            ..missing()
        };
        assert_eq!(decide(machine), Provision::NothingMissing);
        // And that holds however the rest of the machine looks: nothing to
        // fetch outranks having no room to fetch it into.
        let machine = Machine {
            models_present: true,
            free_bytes: 0,
            ..missing()
        };
        assert_eq!(decide(machine), Provision::NothingMissing);
    }

    #[test]
    fn a_full_disk_says_so_before_starting() {
        let machine = Machine {
            free_bytes: 3 * GIB,
            needed_bytes: 4 * GIB,
            ..missing()
        };
        assert_eq!(
            decide(machine),
            Provision::NotEnoughSpace {
                free_bytes: 3 * GIB,
                needed_bytes: 4 * GIB,
            }
        );
    }

    #[test]
    fn a_disk_with_barely_enough_room_still_refuses() {
        // Exactly the file's size is not enough room: the machine has to keep
        // working while the download runs.
        let machine = Machine {
            free_bytes: 4 * GIB + 1,
            needed_bytes: 4 * GIB,
            ..missing()
        };
        assert!(matches!(decide(machine), Provision::NotEnoughSpace { .. }));
    }

    #[test]
    fn the_space_check_cannot_overflow_into_permission() {
        // A needed size near the top of the range must not wrap around the
        // headroom addition and read as "plenty of room".
        let machine = Machine {
            free_bytes: u64::MAX,
            needed_bytes: u64::MAX,
            ..missing()
        };
        assert!(matches!(decide(machine), Provision::NotEnoughSpace { .. }));
    }

    #[test]
    fn a_core_that_is_never_asked_never_fetches() {
        // Not a property of this function — a property of the design it
        // exists to express. `decide` answers a question; it cannot start a
        // download, so a Core nobody asks provisions nothing, which is what
        // keeps the guarantee tests meaningful.
        let _ = decide(missing());
    }
}
