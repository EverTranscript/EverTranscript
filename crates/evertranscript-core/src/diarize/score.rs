//! Scoring diarization against a reference: DER, and cross-meeting EER.
//!
//! In the library rather than beside the harness that uses it, for two
//! reasons. It is arithmetic with edge cases — overlapped speech, an
//! optimal speaker mapping, a threshold sweep — and arithmetic with edge
//! cases wants unit tests against worked examples rather than only against a
//! sixteen-meeting corpus nobody can run in CI. And the numbers it produces
//! are the ones ADR-0037 was argued from; a claim of 49.7% that can only be
//! reproduced by a deleted scratch file is not a measurement.
//!
//! **The protocol is the one pyannote publishes under**: BUT's `only_words`
//! references, no forgiveness collar, overlapped speech scored rather than
//! excluded. Every one of those three choices makes the number worse, and
//! all three are what make it comparable to a published figure instead of
//! flattering.

use std::collections::BTreeMap;
use std::collections::BTreeSet;

/// One labelled stretch of one voice, on the common timeline both the
/// reference and the hypothesis live on.
#[derive(Debug, Clone, PartialEq)]
pub struct Span {
    pub speaker: String,
    pub start_ms: u64,
    pub end_ms: u64,
}

impl Span {
    pub fn new(speaker: &str, start_ms: u64, end_ms: u64) -> Self {
        Self {
            speaker: speaker.to_string(),
            start_ms,
            end_ms,
        }
    }

    fn duration_ms(&self) -> u64 {
        self.end_ms.saturating_sub(self.start_ms)
    }
}

/// What a scoring run found, in milliseconds of speech.
///
/// The three error terms are kept apart rather than summed because they say
/// different things about what to fix: missed speech is the segmentation's
/// fault, false alarm is usually crosstalk or an echo the canceller did not
/// take, and confusion is the clustering. M3's close-out attributed its gap
/// by this split, and a harness reporting only the total could not have.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Der {
    /// Total reference speech, counting each overlapping speaker separately.
    /// The denominator.
    pub total_ms: u64,
    /// Reference speech with no hypothesis speaker on it.
    pub missed_ms: u64,
    /// Hypothesis speech with no reference speaker under it.
    pub false_alarm_ms: u64,
    /// Both spoke, and the hypothesis named the wrong one.
    pub confusion_ms: u64,
}

impl Der {
    /// The rate itself, as a fraction. `0.188` is pyannote's published AMI
    /// test figure.
    pub fn rate(&self) -> f64 {
        if self.total_ms == 0 {
            return 0.0;
        }
        (self.missed_ms + self.false_alarm_ms + self.confusion_ms) as f64 / self.total_ms as f64
    }

    pub fn missed_rate(&self) -> f64 {
        self.ratio(self.missed_ms)
    }

    pub fn false_alarm_rate(&self) -> f64 {
        self.ratio(self.false_alarm_ms)
    }

    pub fn confusion_rate(&self) -> f64 {
        self.ratio(self.confusion_ms)
    }

    fn ratio(&self, part: u64) -> f64 {
        if self.total_ms == 0 {
            return 0.0;
        }
        part as f64 / self.total_ms as f64
    }

    /// Adds another meeting's tally, so a corpus figure is the sum of its
    /// parts rather than the mean of their rates.
    ///
    /// Those are different numbers, and the second one is wrong: averaging
    /// sixteen rates lets a ninety-second meeting count as much as a
    /// fifty-minute one. pyannote's published figure is the pooled one.
    pub fn accumulate(&mut self, other: &Der) {
        self.total_ms += other.total_ms;
        self.missed_ms += other.missed_ms;
        self.false_alarm_ms += other.false_alarm_ms;
        self.confusion_ms += other.confusion_ms;
    }
}

