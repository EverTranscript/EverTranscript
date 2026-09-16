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
/// Public alongside [`optimal_mapping`]: a caller scoring something other
/// than time — which persistent identity a reference speaker ended up with,
/// say — still has to agree with `der` about which cluster answers for whom,
/// and agreeing means running the same two functions rather than forming a
/// second opinion.
pub fn by_speaker(spans: &[Span]) -> BTreeMap<String, Vec<(u64, u64)>> {
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

/// Beyond this many speakers on the **narrower** side, the exact assignment
/// is not worth the exponent and a greedy one is used instead.
///
/// AMI meetings have four or five people in them, so the exact path is what
/// runs; the fallback exists so a pathological case degrades to a slightly
/// pessimistic number rather than to a hang.
///
/// **This said the same thing while the code tested the wider side**, which
/// is the hypothesis — a hundred clusters a meeting before merging, never
/// under eighteen. So every DER this file ever reported was greedy, and the
/// comment asserting otherwise is why nobody looked. Raised by the Codex
/// peer reviewing the sweep (DECISIONS Q141).
const EXACT_MAPPING_LIMIT: usize = 18;

/// The reference-to-hypothesis mapping that credits the most time.
///
/// This is the assignment problem, and it has to be solved rather than
/// approximated: which cluster "is" which person is exactly the question
/// confusion measures, and a greedy pairing would charge the system for the
/// scorer's choice. Solved by a subset DP rather than by Hungarian — at five
/// speakers it is a few thousand operations, and it is twenty lines instead
/// of a hundred that nobody will read again.
pub fn optimal_mapping(
    reference: &BTreeMap<String, Vec<(u64, u64)>>,
    hypothesis: &BTreeMap<String, Vec<(u64, u64)>>,
) -> BTreeMap<String, String> {
    let reference_names: Vec<&String> = reference.keys().collect();
    let hypothesis_names: Vec<&String> = hypothesis.keys().collect();
    if reference_names.is_empty() || hypothesis_names.is_empty() {
        return BTreeMap::new();
    }
    let matrix = overlaps(reference, hypothesis);

    // On the *narrow* side. Squaring the matrix and masking the wide one is
    // what sent every real meeting down the greedy path: AMI has four or five
    // people in it, but our clustering offers a hundred candidates for them,
    // and `max` of those two is never four or five.
    let narrow = reference_names.len().min(hypothesis_names.len());
    let pairs = if narrow <= EXACT_MAPPING_LIMIT {
        exact_assignment(&matrix, reference_names.len(), hypothesis_names.len())
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

/// Maximum-weight matching by DP over subsets of the **narrow** side.
///
/// The matrix is rectangular and usually lopsided: four or five people in the
/// room, a hundred clusters offered for them. Masking the narrow side keeps
/// the exponent on the four or five and streams the hundred past it, which is
/// a few thousand operations. Masking the wide side — squaring the matrix and
/// running the DP over hypothesis subsets — puts 2^100 in the way, so the
/// limit above always tripped and greedy always ran.
///
/// `best[mask]` is the most time creditable having walked some prefix of the
/// wide side, with exactly the narrow entries in `mask` spoken for.
fn exact_assignment(matrix: &[Vec<u64>], rows: usize, columns: usize) -> Vec<(usize, usize)> {
    let narrow_is_row = rows <= columns;
    let narrow = rows.min(columns);
    let wide = rows.max(columns);
    let weight = |n: usize, w: usize| -> u64 {
        let (row, column) = if narrow_is_row { (n, w) } else { (w, n) };
        matrix
            .get(row)
            .and_then(|values| values.get(column))
            .copied()
            .unwrap_or(0)
    };

    let masks = 1usize << narrow;
    let mut best = vec![0u64; masks];
    let mut reached = vec![false; masks];
    reached[0] = true;
    // Which narrow entry the wide entry took to arrive at each mask, or NONE.
    // A byte, because `narrow` is bounded by the limit above and this table is
    // the one thing here that scales with both sides at once.
    const NONE: u8 = u8::MAX;
    let mut taken: Vec<Vec<u8>> = Vec::with_capacity(wide);

    for w in 0..wide {
        // Carried forward: a wide entry is allowed to match nobody, which is
        // the usual case when a hundred clusters chase five people.
        let mut next = best.clone();
        let mut next_reached = reached.clone();
        let mut step = vec![NONE; masks];
        for mask in 0..masks {
            if !reached[mask] {
                continue;
            }
            for n in 0..narrow {
                let bit = 1usize << n;
                if mask & bit != 0 {
                    continue;
                }
                let value = best[mask] + weight(n, w);
                let to = mask | bit;
                if !next_reached[to] || value > next[to] {
                    next[to] = value;
                    next_reached[to] = true;
                    step[to] = n as u8;
                }
            }
        }
        best = next;
        reached = next_reached;
        taken.push(step);
    }

    let mut mask = (0..masks)
        .filter(|&candidate| reached[candidate])
        .max_by_key(|&candidate| best[candidate])
        .unwrap_or(0);
    let mut pairs = Vec::new();
    for w in (0..wide).rev() {
        let n = taken[w][mask];
        if n == NONE {
            continue;
        }
        let n = n as usize;
        pairs.push(if narrow_is_row { (n, w) } else { (w, n) });
        mask &= !(1usize << n);
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

    // Sorted once and swept, rather than counted afresh at each candidate
    // threshold. The old shape re-scanned every trial per distinct score,
    // which is quadratic and was fine while trials were pairs of *voices* —
    // a few thousand. The merge threshold is chosen on pairs of *windows*,
    // which is millions, and quadratic does not finish on those at all.
    let mut sorted: Vec<&Trial> = trials.iter().collect();
    sorted.sort_by(|left, right| left.score.total_cmp(&right.score));

    // At the lowest score every trial is accepted: no positive is refused,
    // and every negative is a false accept. Raising the threshold past a
    // group of equal scores moves exactly that group from accepted to
    // refused, which is the whole update.
    let mut false_accepts = negatives;
    let mut false_rejects = 0usize;
    let mut best: Option<(f64, f64, f32)> = None;
    let mut index = 0;
    while index < sorted.len() {
        let threshold = sorted[index].score;
        let far = false_accepts as f64 / negatives as f64;
        let frr = false_rejects as f64 / positives as f64;
        let gap = (far - frr).abs();
        if best.is_none_or(|(previous, _, _)| gap < previous) {
            best = Some((gap, (far + frr) / 2.0, threshold));
        }
        while index < sorted.len() && sorted[index].score == threshold {
            if sorted[index].same_speaker {
                false_rejects += 1;
            } else {
                false_accepts -= 1;
            }
            index += 1;
        }
    }
    best.map(|(_, rate, threshold)| (rate, threshold))
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

/// The lowest threshold that admits no wrong trial at all, and what refusing
/// them costs in right ones.
///
/// **This, not the equal error rate, is the operating point ticket 07 asks
/// for.** "No different-colleague pair above the match floor" is a demand
/// for zero false accepts, and the EER point is the opposite kind of
/// answer — it is where the two mistakes are equally common, which on a
/// weak embedding means being wrong about a third of the time in *both*
/// directions. Choosing the floor off an EER would ship a constant that
/// misattributes a colleague on every third pair.
///
/// Returns the threshold and the share of same-speaker pairs it refuses, so
/// the cost of the guarantee is reported next to the guarantee. That cost is
/// the whole story on a corpus like this one: a floor above every impostor
/// is trivially reachable by setting it near 1.0, and worth nothing.
///
/// `None` when there are no wrong trials to exclude.
pub fn refusal_point(trials: &[Trial]) -> Option<(f32, f64)> {
    let highest_wrong = trials
        .iter()
        .filter(|trial| !trial.same_speaker)
        .map(|trial| trial.score)
        .fold(f32::NEG_INFINITY, f32::max);
    if !highest_wrong.is_finite() {
        return None;
    }
    // Strictly above the worst impostor: the comparison that ships is `>=`,
    // so sitting *at* that score would admit it.
    let threshold = highest_wrong.next_up();
    let positives = trials.iter().filter(|trial| trial.same_speaker).count();
    let refused = trials
        .iter()
        .filter(|trial| trial.same_speaker && trial.score < threshold)
        .count();
    Some((threshold, refused as f64 / positives.max(1) as f64))
}

/// One voice, as a threshold measurement sees it.
#[derive(Debug, Clone, Copy)]
pub struct Candidate<'a> {
    /// Who it actually is. Ground truth, not the pipeline's guess.
    pub speaker: &'a str,
    /// Where it was heard. The nearest voice is sought outside this, so a
    /// meeting cannot recognize itself.
    pub group: &'a str,
    pub vector: &'a [f32],
}

/// How often the closest voice from another meeting is the right person.
///
/// **EER does not answer this and the difference matters.** EER is about a
/// threshold: how separable the two populations are when every pair is
/// judged on its own. What the product does is pick a winner among the
/// voices History holds, so the question an Operator feels is not "would
/// this pair pass a bar" but "did the right person win". A model can have a
/// respectable EER and still put the wrong colleague first, because the
/// pairs it gets wrong are exactly the ones that compete.
///
/// `None` when no voice has a candidate outside its own meeting.
pub fn nearest_is_right(voices: &[Candidate]) -> Option<f64> {
    let mut asked = 0usize;
    let mut right = 0usize;
    for probe in voices {
        let nearest = voices
            .iter()
            .filter(|other| other.group != probe.group)
            .filter(|other| other.vector.len() == probe.vector.len())
            .max_by(|left, right| {
                super::cluster::cosine(probe.vector, left.vector)
                    .partial_cmp(&super::cluster::cosine(probe.vector, right.vector))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        let Some(nearest) = nearest else { continue };
        asked += 1;
        if nearest.speaker == probe.speaker {
            right += 1;
        }
    }
    (asked > 0).then(|| right as f64 / asked as f64)
}

/// How far each probe's winner beat the runner-up, and whether the winner
/// was the right person.
///
/// This is the evidence a *margin* is chosen from, and it is a different
/// question from the one the floor answers. The floor asks whether a score
/// is high enough to be anybody; the margin asks whether the gap to the next
/// candidate is wide enough to be sure it is *this* one. So the score of
/// each trial here is the gap, not the similarity, and `same_speaker` says
/// whether accepting at that gap would have been right.
///
/// One candidate per speaker, taking the best each one offers, because that
/// is what `cluster::resolve` ranks: History holds a Speaker once, as a
/// centroid, not once per Meeting they appeared in. Ranking the raw list
/// would let one person be their own runner-up and report a margin nothing
/// in production ever sees.
pub fn margin_trials(voices: &[Candidate]) -> Vec<Trial> {
    let mut trials = Vec::new();
    for probe in voices {
        let mut best_by_speaker: BTreeMap<&str, f32> = BTreeMap::new();
        for other in voices {
            if other.group == probe.group || other.vector.len() != probe.vector.len() {
                continue;
            }
            let score = super::cluster::cosine(probe.vector, other.vector);
            let slot = best_by_speaker
                .entry(other.speaker)
                .or_insert(f32::NEG_INFINITY);
            if score > *slot {
                *slot = score;
            }
        }
        let mut ranked: Vec<(&str, f32)> = best_by_speaker.into_iter().collect();
        ranked.sort_by(|left, right| right.1.total_cmp(&left.1));
        let Some((winner, best)) = ranked.first().copied() else {
            continue;
        };
        let runner_up = ranked.get(1).map(|(_, score)| *score).unwrap_or(0.0);
        trials.push(Trial {
            score: best - runner_up,
            same_speaker: winner == probe.speaker,
        });
    }
    trials
}

/// What a threshold would cost, at one setting of it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Operating {
    pub threshold: f32,
    /// A different person accepted as the same one. The mistake that shows
    /// an Operator somebody else's name on their colleague's words.
    pub false_accept: f64,
    /// The same person refused. The mistake that mints a new pseudonym for
    /// somebody History already knows.
    pub false_reject: f64,
}

/// The whole curve, not the point on it.
///
/// A threshold reported as a single number says what was chosen and nothing
/// about how much the choice was worth. The M3 close-out is the reason this
/// exists: dev could not choose a threshold at all — its curve was flat from
/// 0.40 to 0.60 while test moved eight points over the same range — and a
/// single number would have hidden that completely, leaving the next person
/// to rediscover it.
pub fn curve(trials: &[Trial], thresholds: &[f32]) -> Vec<Operating> {
    let positives = trials.iter().filter(|trial| trial.same_speaker).count();
    let negatives = trials.len() - positives;
    thresholds
        .iter()
        .map(|&threshold| Operating {
            threshold,
            false_accept: if negatives == 0 {
                0.0
            } else {
                trials
                    .iter()
                    .filter(|trial| !trial.same_speaker && trial.score >= threshold)
                    .count() as f64
                    / negatives as f64
            },
            false_reject: if positives == 0 {
                0.0
            } else {
                trials
                    .iter()
                    .filter(|trial| trial.same_speaker && trial.score < threshold)
                    .count() as f64
                    / positives as f64
            },
        })
        .collect()
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
    fn a_crowd_of_clusters_does_not_push_a_two_speaker_meeting_onto_the_greedy_path() {
        // The same trap as above, with seventeen clusters of nobody added so
        // the hypothesis names nineteen speakers. The room still holds two
        // people, which is the count that decides whether the exact
        // assignment is affordable — and under the version of this scorer
        // that took `max` of the two sides, nineteen sent it to greedy and
        // this charged 1050. Every AMI meeting was on that path.
        let reference = spans(&[("alice", 0, 1_100), ("bob", 2_000, 2_550)]);
        let mut hypothesis = spans(&[("X", 0, 600), ("X", 2_000, 2_550), ("Y", 600, 1_100)]);
        for spare in 0..17 {
            let start = 10_000 + spare * 100;
            hypothesis.push(Span::new(&format!("spare-{spare}"), start, start + 50));
        }
        let scored = der(&reference, &hypothesis);
        assert_eq!(
            scored.confusion_ms, 600,
            "the clusters of nobody changed who alice is: {scored:?}"
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

/// The two measurements ticket 07 chose thresholds against.
#[cfg(test)]
mod choosing_a_threshold {
    use super::*;

    fn candidates<'a>(entries: &'a [(&'a str, &'a str, [f32; 2])]) -> Vec<Candidate<'a>> {
        entries
            .iter()
            .map(|(speaker, group, vector)| Candidate {
                speaker,
                group,
                vector: vector.as_slice(),
            })
            .collect()
    }

    #[test]
    fn the_nearest_voice_is_sought_outside_its_own_meeting() {
        // Otherwise every voice recognizes itself, the rate is 100%, and the
        // measurement says nothing at all. Alice's own Monday self is the
        // closest thing to her by construction.
        let entries = [
            ("alice", "monday", [1.0, 0.0]),
            ("alice", "monday", [0.99, 0.01]),
            ("alice", "friday", [0.98, 0.02]),
            ("bob", "monday", [0.0, 1.0]),
            ("bob", "friday", [0.02, 0.98]),
        ];
        assert_eq!(nearest_is_right(&candidates(&entries)), Some(1.0));
    }

    #[test]
    fn a_wrong_winner_is_counted_even_where_the_pair_would_pass_a_threshold() {
        // The whole reason this is reported beside EER. Both of Friday's
        // voices score high against Monday's Alice; only one of them is her,
        // and the other one wins.
        let entries = [
            ("alice", "monday", [1.0, 0.0]),
            ("carol", "friday", [0.99, 0.14]),
            ("alice", "friday", [0.95, 0.31]),
        ];
        let rate = nearest_is_right(&candidates(&entries)).expect("askable");
        assert!(
            rate < 1.0,
            "the wrong colleague won and it is counted: {rate}"
        );
    }

    #[test]
    fn a_voice_with_nobody_to_compare_against_is_not_counted_as_wrong() {
        let entries = [("alice", "monday", [1.0, 0.0])];
        assert_eq!(nearest_is_right(&candidates(&entries)), None);
    }

    #[test]
    fn the_swept_equal_error_rate_agrees_with_counting_it_the_slow_way() {
        // The sweep replaced a re-count at every distinct score because that
        // was quadratic and window pairs are millions. Speed is not worth a
        // different answer, so the slow way is kept here as the definition
        // and the fast one is checked against it — including the ties and
        // the duplicate scores that the group-advance step exists to handle.
        let mut trials = Vec::new();
        let mut state = 12_345u64;
        for index in 0..400 {
            // Deterministic and lumpy on purpose: scores land on a coarse
            // grid, so equal scores are common and both classes share them.
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            let bucket = (state >> 33) % 12;
            trials.push(Trial {
                score: bucket as f32 / 12.0,
                same_speaker: index % 3 == 0,
            });
        }

        let positives = trials.iter().filter(|trial| trial.same_speaker).count();
        let negatives = trials.len() - positives;
        let mut scores: Vec<f32> = trials.iter().map(|trial| trial.score).collect();
        scores.sort_by(|left, right| left.total_cmp(right));
        scores.dedup();
        let mut slow = (f64::MAX, 0.0f64, 0.0f32);
        for &threshold in &scores {
            let far = trials
                .iter()
                .filter(|trial| !trial.same_speaker && trial.score >= threshold)
                .count() as f64
                / negatives as f64;
            let frr = trials
                .iter()
                .filter(|trial| trial.same_speaker && trial.score < threshold)
                .count() as f64
                / positives as f64;
            let gap = (far - frr).abs();
            if gap < slow.0 {
                slow = (gap, (far + frr) / 2.0, threshold);
            }
        }

        let (rate, threshold) = equal_error_rate(&trials).expect("both classes present");
        assert!(
            (rate - slow.1).abs() < 1e-12 && threshold == slow.2,
            "swept {rate} at {threshold}, counted {} at {}",
            slow.1,
            slow.2
        );
    }

    #[test]
    fn the_refusal_point_sits_just_above_the_best_impostor() {
        let trials = vec![
            Trial {
                score: 0.9,
                same_speaker: true,
            },
            Trial {
                score: 0.5,
                same_speaker: true,
            },
            Trial {
                score: 0.5,
                same_speaker: false,
            },
            Trial {
                score: 0.1,
                same_speaker: false,
            },
        ];
        let (threshold, refused) = refusal_point(&trials).expect("there are impostors to exclude");
        assert!(
            threshold > 0.5 && threshold < 0.9,
            "just above the best impostor, and still below the best genuine pair: {threshold}"
        );
        assert_eq!(
            refused, 0.5,
            "the genuine pair tied with an impostor, so buying the guarantee costs it"
        );
        assert_eq!(
            false_accept_rate_at(&trials, threshold),
            Some(0.0),
            "the point it returns is the point that admits nobody"
        );
    }

    #[test]
    fn a_refusal_point_needs_somebody_to_refuse() {
        let all_genuine = vec![Trial {
            score: 0.4,
            same_speaker: true,
        }];
        assert_eq!(
            refusal_point(&all_genuine),
            None,
            "with no impostor there is no evidence about where to keep one out"
        );
    }

    #[test]
    fn a_margin_trial_scores_the_gap_and_says_whether_the_winner_was_right() {
        // Two probes, one of each kind. Monday's Alice is won by Friday's
        // Alice at a wide gap over Bob; Monday's Carol has nobody like her,
        // so whoever wins is wrong and the gap is what a margin would have
        // to refuse.
        let entries = [
            ("alice", "monday", [1.0, 0.0]),
            ("carol", "monday", [0.71, 0.71]),
            ("alice", "friday", [0.99, 0.14]),
            ("bob", "friday", [0.0, 1.0]),
        ];
        let trials = margin_trials(&candidates(&entries));
        let right: Vec<&Trial> = trials.iter().filter(|t| t.same_speaker).collect();
        let wrong: Vec<&Trial> = trials.iter().filter(|t| !t.same_speaker).collect();
        assert!(!right.is_empty() && !wrong.is_empty(), "{trials:?}");
        assert!(
            right.iter().all(|t| t.score > 0.0),
            "a correct winner beat the runner-up: {right:?}"
        );
        assert!(
            wrong[0].score < right[0].score,
            "and it beat it by more than the wrong winner did, which is the \
             whole reason a margin can tell them apart: {trials:?}"
        );
    }

    #[test]
    fn one_speaker_heard_twice_is_not_their_own_runner_up() {
        // History holds a Speaker once, as a centroid, so `resolve` never
        // ranks Alice against Alice. Ranking the raw list would: her Tuesday
        // vector would be the runner-up to her Friday one, the gap would be
        // tiny, and the margin would be derived against a comparison
        // production cannot make.
        let entries = [
            ("alice", "monday", [1.0, 0.0]),
            ("alice", "friday", [0.99, 0.14]),
            ("alice", "tuesday", [0.98, 0.20]),
            ("bob", "friday", [0.0, 1.0]),
        ];
        let trials = margin_trials(&candidates(&entries));
        let monday = trials.first().expect("a trial for Monday's Alice");
        assert!(
            monday.same_speaker && monday.score > 0.5,
            "the gap is Alice over Bob, not Alice over Alice: {monday:?}"
        );
    }

    #[test]
    fn the_curve_shows_the_two_mistakes_trading_against_each_other() {
        // What a single chosen threshold cannot say: which way the cost
        // moves, and how fast.
        let trials = vec![
            Trial {
                score: 0.9,
                same_speaker: true,
            },
            Trial {
                score: 0.7,
                same_speaker: true,
            },
            Trial {
                score: 0.5,
                same_speaker: false,
            },
            Trial {
                score: 0.3,
                same_speaker: false,
            },
        ];
        let points = curve(&trials, &[0.2, 0.6, 0.95]);
        assert_eq!(points[0].false_accept, 1.0, "accept everything at 0.2");
        assert_eq!(points[0].false_reject, 0.0);
        assert_eq!(points[1].false_accept, 0.0, "both impostors are below 0.6");
        assert_eq!(points[1].false_reject, 0.0, "and both true pairs above it");
        assert_eq!(points[2].false_reject, 1.0, "refuse everyone at 0.95");
    }

    #[test]
    fn an_empty_class_leaves_the_curve_finite_rather_than_dividing_by_zero() {
        // A dev split can legitimately produce no negatives at all, and a
        // curve full of NaN would be worse than one full of zeros: NaN sorts
        // and prints as though it were an answer.
        let trials = vec![Trial {
            score: 0.9,
            same_speaker: true,
        }];
        let points = curve(&trials, &[0.5]);
        assert_eq!(points[0].false_accept, 0.0);
        assert!(points[0].false_reject.is_finite());
    }
}
