//! Shortening a UUIDv7 for a person to read, and expanding what they typed.
//!
//! **A UUIDv7 begins with a 48-bit millisecond timestamp**, so its leading
//! hex characters are *time*, not randomness. Two ids minted close together
//! share a prefix, and how close is arithmetic rather than luck: eight hex
//! characters are 32 of those 48 bits, so any two ids created inside the same
//! 2^16 milliseconds — **sixty-five seconds** — are character-for-character
//! identical.
//!
//! That is not hypothetical. It cost this product a Meeting's audio once
//! already, when two back-to-back recordings resolved to the same Mirror
//! marker and the second overwrote the first. It is also live wherever
//! Diarization is: one run mints every Speaker it finds within the same
//! instant, so two different people would display under the same short id in
//! the Voice Registry — beside a button that deletes a Voiceprint.

/// How many leading hex characters make a UUIDv7 distinguishable.
///
/// Twelve is 48 bits: the whole timestamp, plus the first characters of the
/// random half. Two ids agreeing this far were minted in the same
/// millisecond *and* drew the same leading randomness.
pub const SHORT_CHARS: usize = 12;

/// The leading hex characters of an id, hyphens dropped.
pub fn short(id: &str) -> String {
    id.chars()
        .filter(|c| c.is_ascii_hexdigit())
        .take(SHORT_CHARS)
        .collect()
}

/// The *trailing* hex characters of an id, for one with no filename to match.
///
/// **Leading characters are the wrong end for anything minted in a burst**,
/// and the measurement is unambiguous: two Speakers taken from a real Voice
/// Registry — the product of one Diarization run — share **twenty-one**
/// leading hex characters. Not eight, not twelve. The timestamp is only part
/// of it; `uuid`'s v7 also carries a sub-millisecond counter, so ids created
/// together agree far past the milliseconds they share.
///
/// No prefix length fixes that. The random half is at the other end, so a
/// Speaker is shown by its tail, where those same two ids differ from the
/// first character.
///
/// A Meeting keeps [`short`] instead, because its short form is not free to
/// choose: it must equal the marker in the Mirror filename, so that an id on
/// screen finds the file on disk. Meetings can afford it — two recordings
/// cannot start in the same millisecond, and twelve characters is the whole
/// timestamp.
pub fn short_tail(id: &str) -> String {
    let hex: Vec<char> = id.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    let from = hex.len().saturating_sub(SHORT_CHARS);
    hex[from..].iter().collect()
}

/// What a person typed, reduced to the characters an id is made of.
///
/// Hyphens go, case is folded. This is what makes the three forms a person
/// can be holding — the `list` column, the Mirror filename, and the full
/// hyphenated id — all mean the same thing when they type one back.
pub fn normalise(typed: &str) -> String {
    typed
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eight_characters_would_collide_within_a_minute() {
        // The arithmetic in this module's docs, asserted so it cannot rot
        // into folklore. Two UUIDv7s a second apart agree for eight
        // characters and differ by twelve.
        let first = uuid::Uuid::now_v7().to_string();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let second = uuid::Uuid::now_v7().to_string();
        assert_eq!(first[..8], second[..8], "the timestamp prefix is shared");
        assert_ne!(
            short(&first),
            short(&second),
            "twelve reaches the random half"
        );
    }

    /// Two Speakers copied out of a real Voice Registry, both produced by one
    /// Diarization run. They are the reason `short_tail` exists.
    const BURST: (&str, &str) = (
        "01a071fe-55e6-76e0-9571-acea4492076e",
        "01a071fe-55e6-76e0-9571-ad09cb20699f",
    );

    #[test]
    fn ids_minted_together_share_far_more_than_a_timestamp() {
        let (first, second) = BURST;
        let shared = normalise(first)
            .chars()
            .zip(normalise(second).chars())
            .take_while(|(a, b)| a == b)
            .count();
        assert_eq!(shared, 21, "measured off the Operator's own registry");

        // Which is why no prefix length would have done.
        assert_eq!(
            short(first),
            short(second),
            "leading characters cannot separate these"
        );
        assert_ne!(
            short_tail(first),
            short_tail(second),
            "trailing ones separate them at once"
        );
    }

    #[test]
    fn every_form_a_person_might_hold_normalises_the_same() {
        // The `list` column, the Mirror filename, and the id `show` prints.
        let full = "01a07431-dc59-7ec3-8ef9-1ef2b42f2a29";
        assert!(normalise(full).starts_with(&normalise("01a07431")));
        assert!(normalise(full).starts_with(&normalise("01a07431dc59")));
        assert!(normalise(full).starts_with(&normalise("01A07431DC59")));
        assert_eq!(normalise(full).len(), 32);
    }

    #[test]
    fn a_typed_id_cannot_carry_a_wildcard_into_a_query() {
        // These reach a LIKE pattern. Anything that is not a hex digit is
        // dropped, so `%` and `_` cannot survive to match everything.
        assert_eq!(normalise("01a0%"), "01a0");
        assert_eq!(normalise("_"), "");
        // The hex digits inside those words survive — D, A, B, E, e, e — and
        // nothing else does, which is the property that matters.
        assert_eq!(normalise("'; DROP TABLE meetings--"), "dabeee");
    }
}
