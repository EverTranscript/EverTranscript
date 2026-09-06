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
