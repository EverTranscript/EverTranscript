//! Transcription quality as a number.
//!
//! The PRD names whisper.cpp streaming quality on real meeting languages as
//! the top unverified risk. A risk you cannot measure cannot be retired, so
//! these run on every fixture transcription and the numbers get reported.
//!
//! Word error rate is the standard measure for English. For Chinese it is
//! close to meaningless — the reference has no spaces, so "words" depend on
//! whichever segmenter you picked — which is why character error rate is
//! computed alongside and is the number to read for Mandarin.

/// The three ways a hypothesis can differ from the reference, plus the rate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ErrorRate {
    pub substitutions: usize,
    pub deletions: usize,
    pub insertions: usize,
    /// Length of the reference, in whatever unit was compared.
    pub reference_length: usize,
}

impl ErrorRate {
    pub fn total_errors(&self) -> usize {
        self.substitutions + self.deletions + self.insertions
    }

    /// Errors per reference token. Can exceed 1.0 when the hypothesis
    /// invents more than the reference contains — which is exactly what a
    /// hallucinating model does, so it is not clamped.
    pub fn rate(&self) -> f64 {
        if self.reference_length == 0 {
            return if self.insertions == 0 { 0.0 } else { 1.0 };
        }
        self.total_errors() as f64 / self.reference_length as f64
    }

    pub fn percent(&self) -> f64 {
        self.rate() * 100.0
    }
}

impl std::fmt::Display for ErrorRate {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{:.1}% ({} sub, {} del, {} ins over {} tokens)",
            self.percent(),
            self.substitutions,
            self.deletions,
            self.insertions,
            self.reference_length
        )
    }
}

/// Lowercases, drops punctuation, and collapses whitespace.
///
/// Without this every comparison is dominated by whether the model wrote a
/// comma, which is not what transcription quality means.
pub fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last_was_space = true;
    for character in text.chars() {
        if character.is_alphanumeric() {
            for lowered in character.to_lowercase() {
                out.push(lowered);
            }
            last_was_space = false;
        } else if !last_was_space {
            out.push(' ');
            last_was_space = true;
        }
    }
    out.trim_end().to_string()
}

/// Word error rate. Use for languages that space their words.
pub fn word_error_rate(reference: &str, hypothesis: &str) -> ErrorRate {
    let reference: Vec<String> = normalize(reference)
        .split_whitespace()
        .map(str::to_string)
        .collect();
    let hypothesis: Vec<String> = normalize(hypothesis)
        .split_whitespace()
        .map(str::to_string)
        .collect();
    align(&reference, &hypothesis)
}

/// Character error rate. The number to read for Chinese, where word
/// boundaries are a segmentation choice rather than a fact.
pub fn character_error_rate(reference: &str, hypothesis: &str) -> ErrorRate {
    let reference: Vec<char> = normalize(reference)
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    let hypothesis: Vec<char> = normalize(hypothesis)
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    align(&reference, &hypothesis)
}

/// Levenshtein alignment, counting each edit type separately.
fn align<T: PartialEq>(reference: &[T], hypothesis: &[T]) -> ErrorRate {
    let (rows, columns) = (reference.len() + 1, hypothesis.len() + 1);

    // Each cell holds (cost, substitutions, deletions, insertions) so the
    // edit types can be reported, not just the total.
    let mut table = vec![(0usize, 0usize, 0usize, 0usize); rows * columns];
    for row in 1..rows {
        table[row * columns] = (row, 0, row, 0);
    }
    for (column, cell) in table.iter_mut().enumerate().take(columns).skip(1) {
        *cell = (column, 0, 0, column);
    }

    for row in 1..rows {
        for column in 1..columns {
            let matched = reference[row - 1] == hypothesis[column - 1];
            let diagonal = table[(row - 1) * columns + column - 1];
            let up = table[(row - 1) * columns + column];
            let left = table[row * columns + column - 1];

            let substitute = (
                diagonal.0 + usize::from(!matched),
                diagonal.1 + usize::from(!matched),
                diagonal.2,
                diagonal.3,
            );
            let delete = (up.0 + 1, up.1, up.2 + 1, up.3);
            let insert = (left.0 + 1, left.1, left.2, left.3 + 1);

            table[row * columns + column] = if matched {
                substitute
            } else {
                let mut best = substitute;
                if delete.0 < best.0 {
                    best = delete;
                }
                if insert.0 < best.0 {
                    best = insert;
                }
                best
            };
        }
    }

    let (_, substitutions, deletions, insertions) = table[rows * columns - 1];
    ErrorRate {
        substitutions,
        deletions,
        insertions,
        reference_length: reference.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_exact_transcription_scores_zero() {
        let reference = "we agreed to defer the hiring plan";
        assert_eq!(word_error_rate(reference, reference).total_errors(), 0);
        assert_eq!(character_error_rate(reference, reference).total_errors(), 0);
    }

    #[test]
    fn punctuation_and_case_are_not_errors() {
        let rate = word_error_rate(
            "We agreed to defer the hiring plan.",
            "we agreed to defer the hiring plan",
        );
        assert_eq!(
            rate.total_errors(),
            0,
            "a missing full stop is not a transcription error"
        );
    }

    #[test]
    fn each_edit_type_is_counted_separately() {
        // reference: a b c d
        // hypothesis: a x c d e   -> one substitution, one insertion
        let rate = word_error_rate("a b c d", "a x c d e");
        assert_eq!(rate.substitutions, 1);
        assert_eq!(rate.insertions, 1);
        assert_eq!(rate.deletions, 0);
        assert_eq!(rate.reference_length, 4);

        let dropped = word_error_rate("a b c d", "a c d");
        assert_eq!(dropped.deletions, 1);
        assert_eq!(dropped.substitutions, 0);
    }

    #[test]
    fn a_hallucination_on_silence_scores_above_one() {
        // Nothing was said; the model produced a sentence. The rate is
        // deliberately unclamped so this is loud rather than capped at 100%.
        let rate = word_error_rate("", "thank you for watching");
        assert!(rate.rate() >= 1.0, "got {rate}");
        assert_eq!(rate.insertions, 4);
    }

    #[test]
    fn silence_transcribed_as_silence_is_perfect() {
        let rate = word_error_rate("", "");
        assert_eq!(rate.rate(), 0.0);
    }

    #[test]
    fn character_error_rate_is_the_usable_measure_for_chinese() {
        let reference = "我们下周的预算评审会议改到周三";
        let hypothesis = "我们下周的预算评审会议改到周四"; // one wrong character

        let characters = character_error_rate(reference, hypothesis);
        assert_eq!(characters.substitutions, 1);
        assert!(
            characters.rate() < 0.1,
            "one wrong character in fifteen is a small error, got {characters}"
        );

        // Word rate on unspaced Chinese sees one giant token and calls a
        // single wrong character a 100% failure — which is why CER exists.
        let words = word_error_rate(reference, hypothesis);
        assert_eq!(words.reference_length, 1);
        assert!(words.rate() >= 1.0);
    }

    #[test]
    fn the_display_is_readable_in_a_test_failure() {
        let rate = word_error_rate("a b c d", "a x c d e");
        let rendered = rate.to_string();
        assert!(rendered.contains("sub"), "{rendered}");
        assert!(rendered.contains("ins"), "{rendered}");
    }
}