/// Scores a hypothesis against a reference.
///
/// No collar and overlap included: every millisecond either side of a
/// boundary counts, and a stretch where two people talk at once contributes
/// two speakers' worth of reference time and needs two hypothesis speakers
/// on it to be right.
pub fn der(reference: &[Span], hypothesis: &[Span]) -> Der {
    let reference = by_speaker(reference);
    let hypothesis = by_speaker(hypothesis);
    let mapping = optimal_mapping(&reference, &hypothesis);

    let mut tally = Der::default();
    for (start, end) in regions(&reference, &hypothesis) {
        let duration = end - start;
        let active_reference: Vec<&String> = reference
            .iter()
            .filter(|(_, spans)| covers(spans, start))
            .map(|(name, _)| name)
            .collect();
        let active_hypothesis: BTreeSet<&String> = hypothesis
            .iter()
            .filter(|(_, spans)| covers(spans, start))
            .map(|(name, _)| name)
            .collect();

        let reference_count = active_reference.len();
        let hypothesis_count = active_hypothesis.len();
        // A reference speaker is *correct* here when the hypothesis speaker
        // they are mapped to is also active here.
        let correct = active_reference
            .iter()
            .filter(|name| {
                mapping
                    .get(**name)
                    .is_some_and(|mapped| active_hypothesis.contains(mapped))
            })
            .count();

        tally.total_ms += duration * reference_count as u64;
        tally.missed_ms += duration * reference_count.saturating_sub(hypothesis_count) as u64;
        tally.false_alarm_ms += duration * hypothesis_count.saturating_sub(reference_count) as u64;
        tally.confusion_ms += duration
            * (reference_count
                .min(hypothesis_count)
                .saturating_sub(correct)) as u64;
    }
    tally
}

/// Groups spans by speaker, merging each speaker's own overlaps.
///
/// A speaker overlapping themselves is a reference artefact, not two people;
/// left unmerged it would count double in the denominator and make every
/// rate look better than it is.
fn by_speaker(spans: &[Span]) -> BTreeMap<String, Vec<(u64, u64)>> {
    let mut grouped: BTreeMap<String, Vec<(u64, u64)>> = BTreeMap::new();
    for span in spans {
        if span.duration_ms() == 0 {
            continue;
        }
        grouped
            .entry(span.speaker.clone())
            .or_default()
            .push((span.start_ms, span.end_ms));
    }
    for ranges in grouped.values_mut() {
        ranges.sort_unstable();
        let mut merged: Vec<(u64, u64)> = Vec::with_capacity(ranges.len());
        for &(start, end) in ranges.iter() {
            match merged.last_mut() {
                Some(last) if start <= last.1 => last.1 = last.1.max(end),
                _ => merged.push((start, end)),
            }
        }
        *ranges = merged;
    }
    grouped
}

fn covers(ranges: &[(u64, u64)], at: u64) -> bool {
    ranges.iter().any(|&(start, end)| at >= start && at < end)
}

/// Every instant where the set of active speakers can change, paired up into
/// the regions between them. Inside a region both sides are constant, so one
/// membership test per speaker scores the whole of it.
fn regions(
    reference: &BTreeMap<String, Vec<(u64, u64)>>,
    hypothesis: &BTreeMap<String, Vec<(u64, u64)>>,
) -> Vec<(u64, u64)> {
    let mut boundaries: BTreeSet<u64> = BTreeSet::new();
    for ranges in reference.values().chain(hypothesis.values()) {
        for &(start, end) in ranges {
            boundaries.insert(start);
            boundaries.insert(end);
        }
    }
    let ordered: Vec<u64> = boundaries.into_iter().collect();
    ordered.windows(2).map(|pair| (pair[0], pair[1])).collect()
}

/// How much time each reference speaker and each hypothesis speaker share.
fn overlaps(
    reference: &BTreeMap<String, Vec<(u64, u64)>>,
    hypothesis: &BTreeMap<String, Vec<(u64, u64)>>,
) -> Vec<Vec<u64>> {
    reference
        .values()
        .map(|left| {
            hypothesis
                .values()
                .map(|right| shared_ms(left, right))
                .collect()
        })
        .collect()
}

