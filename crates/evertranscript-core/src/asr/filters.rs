//! Rejecting what the model invents.
//!
//! Every filter here exists because our record is immutable (ADR-0009). A
//! transcription error in a product that lets you edit the transcript is an
//! annoyance; here it is a permanent line in the Operator's History that
//! they cannot correct. That asymmetry is why this is a layered defence
//! rather than a single confidence threshold:
//!
//! 1. The chunker never sends silence to the model (`vad`).
//! 2. The model is configured not to invent (`whisper`).
//! 3. Whatever still comes back is filtered here.

use evertranscript_protocol::ChineseScript;

/// Phrases whisper produces from silence and room tone.
///
/// The English list is the well-known set; the Chinese entries are the
/// subtitle boilerplate Mandarin-capable models fall into, which matters
/// because code-switching meetings are this product's normal case (story 7).
const KNOWN_INVENTIONS: &[&str] = &[
    // English — verified reproducible against our own silence and room-tone
    // fixtures with a real model.
    "thank you",
    "thanks for watching",
    "thank you for watching",
    "thanks for watching!",
    "please subscribe",
    "like and subscribe",
    "you",
    "bye",
    "goodbye",
    "okay",
    "music",
    "music playing",
    "applause",
    "laughter",
    "silence",
    "blank_audio",
    // Chinese — subtitle-corpus boilerplate, the Mandarin equivalent of
    // "thanks for watching".
    "请不吝点赞",
    "请不吝点赞 订阅 转发 打赏",
    "字幕由amara.org社区提供",
    "字幕志愿者",
    "谢谢观看",
    "谢谢大家",
    "明镜与点点栏目",
];

/// Fraction of a result that may be repetition before the whole thing is
/// discarded. Above this it is a decoder loop, not speech.
const MAX_REPETITION_RATIO: f32 = 0.7;

/// The longest phrase length checked for looping.
const MAX_PHRASE_WORDS: usize = 5;

/// Strips punctuation and case so comparisons are about words, not commas.
fn normalize(text: &str) -> String {
    text.chars()
        .filter(|character| character.is_alphanumeric() || character.is_whitespace())
        .collect::<String>()
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// True when the text is one of the model's known inventions.
pub fn is_known_invention(text: &str) -> bool {
    let normalized = normalize(text);
    if normalized.is_empty() {
        return false;
    }
    KNOWN_INVENTIONS
        .iter()
        .any(|phrase| normalized == normalize(phrase))
}

/// True when the text carries no actual content: too short, or built from
/// almost no distinct characters (`aaaaaaaa`, `......`).
pub fn is_meaningless(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return true;
    }
    let distinct: std::collections::HashSet<char> = trimmed
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    trimmed.chars().count() > 10 && distinct.len() <= 3
}

/// Collapses immediately repeated words: "the the the plan" → "the plan".
pub fn collapse_repeated_words(text: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for word in text.split_whitespace() {
        let repeats = out
            .last()
            .is_some_and(|last| normalize(last) == normalize(word));
        if !repeats {
            out.push(word);
        }
    }
    out.join(" ")
}

/// Collapses repeated multi-word phrases, which is how a stuck decoder
/// usually presents: "we agreed we agreed we agreed" → "we agreed".
pub fn collapse_repeated_phrases(text: &str) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.len() < 4 {
        return text.to_string();
    }

    let mut out: Vec<&str> = Vec::new();
    let mut index = 0;
    while index < words.len() {
        let mut collapsed = false;
        // Longest phrase first, so "a b a b a b" collapses as a pair rather
        // than as three single-word repeats.
        for length in (2..=MAX_PHRASE_WORDS.min((words.len() - index) / 2)).rev() {
            let phrase = &words[index..index + length];
            let next = &words[index + length..index + length * 2];
            if phrase
                .iter()
                .zip(next)
                .all(|(left, right)| normalize(left) == normalize(right))
            {
                out.extend_from_slice(phrase);
                // Skip every further repeat of the same phrase.
                let mut cursor = index + length;
                while cursor + length <= words.len()
                    && phrase
                        .iter()
                        .zip(&words[cursor..cursor + length])
                        .all(|(left, right)| normalize(left) == normalize(right))
                {
                    cursor += length;
                }
                index = cursor;
                collapsed = true;
                break;
            }
        }
        if !collapsed {
            out.push(words[index]);
            index += 1;
        }
    }
    out.join(" ")
}

