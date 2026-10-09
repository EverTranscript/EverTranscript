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