fn shared_ms(left: &[(u64, u64)], right: &[(u64, u64)]) -> u64 {
    let mut total = 0;
    for &(a_start, a_end) in left {
        for &(b_start, b_end) in right {
            let start = a_start.max(b_start);
            let end = a_end.min(b_end);
            total += end.saturating_sub(start);
        }
    }
    total
}

/// Beyond this many speakers on either side, the exact assignment is not
/// worth the exponent and a greedy one is used instead.
///
/// AMI meetings have four or five people in them, so the exact path is what
/// actually runs; the fallback exists so a pathological hypothesis — the
/// 503-Speaker History that migration 10 pruned, say — degrades to a
/// slightly pessimistic number rather than to a hang.
const EXACT_MAPPING_LIMIT: usize = 18;

/// The reference-to-hypothesis mapping that credits the most time.
///
/// This is the assignment problem, and it has to be solved rather than
/// approximated: which cluster "is" which person is exactly the question
/// confusion measures, and a greedy pairing would charge the system for the
/// scorer's choice. Solved by a subset DP rather than by Hungarian — at five
/// speakers it is a few thousand operations, and it is twenty lines instead
/// of a hundred that nobody will read again.
fn optimal_mapping(
    reference: &BTreeMap<String, Vec<(u64, u64)>>,
    hypothesis: &BTreeMap<String, Vec<(u64, u64)>>,
) -> BTreeMap<String, String> {
    let reference_names: Vec<&String> = reference.keys().collect();
    let hypothesis_names: Vec<&String> = hypothesis.keys().collect();
    if reference_names.is_empty() || hypothesis_names.is_empty() {
        return BTreeMap::new();
    }
    let matrix = overlaps(reference, hypothesis);

    let side = reference_names.len().max(hypothesis_names.len());
    let pairs = if side <= EXACT_MAPPING_LIMIT {
        exact_assignment(&matrix, side)
    } else {
        greedy_assignment(&matrix, hypothesis_names.len())
    };

    pairs
        .into_iter()
        // Padding rows and columns exist only to square the matrix; a pair
        // naming one is not a mapping between two real speakers.
        .filter(|&(row, column)| row < reference_names.len() && column < hypothesis_names.len())
        .map(|(row, column)| {
            (
                reference_names[row].to_string(),
                hypothesis_names[column].to_string(),
            )
        })
        .collect()
}

/// Maximum-weight perfect matching by DP over subsets of hypothesis
/// speakers, on the matrix squared out with zeros.
///
/// Squared first because the DP assigns reference speaker `k` at the step
/// where `k + 1` columns are taken, which only visits every reference
/// speaker when the two sides are the same size. With more people in the
/// room than clusters found, an unsquared DP would silently force the
/// alphabetically-first reference speakers to be the matched ones — a
/// scorer's arbitrary choice charged to the system as confusion.
///
/// `best[mask]` is the most time creditable using reference speakers
/// `0..mask.count_ones()` and exactly the hypothesis speakers in `mask`.
fn exact_assignment(matrix: &[Vec<u64>], side: usize) -> Vec<(usize, usize)> {
    let weight = |row: usize, column: usize| -> u64 {
        matrix
            .get(row)
            .and_then(|values| values.get(column))
            .copied()
            .unwrap_or(0)
    };

    let states = 1usize << side;
    let mut best = vec![0u64; states];
    let mut chose = vec![usize::MAX; states];

    for mask in 1..states {
        let row = mask.count_ones() as usize - 1;
        for column in 0..side {
            if mask & (1 << column) == 0 {
                continue;
            }
            let previous = mask & !(1 << column);
            let candidate = best[previous] + weight(row, column);
            if candidate >= best[mask] && (chose[mask] == usize::MAX || candidate > best[mask]) {
                best[mask] = candidate;
                chose[mask] = column;
            }
        }
    }

    let mut pairs = Vec::with_capacity(side);
    let mut mask = states - 1;
    while mask != 0 {
        let column = chose[mask];
        if column == usize::MAX {
            break;
        }
        pairs.push((mask.count_ones() as usize - 1, column));
        mask &= !(1 << column);
    }
    pairs
}