/// How much of the text is repeated words, from 0 (all distinct) to nearly 1.
pub fn repetition_ratio(text: &str) -> f32 {
    let words: Vec<String> = normalize(text)
        .split_whitespace()
        .map(str::to_string)
        .collect();
    if words.len() < 2 {
        return 0.0;
    }
    let distinct: std::collections::HashSet<&String> = words.iter().collect();
    1.0 - (distinct.len() as f32 / words.len() as f32)
}

/// Rewrites Mandarin into the script the Operator asked for.
///
/// The words are the same either way — this is orthography, not
/// translation — and text with no Han characters comes back untouched.
/// Both directions are handled by phrase, not character by character,
/// which is what makes the ambiguous one safe: Simplified 发 is 發 in
/// 发送 and 髮 in 头发, and a per-character table would have to guess.
pub fn in_script(text: &str, script: ChineseScript) -> String {
    match script {
        ChineseScript::Simplified => hanconv::t2s(text),
        ChineseScript::Traditional => hanconv::s2t(text),
    }
}

/// Runs every filter, returning the text to store or `None` to discard it.
pub fn clean(text: &str, script: ChineseScript) -> Option<String> {
    // Settle the script before anything judges the text. The Chinese
    // entries in `KNOWN_INVENTIONS` are written Simplified, so a
    // Traditional decode of the same subtitle boilerplate used to walk
    // straight past them — the filter was script-dependent by accident.
    let normalized = hanconv::t2s(text);
    let trimmed = normalized.trim();
    if trimmed.is_empty() || is_meaningless(trimmed) || is_known_invention(trimmed) {
        return None;
    }

    // Judge repetition on the *original*, before collapsing.
    //
    // Measuring after collapsing hides the very signal being looked for: a
    // decoder stuck on "we agreed" six times folds to a tidy "we agreed"
    // that scores as clean speech. The loop itself is the evidence that the
    // model was not reporting anything it heard, so it is what gets measured.
    if repetition_ratio(trimmed) > MAX_REPETITION_RATIO {
        return None;
    }

    // A speaker who stutters is still speaking: collapse the repeat and keep
    // the sentence.
    let collapsed = collapse_repeated_phrases(&collapse_repeated_words(trimmed));

    // Re-check after collapsing: an invention repeated a few times only
    // looks like one once the loop is folded away.
    if is_known_invention(&collapsed) {
        return None;
    }
    let collapsed = collapsed.trim();
    if collapsed.is_empty() {
        return None;
    }
    // Judged Simplified, stored as asked.
    Some(in_script(collapsed, script))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What ships, and what every test here means by "the record".
    const SIMPLIFIED: ChineseScript = ChineseScript::Simplified;

    #[test]
    fn mandarin_is_recorded_in_simplified_characters() {
        // Measured in the dogfood run (ticket 11): the speaker read
        // Simplified Chinese and the record came back Traditional, because
        // the model stays on automatic language detection for
        // code-switching (story 7) and picks the script its training data
        // favoured. Character-perfect once normalised, and wrong before.
        let heard = "會議決定推遲投票,等審計報告出來以後再做決定。";
        let want = "会议决定推迟投票,等审计报告出来以后再做决定。";
        assert_eq!(clean(heard, SIMPLIFIED).as_deref(), Some(want));
    }

    #[test]
    fn normalising_the_script_leaves_everything_else_alone() {
        let english = "The council met on Tuesday to review the quarterly budget.";
        assert_eq!(clean(english, SIMPLIFIED).as_deref(), Some(english));
        let simplified = "会议决定推迟投票";
        assert_eq!(clean(simplified, SIMPLIFIED).as_deref(), Some(simplified));
    }

    #[test]
    fn subtitle_boilerplate_is_rejected_in_either_script() {
        // The invention list is written Simplified, so a Traditional decode
        // of the same boilerplate used to walk straight past it.
        assert_eq!(clean("請不吝點贊", SIMPLIFIED), None);
    }

    #[test]
    fn an_operator_who_wants_traditional_gets_traditional() {
        // Simplified ships, but it is a preference and not a fact about the
        // speaker. The words are identical either way.
        let simplified = "会议决定推迟投票";
        assert_eq!(
            clean(simplified, ChineseScript::Traditional).as_deref(),
            Some("會議決定推遲投票")
        );
    }

    #[test]
    fn the_ambiguous_direction_is_resolved_by_phrase_not_by_character() {
        // 发 is 發 in "send" and 髮 in "hair". A per-character table has to
        // guess; this is why the conversion is done by phrase.
        assert_eq!(
            clean("发送邮件，理头发", ChineseScript::Traditional).as_deref(),
            Some("發送郵件，理頭髮")
        );
    }

    #[test]
    fn the_classic_inventions_are_rejected() {
        // Reproduced from our own fixtures with a real model: silence
        // produced "you", room tone produced "Thank you."
        for invention in [
            "you",
            "You",
            "Thank you.",
            "Thank you for watching!",
            "thanks for watching",
            "请不吝点赞",
            "谢谢观看",
            "[BLANK_AUDIO]",
        ] {
            assert!(
                clean(invention, SIMPLIFIED).is_none(),
                "{invention:?} must never reach the record"
            );
        }
    }

    #[test]
    fn real_speech_passes_through_untouched() {
        for real in [
            "we agreed to defer the hiring plan until October",
            "I'll send the revised numbers before Friday",
            "我们下周的预算评审会议改到周三",
            // Contains an invention as a substring but is plainly speech.
            "thank you for sending those numbers over",
        ] {
            assert_eq!(
                clean(real, SIMPLIFIED).as_deref(),
                Some(real),
                "{real:?} is speech and must be kept verbatim"
            );
        }
    }

    #[test]
    fn a_stuck_decoder_is_discarded_rather_than_recorded() {
        let stuck = "we agreed we agreed we agreed we agreed we agreed we agreed";
        assert!(
            clean(stuck, SIMPLIFIED).is_none(),
            "a decoder loop is not a record of anything said"
        );
    }

    #[test]
    fn a_stutter_is_collapsed_not_discarded() {
        // A real speaker repeating a word is speech; the filter must not
        // throw the sentence away.
        let stutter = "the the plan is to defer hiring until October";
        let cleaned = clean(stutter, SIMPLIFIED).expect("this is speech");
        assert_eq!(cleaned, "the plan is to defer hiring until October");
    }

    #[test]
    fn repeated_phrases_collapse_to_one() {
        assert_eq!(
            collapse_repeated_phrases("let me check let me check the numbers"),
            "let me check the numbers"
        );
        assert_eq!(
            collapse_repeated_phrases("okay so okay so okay so we start"),
            "okay so we start"
        );
    }

    #[test]
    fn meaningless_output_is_rejected() {
        assert!(is_meaningless("aaaaaaaaaaaaaa"));
        assert!(is_meaningless("..............."));
        assert!(is_meaningless(""));
        assert!(!is_meaningless("we agreed to defer"));
        // Short strings are not judged by distinct-character count: "ok" is
        // two characters and perfectly real.
        assert!(!is_meaningless("ok"));
    }

    #[test]
    fn the_repetition_ratio_separates_speech_from_looping() {
        let speech = repetition_ratio("we agreed to defer the hiring plan until October");
        let loop_text = repetition_ratio("plan plan plan plan plan plan plan plan");
        assert!(speech < 0.3, "normal speech scores low, got {speech}");
        assert!(loop_text > 0.7, "a loop scores high, got {loop_text}");
    }

    #[test]
    fn punctuation_and_case_do_not_smuggle_an_invention_through() {
        for disguised in ["THANK YOU!", "  thank you.  ", "Thank, you"] {
            assert!(
                clean(disguised, SIMPLIFIED).is_none(),
                "{disguised:?} is the same invention wearing punctuation"
            );
        }
    }

    #[test]
    fn a_looped_invention_is_caught_after_collapsing() {
        assert!(
            clean("thank you thank you thank you", SIMPLIFIED).is_none(),
            "the loop hides an invention until it is folded away"
        );
    }
}
