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
    /// Whether this install has ever been set up — the Briefing acknowledged,
    /// a Backend chosen, anything at all written. False means a fresh
    /// install; true means an upgrade or a returning Operator.
    pub configured: bool,
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
    /// Fetch, unasked. Only ever on a fresh install.
    Fetch,
    /// Everything needed is already here.
    NothingMissing,
    /// Missing, but this install is not fresh — an upgrade that introduces a
    /// new model asks rather than helping itself. A fresh install consented to
    /// setting the product up; an upgrade consented to a newer version of what
    /// it already had, which is a smaller thing.
    AskFirst,
    /// Missing, and there is not room. Said before starting rather than
    /// discovered at ninety percent, with the numbers so the Operator can act.
    NotEnoughSpace { free_bytes: u64, needed_bytes: u64 },
}

/// Headroom to leave beyond the download itself.
///
/// A disk with exactly enough room for the file has no room for the operating
/// system to work, and a download that fills a disk completely is worse than
/// one that never started — this project has already lost a day's build cache
/// to a disk at 100%.
const HEADROOM: u64 = 2 * 1024 * 1024 * 1024;

/// Whether to fetch what is missing without being asked.
pub fn decide(machine: Machine) -> Provision {
    if machine.models_present {
        return Provision::NothingMissing;
    }
    if machine.configured {
        return Provision::AskFirst;
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

    fn fresh() -> Machine {
        Machine {
            configured: false,
            models_present: false,
            free_bytes: 100 * GIB,
            needed_bytes: 4 * GIB,
        }
    }

    #[test]
    fn a_fresh_install_with_room_fetches() {
        assert_eq!(decide(fresh()), Provision::Fetch);
    }

    #[test]
    fn a_configured_install_asks_instead_of_helping_itself() {
        // The upgrade case. Someone who installed a newer version of what they
        // had did not ask for a multi-gigabyte download to begin by itself.
        let machine = Machine {
            configured: true,
            ..fresh()
        };
        assert_eq!(decide(machine), Provision::AskFirst);
    }

    #[test]
    fn nothing_is_fetched_when_nothing_is_missing() {
        let machine = Machine {
            models_present: true,
            ..fresh()
        };
        assert_eq!(decide(machine), Provision::NothingMissing);
        // And that holds however the rest of the machine looks — a configured
        // install with everything present is not an upgrade to ask about.
        let machine = Machine {
            models_present: true,
            configured: true,
            free_bytes: 0,
            ..fresh()
        };
        assert_eq!(decide(machine), Provision::NothingMissing);
    }

    #[test]
    fn a_full_disk_says_so_before_starting() {
        let machine = Machine {
            free_bytes: 3 * GIB,
            needed_bytes: 4 * GIB,
            ..fresh()
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
            ..fresh()
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
            ..fresh()
        };
        assert!(matches!(decide(machine), Provision::NotEnoughSpace { .. }));
    }

    #[test]
    fn a_core_that_is_never_asked_never_fetches() {
        // Not a property of this function — a property of the design it
        // exists to express. `decide` answers a question; it cannot start a
        // download, so a Core nobody asks provisions nothing, which is what
        // keeps the guarantee tests meaningful.
        let _ = decide(fresh());
    }
}