/// Best-pair-first, for the pathological case the exact path refuses.
fn greedy_assignment(matrix: &[Vec<u64>], columns: usize) -> Vec<(usize, usize)> {
    let mut cells: Vec<(u64, usize, usize)> = Vec::new();
    for (row, values) in matrix.iter().enumerate() {
        for (column, &weight) in values.iter().enumerate() {
            if weight > 0 {
                cells.push((weight, row, column));
            }
        }
    }
    cells.sort_unstable_by_key(|(weight, _, _)| std::cmp::Reverse(*weight));

    let mut used_rows = vec![false; matrix.len()];
    let mut used_columns = vec![false; columns];
    let mut pairs = Vec::new();
    for (_, row, column) in cells {
        if used_rows[row] || used_columns[column] {
            continue;
        }
        used_rows[row] = true;
        used_columns[column] = true;
        pairs.push((row, column));
    }
    pairs
}

/// Relabels a hypothesis with the reference speaker who actually dominates
/// each span: perfect clustering, the pipeline's own turn placement.
///
/// This is what separates the two halves of the error. Whatever DER remains
/// after this is the cost of *where* the boundaries are — speech missed,
/// speech invented, a turn that runs on past its speaker — and no clustering
/// work can reduce it. The gap between it and the real number is what
/// clustering is costing. M3's close-out put this floor at 32.6% against a
/// measured 49.7%, which is why ADR-0037 rebuilt turn placement rather than
/// only swapping the embedding.
///
/// A span no reference speaker covers keeps its own label, so it still
/// scores as the false alarm it is. An oracle that quietly deleted those
/// would report a floor lower than any clusterer could reach.
pub fn oracle_relabel(hypothesis: &[Span], reference: &[Span]) -> Vec<Span> {
    let grouped = by_speaker(reference);
    hypothesis
        .iter()
        .map(|span| {
            let best = grouped
                .iter()
                .map(|(name, ranges)| (shared_ms(ranges, &[(span.start_ms, span.end_ms)]), name))
                .filter(|&(shared, _)| shared > 0)
                .max_by_key(|&(shared, name)| (shared, std::cmp::Reverse(name)));
            match best {
                Some((_, name)) => Span::new(name, span.start_ms, span.end_ms),
                None => span.clone(),
            }
        })
        .collect()
}

/// Parses an RTTM file into spans, keeping only `SPEAKER` lines.
///
/// RTTM is whitespace-separated with the fields in fixed positions:
/// `SPEAKER <file> <channel> <start> <duration> <NA> <NA> <speaker> …`.
/// Times are seconds with a decimal point; they become milliseconds here so
/// everything downstream is integer arithmetic on the same clock the capture
/// uses.
pub fn parse_rttm(text: &str) -> Vec<Span> {
    text.lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() < 8 || fields[0] != "SPEAKER" {
                return None;
            }
            let start: f64 = fields[3].parse().ok()?;
            let duration: f64 = fields[4].parse().ok()?;
            let start_ms = (start * 1000.0).round().max(0.0) as u64;
            let end_ms = ((start + duration) * 1000.0).round().max(0.0) as u64;
            Some(Span::new(fields[7], start_ms, end_ms))
        })
        .collect()
}

/// One comparison between two voices, and whether they are in fact the same
/// person.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Trial {
    pub score: f32,
    pub same_speaker: bool,
}

/// Equal error rate: the operating point where the two mistakes cost the
/// same.
///
/// Reported rather than accuracy at a fixed threshold because the threshold
/// is ours to choose (`MATCH_FLOOR`), and a number that moves when we move a
/// constant measures the constant rather than the model.
///
/// Returns the rate and the threshold it was reached at, so the second can
/// be compared against the floor the product actually ships.
pub fn equal_error_rate(trials: &[Trial]) -> Option<(f64, f32)> {
    let positives = trials.iter().filter(|trial| trial.same_speaker).count();
    let negatives = trials.len() - positives;
    if positives == 0 || negatives == 0 {
        return None;
    }

    let mut thresholds: Vec<f32> = trials.iter().map(|trial| trial.score).collect();
    thresholds.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    thresholds.dedup();

    let mut best = (f64::MAX, 0.0, 0.0f32);
    for &threshold in &thresholds {
        // Accept at or above the threshold, which is how `resolve` reads
        // `MATCH_FLOOR`.
        let false_accepts = trials
            .iter()
            .filter(|trial| !trial.same_speaker && trial.score >= threshold)
            .count();
        let false_rejects = trials
            .iter()
            .filter(|trial| trial.same_speaker && trial.score < threshold)
            .count();
        let far = false_accepts as f64 / negatives as f64;
        let frr = false_rejects as f64 / positives as f64;
        let gap = (far - frr).abs();
        if gap < best.0 {
            best = (gap, (far + frr) / 2.0, threshold);
        }
    }
    Some((best.1, best.2))
}

/// The share of trials that are wrong at a threshold the product ships.
///
/// EER says how separable the two populations are; this says what happens at
/// the constant in the code. The close-out's "15–44% of different colleagues
/// above `MATCH_FLOOR`" was this number, and it is the one that predicts
/// what an Operator sees.
pub fn false_accept_rate_at(trials: &[Trial], threshold: f32) -> Option<f64> {
    let negatives = trials.iter().filter(|trial| !trial.same_speaker).count();
    if negatives == 0 {
        return None;
    }
    let accepted = trials
        .iter()
        .filter(|trial| !trial.same_speaker && trial.score >= threshold)
        .count();
    Some(accepted as f64 / negatives as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spans(entries: &[(&str, u64, u64)]) -> Vec<Span> {
        entries
            .iter()
            .map(|&(speaker, start, end)| Span::new(speaker, start, end))
            .collect()
    }

    #[test]
    fn a_perfect_hypothesis_scores_zero_whatever_the_labels_are() {
        // The mapping is the point: the system never sees the reference's
        // names, so "A" answering for "alice" is a correct answer.
        let reference = spans(&[("alice", 0, 1_000), ("bob", 1_000, 2_000)]);
        let hypothesis = spans(&[("1", 0, 1_000), ("0", 1_000, 2_000)]);
        let scored = der(&reference, &hypothesis);
        assert_eq!(scored.total_ms, 2_000);
        assert_eq!(scored.rate(), 0.0, "{scored:?}");
    }

    #[test]
    fn the_mapping_is_optimal_rather_than_greedy() {
        // Greedy takes the single biggest cell first and is then forced into
        // the worse pairing for the rest.
        //
        //          X      Y
        // alice   600    500
        // bob     550      0
        //
        // Greedy takes alice/X at 600 and strands bob on Y at 0, crediting
        // 600. The optimal assignment is alice/Y + bob/X, crediting 1050.
        let reference = spans(&[("alice", 0, 1_100), ("bob", 2_000, 2_550)]);
        let hypothesis = spans(&[("X", 0, 600), ("X", 2_000, 2_550), ("Y", 600, 1_100)]);
        let scored = der(&reference, &hypothesis);
        // Every millisecond is covered by exactly one speaker on each side,
        // so there is no missed speech and no false alarm; the only error
        // available is confusion, and the optimal mapping minimises it.
        assert_eq!(scored.missed_ms, 0, "{scored:?}");
        assert_eq!(scored.false_alarm_ms, 0, "{scored:?}");
        // 1650 ms of reference speech, 1050 of it credited.
        assert_eq!(
            scored.confusion_ms, 600,
            "greedy would charge 1050 here: {scored:?}"
        );
    }

    #[test]
    fn overlapped_speech_is_scored_rather_than_forgiven() {
        // Two people talk over each other for a second and the system hears
        // one voice. Under a protocol that excluded overlap this would be
        // free; under pyannote's it is half a second missed out of two
        // seconds of reference speech.
        let reference = spans(&[("alice", 0, 1_000), ("bob", 0, 1_000)]);
        let hypothesis = spans(&[("0", 0, 1_000)]);
        let scored = der(&reference, &hypothesis);
        assert_eq!(scored.total_ms, 2_000, "both speakers count: {scored:?}");
        assert_eq!(scored.missed_ms, 1_000);
        assert_eq!(scored.rate(), 0.5);
    }

    #[test]
    fn speech_invented_where_there_was_none_is_false_alarm() {
        let reference = spans(&[("alice", 0, 1_000)]);
        let hypothesis = spans(&[("0", 0, 1_000), ("1", 1_000, 1_500)]);
        let scored = der(&reference, &hypothesis);
        assert_eq!(scored.false_alarm_ms, 500, "{scored:?}");
        assert_eq!(scored.missed_ms, 0);
        assert_eq!(scored.confusion_ms, 0);
    }

    #[test]
    fn a_speaker_overlapping_themselves_counts_once() {
        // A reference artefact, not two people. Counted twice it would
        // inflate the denominator and flatter every rate computed from it.
        let reference = spans(&[("alice", 0, 1_000), ("alice", 500, 1_500)]);
        let hypothesis = spans(&[("0", 0, 1_500)]);
        let scored = der(&reference, &hypothesis);
        assert_eq!(scored.total_ms, 1_500);
        assert_eq!(scored.rate(), 0.0, "{scored:?}");
    }

    #[test]
    fn there_is_no_collar_around_a_boundary() {
        // 100 ms late on a boundary costs 100 ms of confusion. Scorers that
        // forgive 250 ms either side would report this as perfect, which is
        // why the protocol is stated rather than assumed.
        let reference = spans(&[("alice", 0, 1_000), ("bob", 1_000, 2_000)]);
        let hypothesis = spans(&[("0", 0, 1_100), ("1", 1_100, 2_000)]);
        let scored = der(&reference, &hypothesis);
        assert_eq!(scored.confusion_ms, 100, "{scored:?}");
    }

    #[test]
    fn the_oracle_keeps_the_boundaries_and_fixes_only_the_labels() {
        // Perfect clustering cannot recover speech that was never found, nor
        // give back a boundary placed 200 ms late. What it does remove is
        // every confusion.
        let reference = spans(&[("alice", 0, 1_000), ("bob", 1_000, 2_000)]);
        // Two turns, both put in one cluster — the clustering error. The
        // first boundary is also 200 ms late and the last 300 ms of bob was
        // never found, which are the placement errors underneath it.
        let hypothesis = spans(&[("0", 0, 1_200), ("0", 1_200, 1_700)]);

        let plain = der(&reference, &hypothesis);
        let oracle = der(&reference, &oracle_relabel(&hypothesis, &reference));
        // Not zero, and that is the point. The first span runs 200 ms past
        // alice into bob, and a span gets one label — so the overhang stays
        // confusion however perfect the clustering is. That residue is
        // turn-placement error wearing confusion's name, and a floor that
        // hid it would be unreachable rather than a floor.
        assert_eq!(oracle.confusion_ms, 200, "{oracle:?}");
        assert!(
            oracle.confusion_ms < plain.confusion_ms,
            "but far less than the real run's: {plain:?} vs {oracle:?}"
        );
        assert_eq!(
            oracle.missed_ms, plain.missed_ms,
            "and the missed speech is untouched: {plain:?} vs {oracle:?}"
        );
        assert!(
            oracle.rate() > 0.0 && oracle.rate() < plain.rate(),
            "a floor, not a zero: {} vs {}",
            oracle.rate(),
            plain.rate()
        );
    }

    #[test]
    fn the_oracle_does_not_quietly_delete_false_alarms() {
        // Relabelling a span nobody was speaking under would make the floor
        // lower than any clusterer could reach.
        let reference = spans(&[("alice", 0, 1_000)]);
        let hypothesis = spans(&[("0", 0, 1_000), ("1", 5_000, 6_000)]);
        let oracle = der(&reference, &oracle_relabel(&hypothesis, &reference));
        assert_eq!(oracle.false_alarm_ms, 1_000, "{oracle:?}");
    }

    #[test]
    fn rttm_parses_to_milliseconds() {
        let text = "SPEAKER ES2004a 1 12.340 2.500 <NA> <NA> MEE014 <NA> <NA>\n\
                    SPKR-INFO ES2004a 1 <NA> <NA> <NA> unknown MEE014 <NA> <NA>\n\
                    SPEAKER ES2004a 1 20.000 1.000 <NA> <NA> MEE015 <NA> <NA>\n";
        let parsed = parse_rttm(text);
        assert_eq!(
            parsed,
            vec![
                Span::new("MEE014", 12_340, 14_840),
                Span::new("MEE015", 20_000, 21_000),
            ],
            "SPKR-INFO lines are not speech"
        );
    }

    #[test]
    fn the_equal_error_rate_finds_where_the_two_mistakes_balance() {
        // Perfectly separable: there is a threshold with neither mistake.
        let trials = vec![
            Trial {
                score: 0.9,
                same_speaker: true,
            },
            Trial {
                score: 0.8,
                same_speaker: true,
            },
            Trial {
                score: 0.2,
                same_speaker: false,
            },
            Trial {
                score: 0.1,
                same_speaker: false,
            },
        ];
        let (rate, threshold) = equal_error_rate(&trials).expect("eer");
        assert_eq!(rate, 0.0);
        assert!(
            (0.2..=0.8).contains(&threshold),
            "the threshold sits in the gap: {threshold}"
        );
    }

    #[test]
    fn a_model_that_cannot_tell_them_apart_scores_fifty_percent() {
        let trials = vec![
            Trial {
                score: 0.5,
                same_speaker: true,
            },
            Trial {
                score: 0.5,
                same_speaker: false,
            },
        ];
        let (rate, _) = equal_error_rate(&trials).expect("eer");
        assert_eq!(rate, 0.5);
    }

    #[test]
    fn the_false_accept_rate_is_read_at_the_threshold_we_ship() {
        // The close-out's "15-44% of different colleagues above MATCH_FLOOR"
        // is this number, and it is the one an Operator feels.
        let trials = vec![
            Trial {
                score: 0.70,
                same_speaker: false,
            },
            Trial {
                score: 0.65,
                same_speaker: false,
            },
            Trial {
                score: 0.30,
                same_speaker: false,
            },
            Trial {
                score: 0.20,
                same_speaker: false,
            },
            Trial {
                score: 0.95,
                same_speaker: true,
            },
        ];
        assert_eq!(false_accept_rate_at(&trials, 0.62), Some(0.5));
    }

    #[test]
    fn an_empty_hypothesis_misses_everything_rather_than_dividing_by_zero() {
        let reference = spans(&[("alice", 0, 1_000)]);
        let scored = der(&reference, &[]);
        assert_eq!(scored.missed_ms, 1_000);
        assert_eq!(scored.rate(), 1.0);
        assert_eq!(
            der(&[], &[]).rate(),
            0.0,
            "and nothing against nothing is 0"
        );
    }

    #[test]
    fn a_corpus_figure_pools_rather_than_averages_rates() {
        // A ninety-second meeting must not count as much as a fifty-minute
        // one. Pooled: 1100/11000 = 10%. Averaged: (100% + 5%)/2 = 52.5%.
        let mut pooled = Der::default();
        pooled.accumulate(&Der {
            total_ms: 1_000,
            missed_ms: 600,
            false_alarm_ms: 0,
            confusion_ms: 400,
        });
        pooled.accumulate(&Der {
            total_ms: 10_000,
            missed_ms: 100,
            false_alarm_ms: 0,
            confusion_ms: 0,
        });
        assert_eq!(pooled.total_ms, 11_000);
        assert!((pooled.rate() - 0.1).abs() < 1e-9, "{}", pooled.rate());
    }
}
