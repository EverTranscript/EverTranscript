//! Turning embeddings into Speakers — within a Meeting, and across all of
//! them.
//!
//! Story 28 ("every voice resolves to a persistent Speaker") is either true
//! here or it is a slogan. Two decisions carry it.
//!
//! **Recognition is clustering, not a second stage.** The catalog's shape is
//! to seed each Meeting's clusterer with prior Voiceprints as frozen
//! speakers, so a returning voice is recognized by the same code that groups
//! a new one. A separate post-hoc matcher can disagree with the clusterer —
//! it says two turns are one voice, the matcher says they are two people —
//! and when they disagree neither answer is defensible. So [`resolve`] is
//! the *only* place a similarity threshold is applied, and the live
//! clusterer applies it to its seeds through this same function.
//!
//! **Matching is conservative and structurally so.** Three conditions, all
//! required: similarity above a floor, a margin over the runner-up, and
//! mutual agreement in both directions. A confident wrong attribution is
//! worse than an unnamed Speaker — the Operator has to *notice* a wrong name
//! before they can correct it, and a plausible one does not get noticed.

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use super::Cluster;
use super::Embedding;

/// How similar two voices must be before they can be the same person.
///
/// The catalog's reference number, and **measured to hold** once the
/// front end was fixed (DECISIONS Q115). On AMI, with one Voiceprint per
/// person built from their other meetings in the series, a returning
/// person scores their own print at 0.76 or better on the dev set and a
/// stranger's at 0.57 or worse; 0.62 sits in that gap, so dev gives no
/// reason to move it. On the test set it lets one thin print (six seconds
/// of voice) go unrecognized at 0.61 and admits nobody wrongly.
/// Before the fix the same measurement put the *same* person at 0.58 on
/// average, which is why recognition never worked in the shipped builds.
///
/// **Selected rather than inherited since 2026-09-17** (DECISIONS Q231).
/// Held-out AMI test, in the arm that ships, against the two other points
/// predeclared in Q182: 0.30/0.00 is worse on all four recognition
/// quantities, and 0.80/0.00 trades 303.790s of correct returning for
/// 37.700s less wrong with newcomers tied — a trade this loses only if a
/// wrong attributed second is held to cost more than 8.06 correct ones.
pub const MATCH_FLOOR: f32 = 0.62;

/// How far the best candidate must beat the runner-up.
///
/// The condition that makes two similar voices produce *no* match rather
/// than a coin-flip between them. Without it, the closer two colleagues
/// sound, the more confidently the system mislabels them. On the same
/// measurement the smallest margin between a person's own print and the
/// nearest other person's was 0.34, so this is never the binding rule
/// there; it is kept for the pair of colleagues that measurement did not
/// contain.
///
/// Selected with [`MATCH_FLOOR`], which it is chosen as a pair with.
pub const MATCH_MARGIN: f32 = 0.08;

/// Agglomerative merge threshold on L2-normalized embeddings (catalog M3).
pub const MERGE_THRESHOLD: f32 = 0.6;

/// Least voice a cluster must hold before it is minted as a Speaker.
///
/// **Measured into existence**, like the sub-window above. Without a floor
/// every group the clusterer left standing became a permanent Speaker with
/// a Voiceprint, and the first real History this product kept showed what
/// that means: 503 Speakers across six Meetings of three to five people,
/// 378 of them owning no transcribed word — three-second windows of echo
/// and crosstalk, each a stranger in the Registry. Ten seconds is about
/// three sub-windows: enough for the centroid to be an average rather than
/// one window's noise, and more than a cough or a fragment of echo ever
/// gets. A voice under it still exists as turns and still reaches the
/// Transcript, honestly "Unattributed"; it just does not get to be somebody.
pub const MIN_SPEAKER_MS: u64 = 10_000;

/// Most exemplars kept per Speaker.
///
/// A Speaker seen in two hundred Meetings must not carry two hundred vectors
/// into every subsequent clustering run. Keeping the most recent bounds both
/// the work and the drift — a voice from three years and one microphone ago
/// is not better evidence than last week's.
pub const MAX_EXEMPLARS: usize = 32;

/// A voice the system already knows, offered to the clusterer as a seed.
#[derive(Debug, Clone, PartialEq)]
pub struct SeedVoice {
    pub speaker_id: String,
    pub vector: Vec<f32>,
    /// Operator-confirmed Voiceprints win ties (ADR-0008 as amended).
    pub confirmed: bool,
    /// Which model's space this vector lives in, carried so that
    /// [`resolve_with`] can refuse to score it against another one.
    pub model: String,
    pub model_version: String,
}

/// What resolution concluded about one cluster.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved {
    /// Recognized as a Speaker from History.
    Existing(String),
    /// A voice this installation has not heard before, or has heard but
    /// cannot claim with enough confidence to name. Both become a new
    /// Speaker, honestly labelled as new rather than guessed at.
    New,
}

/// Cosine similarity. Zero for a zero vector rather than NaN, because a
/// silent cluster must not compare equal to everything.
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }
    let mut dot = 0.0;
    let mut norm_a = 0.0;
    let mut norm_b = 0.0;
    for (x, y) in a.iter().zip(b) {
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }
    if norm_a <= f32::EPSILON || norm_b <= f32::EPSILON {
        return 0.0;
    }
    dot / (norm_a.sqrt() * norm_b.sqrt())
}

/// Scales a vector to unit length. Leaves a zero vector alone.
pub fn l2_normalize(vector: &mut [f32]) {
    let norm: f32 = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm <= f32::EPSILON {
        return;
    }
    for value in vector {
        *value /= norm;
    }
}

/// Matches this Meeting's clusters against voices already in History.
///
/// Every candidate is scored against every cluster, then three conditions
/// have to hold at once. The mutual-best requirement is the one that is easy
/// to leave out and expensive to lose: without it, one distinctive voice can
/// be claimed by several clusters at once, and a meeting with three people
/// comes back attributed entirely to whichever of them the system knows best.
pub fn resolve(
    clusters: &BTreeMap<Cluster, Embedding>,
    seeds: &[SeedVoice],
) -> BTreeMap<Cluster, Resolved> {
    resolve_with(clusters, seeds, MATCH_FLOOR, MATCH_MARGIN)
}

/// [`resolve`] with the floor and margin named rather than taken from the
/// constants.
///
/// One implementation, so a harness that varies them exercises the rules
/// production runs rather than a copy that can drift from them. **Production
/// has no way to reach it:** `resolve` is what the pipeline calls and it
/// passes the constants.
pub fn resolve_with(
    clusters: &BTreeMap<Cluster, Embedding>,
    seeds: &[SeedVoice],
    floor: f32,
    margin: f32,
) -> BTreeMap<Cluster, Resolved> {
    let mut resolved = BTreeMap::new();
    if seeds.is_empty() {
        for cluster in clusters.keys() {
            resolved.insert(*cluster, Resolved::New);
        }
        return resolved;
    }

    // Every pairwise score once, **and only where the pair is in one space**.
    // Cheap — clusters are single digits and seeds are bounded — and it makes
    // the mutual-best check a lookup rather than a second pass over the
    // models.
    //
    // A Voiceprint from another model is not a candidate, however close the
    // numbers come out. [`cosine`] already refuses a dimension mismatch,
    // which is all that stood between the ReDimNet2 A/B and a silent
    // cross-space match: its vectors are 192 wide against WeSpeaker's 256,
    // so every such score was zero. Two models of the same width would have
    // scored, and the match would have looked like any other.
    //
    // The test is per pair rather than per batch. Taking the space from the
    // first cluster and filtering the seeds once would be right only if
    // every cluster in the call were in that space, which nothing here can
    // promise: a caller passing clusters from two passes would have had its
    // second pass scored against the first pass's seeds, which is the very
    // comparison this exists to refuse. An absent entry is a non-candidate
    // everywhere below, including in the mutual-best check, so a cluster in
    // another space cannot take a seed away from one that could have had it.
    let scores: BTreeMap<(Cluster, usize), f32> = clusters
        .iter()
        .flat_map(|(cluster, embedding)| {
            seeds
                .iter()
                .enumerate()
                .filter(move |(_, seed)| {
                    seed.model == embedding.model && seed.model_version == embedding.model_version
                })
                .map(move |(index, seed)| {
                    ((*cluster, index), cosine(&embedding.vector, &seed.vector))
                })
        })
        .collect();

    for cluster in clusters.keys() {
        let mut ranked: Vec<(usize, f32)> = seeds
            .iter()
            .enumerate()
            .filter_map(|(index, _)| Some((index, *scores.get(&(*cluster, index))?)))
            .collect();
        // Confirmed Voiceprints outrank unconfirmed ones at equal score
        // (ADR-0008 as amended): the Operator vouched for one of them.
        ranked.sort_by(|a, b| {
            b.1.total_cmp(&a.1)
                .then_with(|| seeds[b.0].confirmed.cmp(&seeds[a.0].confirmed))
        });

        let Some(&(best_index, best_score)) = ranked.first() else {
            resolved.insert(*cluster, Resolved::New);
            continue;
        };
        let runner_up = ranked.get(1).map(|(_, score)| *score).unwrap_or(0.0);

        let clears_floor = best_score >= floor;

        // The margin is what turns "too close to call" into no match rather
        // than a coin flip. ADR-0008 as amended names the one exception:
        // **confirmed Voiceprints win ties.** Acoustically a tie is a tie,
        // so the tie-break cannot come from the audio — it comes from the
        // Operator having vouched for one of these voices and not the other,
        // which is the strongest evidence this system ever receives.
        //
        // Scoped deliberately narrowly. If both candidates are confirmed, or
        // neither is, the Operator has said nothing that distinguishes them
        // and the margin rule stands.
        let confirmation_breaks_the_tie = ranked.get(1).is_some_and(|(runner_index, _)| {
            seeds[best_index].confirmed && !seeds[*runner_index].confirmed
        });
        let clears_margin = (best_score - runner_up) >= margin || confirmation_breaks_the_tie;
        let mutual = best_cluster_for(clusters, &scores, best_index) == Some(*cluster);

        resolved.insert(
            *cluster,
            if clears_floor && clears_margin && mutual {
                Resolved::Existing(seeds[best_index].speaker_id.clone())
            } else {
                Resolved::New
            },
        );
    }
    resolved
}

/// Which cluster this seed likes best — the other half of mutual-best.
fn best_cluster_for(
    clusters: &BTreeMap<Cluster, Embedding>,
    scores: &BTreeMap<(Cluster, usize), f32>,
    seed_index: usize,
) -> Option<Cluster> {
    clusters
        .keys()
        .filter(|cluster| scores.contains_key(&(**cluster, seed_index)))
        .max_by(|a, b| scores[&(**a, seed_index)].total_cmp(&scores[&(**b, seed_index)]))
        .copied()
}

/// Merges clusters that are the same voice.
///
/// **Properly agglomerative — closest pair first, repeatedly, against a
/// running centroid.** The first version was a single pass that joined each
/// cluster to the first earlier one within [`MERGE_THRESHOLD`], which is not
/// the same algorithm and does not give the same answer: it compares against
/// whichever member happened to come first rather than against the group, so
/// a voice whose short windows vary lands in two groups that are each
/// internally consistent and never get compared to each other.
///
/// That is not hypothetical. The close-out measured three speakers in a
/// two-speaker recording, and the two extra clusters were the *same* person
/// — which an Operator reads as a stranger in their own meeting. Merging by
/// centroid, closest pair first, is what the catalog specifies and it is
/// what fixed it.
pub fn agglomerate(embeddings: &BTreeMap<Cluster, Embedding>) -> BTreeMap<Cluster, Cluster> {
    agglomerate_with(embeddings, MERGE_THRESHOLD)
}

/// [`agglomerate`] with the merge threshold named rather than taken from the
/// constant.
///
/// The threshold is where a similarity distribution becomes a partition, and
/// different embeddings put their distributions in different places — so the
/// same number does not mean the same thing to two models, and comparing two
/// models at one number compares a configuration rather than the models. This
/// exists for a harness that sweeps it. **Production has no way to reach it:**
/// `agglomerate` is what the pipeline calls and it passes the constant.
pub fn agglomerate_with(
    embeddings: &BTreeMap<Cluster, Embedding>,
    threshold: f32,
) -> BTreeMap<Cluster, Cluster> {
    let groups: Vec<Group> = embeddings
        .iter()
        .map(|(cluster, embedding)| Group {
            members: vec![*cluster],
            centroid: embedding.vector.clone(),
            blocked: BTreeSet::new(),
        })
        .collect();

    agglomerate_groups(groups, threshold)
}

/// [`agglomerate_with`], plus pairs that must not end up in one cluster.
///
/// Two local speakers of one segmentation window are different people by
/// segmentation's own account — that is what made them two — and nothing in
/// the merge loop knows it. This exists to measure what the constraint is
/// worth, and **production has no way to reach it**: `agglomerate` passes
/// the constant and no constraints, as it always did.
///
/// `forbidden` maps each cluster to the clusters it may not join. It is
/// expected to be symmetric; anything asymmetric is silently one-directional
/// and would make the answer depend on merge order, so the harness that
/// builds it builds both directions.
///
/// **The constraints are hypotheses, not ground truth.** Segmentation
/// assigning two local tracks is a claim that can be wrong, and when it is,
/// this refuses a merge that should have happened. That cost is part of what
/// the experiment measures, which is why the pairs come from provenance
/// rather than from reference labels — a constraint read off the answer key
/// would be an oracle, and would measure nothing a product can ship.
pub fn agglomerate_constrained(
    embeddings: &BTreeMap<Cluster, Embedding>,
    threshold: f32,
    forbidden: &BTreeMap<Cluster, BTreeSet<Cluster>>,
) -> BTreeMap<Cluster, Cluster> {
    let groups: Vec<Group> = embeddings
        .iter()
        .map(|(cluster, embedding)| Group {
            members: vec![*cluster],
            centroid: embedding.vector.clone(),
            blocked: forbidden.get(cluster).cloned().unwrap_or_default(),
        })
        .collect();

    agglomerate_groups(groups, threshold)
}

/// The two stages, and the canonical naming, shared by both entry points.
fn agglomerate_groups(groups: Vec<Group>, threshold: f32) -> BTreeMap<Cluster, Cluster> {
    // One pass while the meeting is small enough, two when it is not. The
    // second stage runs over block centroids, of which there are a handful
    // per block, so it is never the expensive one.
    let merged = if groups.len() <= BLOCK {
        merge_closest_first(groups, threshold)
    } else {
        let blocked: Vec<Group> = groups
            .chunks(BLOCK)
            .flat_map(|block| merge_closest_first(block.to_vec(), threshold))
            .collect();
        merge_closest_first(blocked, threshold)
    };

    merged
        .into_iter()
        .flat_map(|group| {
            // The lowest id names the group, so the result does not depend
            // on the order groups happened to merge in — otherwise
            // "Speaker 1" and "Speaker 2" could swap between two runs over
            // the same audio, and an Operator who named one has named the
            // other.
            let canonical = group
                .members
                .iter()
                .copied()
                .min()
                .expect("a non-empty group");
            group
                .members
                .into_iter()
                .map(move |member| (member, canonical))
        })
        .collect()
}

/// How many windows one block of the first stage holds.
///
/// Blocks are consecutive in cluster-id order, which is time order, so a
/// block is a stretch of the meeting. The same voice appears in many of them
/// and the second stage is what puts those back together.
const BLOCK: usize = 2_000;

/// One group on its way to becoming a speaker.
#[derive(Clone)]
struct Group {
    members: Vec<Cluster>,
    centroid: Vec<f32>,
    /// Clusters this group may never merge with, unioned as it grows.
    ///
    /// Empty for every production path. Carrying it on the group rather
    /// than consulting a pair table is what makes the constraint survive
    /// merging: once two groups are one, the survivor inherits both sets,
    /// so a pair that was never compared directly is still refused when
    /// its members end up on opposite sides of a later candidate merge.
    blocked: BTreeSet<Cluster>,
}

/// Whether a constraint forbids merging these two rows.
///
/// Symmetric by construction — both groups carry the pair — so one
/// direction is enough. `owner` is the current row of each cluster, which
/// turns "does the group over there contain any cluster I am blocked from"
/// into one lookup per blocked entry instead of a scan of its members.
fn forbidden(
    groups: &[Group],
    owner: &BTreeMap<Cluster, usize>,
    left: usize,
    right: usize,
) -> bool {
    groups[left]
        .blocked
        .iter()
        .any(|cluster| owner.get(cluster) == Some(&right))
}

/// Closest pair first, repeatedly, against a running centroid.
///
/// The naive form of this — rescan every pair, merge the best, repeat — is
/// cubic, and it is what the close-out measured at 70 s on a 1,259-window
/// meeting, projecting about a quarter of an hour for two hours of audio.
/// Sliding the segmentation window at one second instead of hopping it by
/// ten multiplies the windows by roughly ten, and ten times the windows
/// through a cubic clusterer is not a slower product but an unusable one.
///
/// So the scores are computed once and kept, and each group remembers its
/// own best partner. Choosing the global best is then a scan of those
/// rather than of every pair, and a merge only has to recompute the row
/// that changed plus the rows that were pointing at the two rows it
/// replaced.
///
/// ponytail: the score matrix is n² floats, which is why `BLOCK` exists at
/// all — 2,000 groups is 16 MB and bounded, where a two-hour meeting's
/// 12,000 would be 576 MB. If blocks ever need to be much larger, the
/// matrix is the thing to replace, with a nearest-neighbour chain.
fn merge_closest_first(mut groups: Vec<Group>, threshold: f32) -> Vec<Group> {
    let count = groups.len();
    if count < 2 {
        return groups;
    }

    // Where each cluster currently lives. Both stages run through here, so
    // maintaining it here is what makes constraints hold in the second one
    // as well as the first.
    let mut owner: BTreeMap<Cluster, usize> = BTreeMap::new();
    for (row, group) in groups.iter().enumerate() {
        for member in &group.members {
            owner.insert(*member, row);
        }
    }

    let mut score = vec![f32::NEG_INFINITY; count * count];
    for left in 0..count {
        for right in (left + 1)..count {
            if forbidden(&groups, &owner, left, right) {
                continue;
            }
            let value = cosine(&groups[left].centroid, &groups[right].centroid);
            score[left * count + right] = value;
            score[right * count + left] = value;
        }
    }

    let mut alive = vec![true; count];
    let mut best: Vec<(usize, f32)> = (0..count)
        .map(|row| best_partner(row, count, &score, &alive))
        .collect();

    loop {
        // The best of every group's best is the best pair there is: if some
        // other pair scored higher, one of its members would be pointing at
        // the other.
        let mut pick: Option<(usize, usize, f32)> = None;
        for row in 0..count {
            if !alive[row] {
                continue;
            }
            let (partner, value) = best[row];
            if value >= threshold && pick.is_none_or(|(_, _, previous)| value > previous) {
                pick = Some((row, partner, value));
            }
        }
        let Some((first, second, _)) = pick else {
            break;
        };
        let (keep, drop) = (first.min(second), first.max(second));

        // Merge into the earlier group and recentre. The centroid is what
        // makes this stable: comparing against a single member is how a
        // group drifts apart one window at a time.
        let taken = std::mem::take(&mut groups[drop].members);
        let vector = std::mem::take(&mut groups[drop].centroid);
        let inherited = std::mem::take(&mut groups[drop].blocked);
        let weight = groups[keep].members.len() as f32;
        let total = weight + taken.len() as f32;
        for (slot, value) in groups[keep].centroid.iter_mut().zip(vector.iter()) {
            *slot = (*slot * weight + value * taken.len() as f32) / total;
        }
        l2_normalize(&mut groups[keep].centroid);
        for member in &taken {
            owner.insert(*member, keep);
        }
        groups[keep].members.extend(taken);
        groups[keep].blocked.extend(inherited);
        alive[drop] = false;

        for other in 0..count {
            if !alive[other] || other == keep {
                continue;
            }
            // The survivor inherited the constraints of both, so a pair that
            // was merely far apart a moment ago can be forbidden now.
            let value = if forbidden(&groups, &owner, keep, other) {
                f32::NEG_INFINITY
            } else {
                cosine(&groups[keep].centroid, &groups[other].centroid)
            };
            score[keep * count + other] = value;
            score[other * count + keep] = value;
        }
        // Anyone whose best was one of the two rows that just changed has to
        // look again; everyone else's answer is still true.
        for other in 0..count {
            if alive[other] && (other == keep || best[other].0 == keep || best[other].0 == drop) {
                best[other] = best_partner(other, count, &score, &alive);
            }
        }
    }

    groups
        .into_iter()
        .zip(alive)
        .filter_map(|(group, living)| living.then_some(group))
        .collect()
}

/// The living group this one is closest to, and how close.
fn best_partner(row: usize, count: usize, score: &[f32], alive: &[bool]) -> (usize, f32) {
    let mut best = (row, f32::NEG_INFINITY);
    for other in 0..count {
        if other == row || !alive[other] {
            continue;
        }
        let value = score[row * count + other];
        if value > best.1 {
            best = (other, value);
        }
    }
    best
}

/// Which of a Speaker's exemplars a Voiceprint is built from, given each
/// one's Meeting in the order the store returns them — oldest first.
///
/// Round-robin across the Meetings that contributed, newest Meeting first and
/// newest-first within each, until [`MAX_EXEMPLARS`]. Returns indices into
/// the slice it was handed.
///
/// The cap exists to bound cost — a Speaker seen in two hundred Meetings must
/// not carry two hundred vectors into every later clustering run — and taking
/// the newest 32 was one way to spend it, not a considered one. Because
/// exemplar ids are UUIDv7 minted at insert, "newest" was a *contiguous tail*:
/// the end of the last Meeting that contributed any. On a real History that
/// gave Speakers heard in seven Meetings an identity decided by one of them,
/// and gave one whose last Meeting ran under another voice an identity that
/// matched the wrong person outright.
///
/// Any 32 cost the same downstream, so this spends the budget across the
/// Meetings instead. Recency survives as the ordering rather than the rule:
/// the newest Meeting is served first and the newest exemplar within each
/// Meeting is taken first, so a voice that has changed is still represented
/// by how it sounds now — it just no longer crowds out every other occasion
/// the Speaker was heard.
///
/// Exemplars with no Meeting share one bucket: there is nothing to spread
/// them across.
pub(super) fn spread_across_meetings(meetings: &[Option<&str>]) -> Vec<usize> {
    // First appearance walking backwards orders the buckets newest-Meeting
    // first, and fills each one newest-first, which is the whole of how
    // recency survives here.
    let mut buckets: Vec<(Option<&str>, Vec<usize>)> = Vec::new();
    for (index, meeting) in meetings.iter().enumerate().rev() {
        match buckets.iter_mut().find(|(key, _)| key == meeting) {
            Some((_, rows)) => rows.push(index),
            None => buckets.push((*meeting, vec![index])),
        }
    }

    let mut chosen = Vec::with_capacity(MAX_EXEMPLARS.min(meetings.len()));
    let mut depth = 0;
    while chosen.len() < MAX_EXEMPLARS {
        let before = chosen.len();
        for (_, rows) in &buckets {
            let Some(&index) = rows.get(depth) else {
                continue;
            };
            chosen.push(index);
            if chosen.len() == MAX_EXEMPLARS {
                break;
            }
        }
        // Every bucket is shorter than `depth`: there is nothing left to take.
        if chosen.len() == before {
            break;
        }
        depth += 1;
    }
    chosen
}

/// The average of a Speaker's exemplars, weighted by how much voiced audio
/// each came from, L2-normalized.
///
/// Weighted because a Voiceprint built equally from eight seconds and from
/// half a second would let the shortest, noisiest observation pull identity
/// around. Negative exemplars are excluded: they record that a voice is
/// *not* this Speaker, and averaging them in would move the centroid toward
/// the person it was supposed to distinguish.
pub fn centroid(exemplars: &[(Vec<f32>, i64, bool)]) -> Option<Vec<f32>> {
    let usable: Vec<&(Vec<f32>, i64, bool)> = exemplars
        .iter()
        .filter(|(vector, _, is_negative)| !is_negative && !vector.is_empty())
        .rev()
        .take(MAX_EXEMPLARS)
        .collect();
    let width = usable.first()?.0.len();

    let mut sum = vec![0.0_f32; width];
    let mut total_weight = 0.0_f32;
    for (vector, voiced_ms, _) in &usable {
        if vector.len() != width {
            continue;
        }
        let weight = (*voiced_ms).max(1) as f32;
        for (slot, value) in sum.iter_mut().zip(vector.iter()) {
            *slot += value * weight;
        }
        total_weight += weight;
    }
    if total_weight <= f32::EPSILON {
        return None;
    }
    for slot in sum.iter_mut() {
        *slot /= total_weight;
    }
    l2_normalize(&mut sum);
    Some(sum)
}

// ---- Where clustering meets the record ----

/// Every voice History can offer this Meeting's clusterer, in the space
/// its embeddings are in.
pub fn seeds(
    connection: &rusqlite::Connection,
    model: &str,
    model_version: &str,
) -> anyhow::Result<Vec<SeedVoice>> {
    Ok(
        crate::store::speakers::voiceprints(connection, model, model_version)?
            .into_iter()
            .map(|(speaker_id, vector, confirmed)| SeedVoice {
                speaker_id,
                vector,
                confirmed,
                model: model.to_string(),
                model_version: model_version.to_string(),
            })
            .collect(),
    )
}

/// Recomputes a Speaker's Voiceprint from the evidence it still holds, or
/// clears it when the evidence cannot produce one.
///
/// The clearing is what makes a withdrawn exemplar actually withdrawn:
/// [`seeds`] reads the column, not the rows, and a Speaker whose every
/// exemplar is gone but whose vector stayed would go on recognizing itself.
///
/// Only evidence in the newest exemplar's space enters the average: two
/// spaces averaged together is a vector in neither, and the column's model
/// columns would say otherwise.
///
/// **It clears; it never forgets.** Forgetting is the Operator's act and
/// [`speakers::delete_voiceprint`] is the only thing that performs it —
/// which is exactly why a recomputation must not call it. This used to, for
/// the case where no evidence is left, and the mark it set is the one
/// [`speakers::relearnable`] consults: a Speaker that simply ran out of
/// evidence would have been recorded as one the Operator deliberately
/// forgot, excluded from every future re-run, and shown in the Registry as
/// deleted by them. That call also removed *every* exemplar the Speaker
/// had, from every Meeting — harmless only because it ran when there were
/// none. [`speakers::clear_voiceprint`] is the half a recomputation wants,
/// and it exists for this.
///
/// **Evidence that is all negative clears too.** It used to be left alone,
/// on the reasoning that a Speaker with nothing but negatives had nothing to
/// recompute *from*. But it has something to recompute *away*: correcting a
/// Speaker's last positive segment to somebody else leaves the old vector
/// standing, still offered to every future match, with nothing behind it
/// that agrees. The stored negative does not prevent that by itself, and
/// saying so would be a claim this code does not back: [`centroid`] filters
/// negatives out, [`seeds`] reads the positive Voiceprint column, and
/// nothing else scores against one either. **The vector going is the whole
/// of what withdraws the recognition.** The negative exemplar is left where
/// it is — it is the record of the correction (ADR-0009 as amended), and a
/// recomputation is not the place to decide it has served its purpose.
pub(super) fn refresh_voiceprint(
    connection: &rusqlite::Connection,
    speaker_id: &str,
) -> anyhow::Result<()> {
    use crate::store::speakers;

    let evidence = speakers::exemplars(connection, speaker_id)?;
    let space = evidence
        .last()
        .map(|latest| (latest.model.clone(), latest.model_version.clone()));
    // Only what could contribute: the newest model space, positives, and a
    // vector that is actually there. The selection spends the cap, so it has
    // to see what the cap will be spent on — an exemplar `centroid` would
    // drop anyway must not consume one of the slots first.
    let usable: Vec<&speakers::Exemplar> = evidence
        .iter()
        .filter(|exemplar| {
            !exemplar.is_negative
                && !exemplar.vector.is_empty()
                && space.as_ref().is_some_and(|(model, version)| {
                    exemplar.model == *model && exemplar.model_version == *version
                })
        })
        .collect();
    let meetings: Vec<Option<&str>> = usable
        .iter()
        .map(|exemplar| exemplar.meeting_id.as_deref())
        .collect();
    let history: Vec<(Vec<f32>, i64, bool)> = spread_across_meetings(&meetings)
        .into_iter()
        .map(|index| {
            let exemplar = usable[index];
            (
                exemplar.vector.clone(),
                exemplar.voiced_ms,
                exemplar.is_negative,
            )
        })
        .collect();
    match (centroid(&history), evidence.last()) {
        (Some(vector), Some(latest)) => speakers::set_voiceprint(
            connection,
            speaker_id,
            &vector,
            &latest.model,
            &latest.model_version,
        ),
        // No evidence at all, or none that yields a centroid. Either way
        // there is no vector this Speaker's record supports, and the columns
        // that claim one are cleared — without touching the name, the
        // exemplars, or the forgetting mark.
        _ => speakers::clear_voiceprint(connection, speaker_id).map(|_| ()),
    }
}

/// Resolves a Meeting's clusters to persistent Speakers.
///
/// Recognized clusters return their existing Speaker; unrecognized ones get
/// a new Speaker with no name, which is what "every voice resolves to a
/// persistent Speaker, named or not" means in ADR-0008. Either way the new
/// observation is folded back in as an exemplar and the Voiceprint is
/// recomputed, so the next Meeting's clusterer is seeded with a slightly
/// better picture than this one was — the improvement ADR-0008 promises.
///
/// **Two gates before a cluster is anybody**, and they are where "every
/// voice" stops meaning "every group of windows". `heard` is the set of
/// clusters that own at least one transcript segment: a cluster that owns
/// none has nothing to be attributed to and nothing to be recognized in,
/// so it leaves no Speaker and no evidence. And a cluster nobody in History
/// recognizes must hold [`MIN_SPEAKER_MS`] of voice before it is minted.
/// Recognition itself has no floor — the conservative match rule is the
/// guard there, and "what did Alice say" should work for one sentence.
///
/// `withheld` names a Speaker whose Voiceprint this run may not see at all.
///
/// **A re-run replaces the run.** What the previous run of this Meeting
/// taught History about anonymous Speakers is withdrawn before the seeds
/// are read, and each Speaker this run recognizes has its earlier hearing
/// from this Meeting replaced rather than doubled. The other half — the
/// Speakers the previous run minted that this one did not re-attribute —
/// can only go once the segments have moved, so the caller runs
/// [`crate::store::speakers::sweep_unreferenced`] after `apply`.
/// What a bulk re-run has already settled about this Meeting.
///
/// Ticket 12. Empty for every ordinary run, which is every run today, and
/// [`persist_with`] then behaves exactly as it did before this existed.
///
/// It exists because the two halves of the lifecycle disagree otherwise.
/// Re-seeding rebuilds a named Speaker's evidence for this Meeting as
/// vectors cut from **exactly the ranges the record attributes to them**,
/// and files them as the machine's — which is what they are, since the
/// machine attributed most of those segments. [`persist_with`] then deletes
/// this Meeting's machine exemplars for every Speaker it resolves and
/// installs the whole cluster's centroid instead. Left alone, the second
/// step erases the first and replaces bounded evidence with a vector
/// `live::assemble` built over every grouped observation — audio no
/// transcript segment covers, under a name the Operator trusts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rebuilt {
    /// Clusters the standing attribution already names, from
    /// [`claims`]. **Assignment only.** The cluster is attributed to that
    /// Speaker without being resolved and without its centroid being
    /// enrolled: unanimity among a cluster's segments says who those
    /// segments belong to, never that the rest of the vector is theirs. It
    /// leaves [`resolve_with`] entirely rather than being overridden after
    /// it, so it cannot take a seed from a cluster that had to earn one, and
    /// it is honoured whether or not the cluster has an embedding at all.
    pub claimed: BTreeMap<Cluster, String>,
    /// Speakers whose evidence for this Meeting was rebuilt from bounded
    /// ranges in this same transaction. Their exemplars here are left where
    /// they are and no centroid joins them.
    pub reseeded: BTreeSet<String>,
}

pub fn persist(
    connection: &rusqlite::Connection,
    meeting_id: &str,
    embeddings: &BTreeMap<Cluster, Embedding>,
    heard: &BTreeSet<Cluster>,
    withheld: Option<&str>,
    rebuilt: &Rebuilt,
) -> anyhow::Result<BTreeMap<Cluster, String>> {
    persist_with(
        connection,
        meeting_id,
        embeddings,
        heard,
        withheld,
        rebuilt,
        MATCH_FLOOR,
        MATCH_MARGIN,
    )
}

/// [`persist`] with the matching thresholds named rather than taken from the
/// constants.
///
/// The whole lifecycle, unchanged — the withdrawal of the previous run's
/// exemplars, the mint floor, the sweep the caller owes afterwards — with
/// only the two numbers [`resolve_with`] applies coming from the caller.
/// **Production has no way to reach it:** `persist` is what the pipeline
/// calls and it passes the constants.
// Eight, because the last two exist only so the harness can sweep thresholds
// production reads from constants. Bundling them would make `persist`'s own
// call read worse to satisfy a count.
#[allow(clippy::too_many_arguments)]
pub fn persist_with(
    connection: &rusqlite::Connection,
    meeting_id: &str,
    embeddings: &BTreeMap<Cluster, Embedding>,
    heard: &BTreeSet<Cluster>,
    withheld: Option<&str>,
    rebuilt: &Rebuilt,
    floor: f32,
    margin: f32,
) -> anyhow::Result<BTreeMap<Cluster, String>> {
    use crate::store::speakers;

    // The first real re-runs showed why this comes first: the previous
    // run's Voiceprints were cut from the very audio being re-diarized,
    // so every one of them was recognized from it — twenty-four of
    // twenty-four in one Meeting — and the floor below never got a say.
    // A Speaker the Operator named or confirmed keeps its evidence: that
    // is the Operator's word about the voice, not the machine's guess.
    for speaker_id in speakers::anonymous_speakers_heard_in(connection, meeting_id)? {
        speakers::delete_machine_exemplars(connection, &speaker_id, meeting_id)?;
        refresh_voiceprint(connection, &speaker_id)?;
    }

    // Seeds from the space these embeddings are in, and none when there
    // are no embeddings to resolve.
    let mut known = match embeddings.values().next() {
        Some(embedding) => seeds(connection, &embedding.model, &embedding.model_version)?,
        None => Vec::new(),
    };
    // One Speaker may be held out of the whole resolve, not merely out of
    // the answer it was being asked about. ADR-0029's third rule gates the
    // Operator's Voiceprint on the Meeting being big enough for a match to
    // mean something, and a gate applied only to the Operator flag would be
    // decorative: the seed would still sit here among every other voice and
    // match, and the cluster would simply be attributed to "You" without the
    // flag being reconsidered.
    if let Some(withheld) = withheld {
        known.retain(|seed| seed.speaker_id != withheld);
    }
    // A claim is the Operator's standing word about whose segments these are,
    // and it is settled **before** the resolve rather than layered over it.
    //
    // Two things go wrong when a claim is an override applied afterwards.
    // The claimed cluster is still scored, so it can be the argmax for
    // somebody else's seed and take mutual-best away from the cluster that
    // seed belongs to — an unclaimed Bob fragment is refused because the
    // cluster the record already calls Alice looks more like Bob, and is then
    // reassigned to Alice anyway. Nobody is recognized as Bob and nothing
    // says why. And the override sits behind `embeddings.get`, so a claim is
    // lost whenever its cluster has no embedding entry — which `assemble`
    // leaves out when a centroid is unavailable, while still keeping the
    // turns. A claim assigns words. It neither needs a vector nor licenses
    // one, so it is applied from `heard` alone and its cluster leaves the
    // matching entirely.
    let mut assigned: BTreeMap<Cluster, String> = rebuilt
        .claimed
        .iter()
        .filter(|(cluster, _)| heard.contains(cluster))
        .map(|(cluster, speaker_id)| (*cluster, speaker_id.clone()))
        .collect();
    let unclaimed: BTreeMap<Cluster, Embedding> = embeddings
        .iter()
        .filter(|(cluster, _)| !rebuilt.claimed.contains_key(*cluster))
        .map(|(cluster, embedding)| (*cluster, embedding.clone()))
        .collect();
    let resolved = resolve_with(&unclaimed, &known, floor, margin);

    for (cluster, outcome) in resolved {
        let Some(embedding) = embeddings.get(&cluster) else {
            continue;
        };
        if !heard.contains(&cluster) {
            continue;
        }
        let speaker_id = match outcome {
            Resolved::Existing(id) => id,
            Resolved::New if embedding.voiced_ms < MIN_SPEAKER_MS => continue,
            Resolved::New => speakers::create(connection, false)?.id,
        };

        // Their evidence for this Meeting was rebuilt from the ranges the
        // record attributes to them, minutes ago and in this transaction.
        // Deleting it and installing the centroid instead would undo that and
        // put unvouched audio in its place.
        if rebuilt.reseeded.contains(&speaker_id) {
            assigned.insert(cluster, speaker_id);
            continue;
        }

        // This run's hearing replaces the previous run's, never sits beside
        // it: a named Speaker recognized from its own audio would otherwise
        // weigh that audio twice in its Voiceprint.
        speakers::delete_machine_exemplars(connection, &speaker_id, meeting_id)?;
        speakers::add_exemplar(
            connection,
            speakers::NewExemplar {
                speaker_id: &speaker_id,
                meeting_id: Some(meeting_id),
                vector: &embedding.vector,
                model: &embedding.model,
                model_version: &embedding.model_version,
                voiced_ms: embedding.voiced_ms as i64,
                from_operator: false,
                is_negative: false,
                sample: embedding.sample.map(|window| speakers::Sample {
                    channel: window.channel,
                    start_ms: window.start.millis() as i64,
                    end_ms: window.end.millis() as i64,
                }),
            },
        )?;
        refresh_voiceprint(connection, &speaker_id)?;
        assigned.insert(cluster, speaker_id);
    }
    Ok(assigned)
}

/// What the Operator has already said about the voices in this Meeting.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Claims {
    /// Clusters this run may not guess about: every segment in them already
    /// belonged to the same Speaker, so the Operator has already said whose
    /// voice it is. Assigned rather than resolved.
    pub claimed: BTreeMap<Cluster, String>,
    /// Speakers a correction took every one of a cluster's *transcript
    /// segments* away from. Evidence of what the Operator denied, for a
    /// writer that can bound a vector to those segments — see the
    /// provenance note on [`claims`]. Nothing writes it today.
    pub denied: BTreeSet<(String, Cluster)>,
}

/// Reads the Operator's word about this Meeting before this run overwrites it.
///
/// Ticket 12. **Must be called before [`super::reconcile::apply`].** What it
/// reads is the attribution standing on the segments *now* — the previous
/// model's conclusion with the Operator's corrections on top — and `apply`
/// replaces exactly that. After ticket 05's wipe there is no vector left
/// anywhere, so this is the only surviving record of who these voices were;
/// reading it late reads what this run has just guessed.
///
/// **A cluster is claimed only where every one of its segments belongs to
/// the same eligible Speaker.** Ticket 12 seeds a named Speaker from *their
/// own* attributed segments; naming a cluster the old model drew was never
/// confirmation of every voice in a new, differently drawn one. A re-run
/// redraws the clusters, so one can arrive holding two people's words, or
/// one person's mixed with audio nobody has vouched for — and claiming that
/// whole would enroll the unvouched part under a name the Operator trusts.
/// Conflicting or unsupported ownership therefore yields no claim at all
/// rather than a winner.
///
/// Abstaining here costs the shortcut, not the person: the seeding path can
/// still rebuild that Speaker from the ranges that *are* theirs, and the
/// resolve still has their Voiceprint to match against. What an absent claim
/// withholds is the right to skip the resolve for a cluster whose ownership
/// nobody has established.
///
/// The test is over the *set* of owners, never a count, so splitting one
/// utterance into more segments cannot change who claims it. A tally or a
/// coverage percentage would make transcription granularity an input to
/// identity, and a tie between two names would be broken by comparing two
/// UUIDs, which mean nothing about whose voice it is.
///
/// **The Operator is included**, like every other named Speaker, since
/// 2026-09-17 (DECISIONS Q237). They were excluded until then, on the
/// reasoning that ADR-0029's three channel rules rebuild the Operator by
/// themselves so this path need not. The first real re-run against a
/// populated History showed the reasoning does not close: the third of those
/// three rules **is** the Voiceprint match, so a rule needing a Voiceprint
/// that nothing rebuilds is a rule that cannot fire after a model change. It
/// cost the Operator their name on the four most recent Meetings (Q234,
/// Q236). The boundary that was actually load-bearing stays, and it is the
/// one the rest of this doc applies to everybody: what is read here is which
/// Speaker *the record attributes* a segment to, never a cluster vector, so
/// the Operator is relearned from their own words and not from a previous
/// model's guess at which cluster was theirs.
///
/// Corrections outrank the machine because
/// [`crate::store::speakers::attributed_speaker`] is the display join: the
/// newest hint wins, and the machine shows through only where nobody
/// disagreed. Reading `speaker_id` directly here would treat a correction
/// the Operator made as though it had been ignored.
///
/// A Speaker the Operator forgot claims nothing — that is the whole reason
/// the mark exists (ADR-0009, ticket 09) — and neither does a pseudonym,
/// which is re-derived rather than relearned.
///
/// **The other half of a correction is what it denied**, and it is held to
/// the same standard: a cluster is denied to a Speaker only where *every*
/// one of its segments was corrected away from them. One corrected segment
/// in thirty denies nothing — the other twenty-nine are audio the Operator
/// never disputed.
///
/// # This is evidence about attribution, not permission to enrol a centroid
///
/// **Neither half licenses writing this run's cluster vector as an exemplar,
/// positive or negative.** The two are about different things.
/// [`super::live::assemble`] builds each cluster's vector with [`centroid`]
/// over every grouped `Observation`, before reconciliation has run; a
/// transcript segment is mapped to a cluster only afterwards. So a cluster's
/// vector can carry speech no segment covers at all, and the parts of each
/// observation window that fall outside the segments over it. Unanimity here
/// is unanimity among the segments — it cannot speak for the rest of what
/// went into the vector.
///
/// A writer wanting to enrol or suppress a voice from this needs an
/// embedding **bounded to the ranges the claim actually covers**, which this
/// API does not carry and this module does not compute. Until it exists, a
/// claim is a fact about who owned some words, to be used by a path that
/// re-embeds those words.
pub fn claims(
    connection: &rusqlite::Connection,
    reconciliation: &super::reconcile::Reconciliation,
) -> anyhow::Result<Claims> {
    use crate::store::speakers;

    let eligible: BTreeSet<String> = speakers::relearnable(connection)?
        .into_iter()
        .map(|speaker| speaker.id)
        .collect();
    if eligible.is_empty() {
        return Ok(Claims::default());
    }

    // Per cluster, the distinct Speakers its segments point at in each
    // direction. `None` is a segment that supports nobody — unattributed, or
    // attributed to somebody a re-run may not relearn — and it is *kept* in
    // the set rather than skipped, because it is the whole difference
    // between "all of this is Alice's" and "some of this is Alice's".
    let mut owners: BTreeMap<Cluster, BTreeSet<Option<String>>> = BTreeMap::new();
    let mut taken_from: BTreeMap<Cluster, BTreeSet<Option<String>>> = BTreeMap::new();
    for assignment in &reconciliation.assignments {
        // Silence is in no cluster, so it supports and denies nothing.
        let Some(cluster) = assignment.cluster else {
            continue;
        };
        let owner = speakers::attributed_speaker(connection, &assignment.segment_id)?
            .filter(|id| eligible.contains(id));
        owners.entry(cluster).or_default().insert(owner);
        let taken = speakers::replaced_speaker(connection, &assignment.segment_id)?
            .filter(|id| eligible.contains(id));
        taken_from.entry(cluster).or_default().insert(taken);
    }

    // Unanimity or nothing: one entry in the set, and that entry names
    // somebody. A cluster with no segments and a cluster nobody vouched for
    // both fall out here without needing a guard of their own.
    let agreed = |who: &BTreeSet<Option<String>>| -> Option<String> {
        if who.len() == 1 {
            who.iter().next().cloned().flatten()
        } else {
            None
        }
    };

    let claimed: BTreeMap<Cluster, String> = owners
        .iter()
        .filter_map(|(cluster, who)| agreed(who).map(|speaker| (*cluster, speaker)))
        .collect();
    let mut denied: BTreeSet<(String, Cluster)> = taken_from
        .iter()
        .filter_map(|(cluster, who)| agreed(who).map(|speaker| (speaker, *cluster)))
        .collect();
    // A Speaker who still owns a cluster is not also denied it: the
    // Operator's latest word about those words is that they are theirs.
    denied.retain(|(speaker, cluster)| claimed.get(cluster) != Some(speaker));

    Ok(Claims { claimed, denied })
}

/// What rebuilding stale evidence did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Adopted {
    /// Exemplars re-embedded from their kept audio.
    pub rebuilt: usize,
    /// Exemplars with no audio to rebuild from, removed.
    pub dropped: usize,
    /// Speakers whose Voiceprint was recomputed.
    pub speakers: usize,
}

/// Moves History into the embedding space now in use.
///
/// `rebuilt` pairs each stale exemplar's id with its vector re-embedded
/// from the kept audio, or `None` where there was none to re-embed from:
/// a row written before samples were kept, or whose Meeting is gone. Those
/// rows go, honestly — a Speaker's Voiceprint is recomputed from what can
/// still be heard, and a Speaker nothing can be heard of loses its
/// Voiceprint and is a stranger next time, as ADR-0009 says a deleted one
/// is. Its name and its words stay.
///
/// This is what ADR-0035 kept `model`/`model_version` on every row for:
/// "a model upgrade re-embeds cleanly from kept audio". The alternative —
/// leaving old vectors in place and letting `cosine` compare them — is
/// what the first front-end fix would have done silently, and every
/// existing Speaker would have stopped being recognized without a word.
pub fn adopt_rebuilt(
    connection: &rusqlite::Connection,
    rebuilt: &[(String, Option<Vec<f32>>)],
    model: &str,
    model_version: &str,
) -> anyhow::Result<Adopted> {
    use crate::store::speakers;

    let mut adopted = Adopted::default();
    let mut touched = BTreeSet::new();
    let stale = speakers::stale_exemplars(connection, model, model_version)?;
    let owner: BTreeMap<&str, &str> = stale
        .iter()
        .map(|exemplar| (exemplar.id.as_str(), exemplar.speaker_id.as_str()))
        .collect();
    for (id, vector) in rebuilt {
        let Some(speaker_id) = owner.get(id.as_str()) else {
            continue;
        };
        match vector {
            Some(vector) => {
                speakers::replace_exemplar_vector(connection, id, vector, model, model_version)?;
                adopted.rebuilt += 1;
            }
            None => {
                speakers::delete_exemplar(connection, id)?;
                adopted.dropped += 1;
            }
        }
        touched.insert(speaker_id.to_string());
    }
    // A Voiceprint from the old space with nothing behind it is stale too.
    touched.extend(speakers::speakers_with_stale_voiceprint(
        connection,
        model,
        model_version,
    )?);
    for speaker_id in &touched {
        refresh_voiceprint(connection, speaker_id)?;
    }
    // What the evidence could not recompute — a Speaker left with only
    // negative exemplars — must not stay as a vector in the old space
    // either: nothing would offer it, and every run would find it stale.
    for speaker_id in speakers::speakers_with_stale_voiceprint(connection, model, model_version)? {
        speakers::clear_voiceprint(connection, &speaker_id)?;
    }
    adopted.speakers = touched.len();
    Ok(adopted)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Long enough to be minted: the floor is a separate test's subject.
    fn embedding(vector: &[f32]) -> Embedding {
        Embedding::new(vector.to_vec(), "test", "1", 30_000)
    }

    fn clusters(entries: &[(u32, &[f32])]) -> BTreeMap<Cluster, Embedding> {
        entries
            .iter()
            .map(|(index, vector)| (Cluster(*index), embedding(vector)))
            .collect()
    }

    /// Every cluster owns words, which is the ordinary case the persistence
    /// tests below are about.
    fn heard(clusters: &BTreeMap<Cluster, Embedding>) -> BTreeSet<Cluster> {
        clusters.keys().copied().collect()
    }

    fn seed(id: &str, vector: &[f32], confirmed: bool) -> SeedVoice {
        SeedVoice {
            speaker_id: id.into(),
            vector: vector.to_vec(),
            confirmed,
            model: "test".into(),
            model_version: "1".into(),
        }
    }

    /// Same width, same numbers, different model: still a stranger.
    ///
    /// `cosine` refuses a dimension mismatch, so two models of different
    /// widths could never have matched by accident. Two of the *same* width
    /// would have, and a version bump is exactly that case — the front-end
    /// fix behind version 2 left vectors of the same 256 dimensions
    /// agreeing with version 1's at cosine 0.36 (DECISIONS Q115). Nothing
    /// in the numbers says which space a vector is in; only the label does.
    #[test]
    fn a_voiceprint_from_another_model_is_never_a_match() {
        let this_meeting = clusters(&[(0, &[1.0, 0.0, 0.0])]);
        let mut elsewhere = seed("alice", &[1.0, 0.0, 0.0], true);
        elsewhere.model = "some-other-model".into();

        // The control: the identical seed in this space is recognized, so
        // the refusal below is about the label and nothing else.
        assert_eq!(
            resolve(&this_meeting, &[seed("alice", &[1.0, 0.0, 0.0], true)])[&Cluster(0)],
            Resolved::Existing("alice".into())
        );
        assert_eq!(
            resolve(&this_meeting, &[elsewhere])[&Cluster(0)],
            Resolved::New
        );
    }

    /// The batch is not assumed homogeneous, because nothing makes it so.
    ///
    /// The first version of this guard read the space off
    /// `clusters.values().next()` and filtered the seeds once. That is right
    /// only if every cluster in the call is in that space — and a caller that
    /// mixed two passes would have had its second pass scored against the
    /// first pass's seeds, the exact comparison the guard exists to refuse.
    /// Both mixtures are here because model and version are separate columns
    /// and a check on one is not a check on the other.
    #[test]
    fn a_mixed_batch_does_not_let_the_first_clusters_space_speak_for_the_rest() {
        let with_space = |vector: &[f32], model: &str, version: &str| {
            Embedding::new(vector.to_vec(), model, version, 30_000)
        };
        let alice = seed("alice", &[1.0, 0.0], true);

        for (label, elsewhere) in [
            ("another model", with_space(&[1.0, 0.0], "other", "1")),
            ("another version", with_space(&[1.0, 0.0], "test", "9")),
        ] {
            // Cluster 0 is in the seed's space and is the same voice;
            // cluster 1 is an identical vector from somewhere else.
            let mixed: BTreeMap<Cluster, Embedding> = [
                (Cluster(0), with_space(&[1.0, 0.0], "test", "1")),
                (Cluster(1), elsewhere),
            ]
            .into_iter()
            .collect();
            let resolved = resolve(&mixed, std::slice::from_ref(&alice));
            assert_eq!(
                resolved[&Cluster(1)],
                Resolved::New,
                "{label}: scored against a seed it shares no space with"
            );
            // And the out-of-space cluster must not have taken the seed
            // away from the cluster that could legitimately have it: it is
            // not a candidate anywhere, including in the mutual-best check.
            assert_eq!(
                resolved[&Cluster(0)],
                Resolved::Existing("alice".into()),
                "{label}: the in-space cluster lost its own match to it"
            );
        }
    }

    /// A version bump is a different space, and reads as one.
    #[test]
    fn a_voiceprint_from_another_version_is_never_a_match() {
        let this_meeting = clusters(&[(0, &[1.0, 0.0, 0.0])]);
        let mut older = seed("alice", &[1.0, 0.0, 0.0], true);
        older.model_version = "0".into();
        assert_eq!(resolve(&this_meeting, &[older])[&Cluster(0)], Resolved::New);
    }

    /// One out-of-space seed must not take the in-space one down with it.
    #[test]
    fn a_stale_seed_beside_a_current_one_leaves_the_current_one_matching() {
        let this_meeting = clusters(&[(0, &[1.0, 0.0, 0.0])]);
        let mut stale = seed("bob", &[1.0, 0.0, 0.0], true);
        stale.model_version = "0".into();
        assert_eq!(
            resolve(
                &this_meeting,
                &[stale, seed("alice", &[0.98, 0.1, 0.0], true)]
            )[&Cluster(0)],
            Resolved::Existing("alice".into()),
            "the stale seed is not a candidate, so it is also not a runner-up the margin trips on"
        );
    }

    #[test]
    fn a_returning_voice_is_recognized_as_the_same_speaker() {
        // Story 28, and the only reason Voiceprints are stored at all.
        let this_meeting = clusters(&[(0, &[1.0, 0.0, 0.0])]);
        let history = vec![seed("alice", &[0.98, 0.1, 0.0], true)];
        let resolved = resolve(&this_meeting, &history);
        assert_eq!(
            resolved[&Cluster(0)],
            Resolved::Existing("alice".into()),
            "she came back"
        );
    }

    #[test]
    fn a_stranger_becomes_a_new_speaker_rather_than_the_nearest_match() {
        // The failure that costs trust: everyone the system does not know
        // being confidently labelled as the person it knows best.
        let this_meeting = clusters(&[(0, &[0.0, 0.0, 1.0])]);
        let history = vec![seed("alice", &[1.0, 0.0, 0.0], true)];
        assert_eq!(resolve(&this_meeting, &history)[&Cluster(0)], Resolved::New);
    }

    #[test]
    fn two_similar_voices_produce_no_match_rather_than_a_coin_flip() {
        // The margin condition. Without it, the closer two colleagues sound,
        // the more confidently the system mislabels one as the other — and
        // it is precisely colleagues who sound alike that an Operator would
        // struggle to catch.
        let this_meeting = clusters(&[(0, &[1.0, 0.05, 0.0])]);
        let history = vec![
            seed("alice", &[1.0, 0.0, 0.0], false),
            seed("bob", &[1.0, 0.1, 0.0], false),
        ];
        assert_eq!(
            resolve(&this_meeting, &history)[&Cluster(0)],
            Resolved::New,
            "too close to call is not a licence to guess"
        );
    }

    #[test]
    fn one_known_voice_cannot_claim_every_cluster_in_the_meeting() {
        // Mutual-best, and why leaving it out is expensive. Three people in
        // a room, one of whom the system knows: without this check the known
        // voice is the best match for all three clusters and the whole
        // meeting comes back as that person.
        let this_meeting = clusters(&[
            (0, &[1.00, 0.0, 0.0]),
            (1, &[0.95, 0.3, 0.0]),
            (2, &[0.90, 0.4, 0.0]),
        ]);
        let history = vec![seed("alice", &[1.0, 0.0, 0.0], true)];
        let resolved = resolve(&this_meeting, &history);
        let claimed = resolved
            .values()
            .filter(|r| matches!(r, Resolved::Existing(_)))
            .count();
        assert_eq!(claimed, 1, "she can only be one of them");
        assert_eq!(resolved[&Cluster(0)], Resolved::Existing("alice".into()));
    }

    #[test]
    fn a_confirmed_voiceprint_wins_a_tie() {
        // ADR-0008 as amended, and the one place the margin rule yields.
        // Acoustically these two are indistinguishable, so the tie-break
        // cannot come from the audio; it comes from the Operator having
        // vouched for one of them.
        let this_meeting = clusters(&[(0, &[1.0, 0.0])]);
        let history = vec![
            seed("unconfirmed", &[1.0, 0.0], false),
            seed("confirmed", &[1.0, 0.0], true),
        ];
        assert_eq!(
            resolve(&this_meeting, &history)[&Cluster(0)],
            Resolved::Existing("confirmed".into())
        );
    }

    #[test]
    fn two_confirmed_voiceprints_still_refuse_to_guess() {
        // The exception is scoped to what the Operator actually said. If
        // they vouched for both voices, they have said nothing that
        // separates these two, and the margin rule stands.
        let this_meeting = clusters(&[(0, &[1.0, 0.0])]);
        let history = vec![
            seed("alice", &[1.0, 0.0], true),
            seed("bob", &[1.0, 0.0], true),
        ];
        assert_eq!(resolve(&this_meeting, &history)[&Cluster(0)], Resolved::New);
    }

    #[test]
    fn confirmation_does_not_lower_the_floor() {
        // A confirmed Voiceprint matches more readily *between candidates*.
        // It must not make an unrelated voice match at all — that would turn
        // the Operator's helpfulness into a source of false attributions.
        let this_meeting = clusters(&[(0, &[0.0, 1.0])]);
        let history = vec![seed("alice", &[1.0, 0.0], true)];
        assert_eq!(resolve(&this_meeting, &history)[&Cluster(0)], Resolved::New);
    }

    #[test]
    fn an_empty_history_makes_everyone_new_without_dividing_by_zero() {
        let this_meeting = clusters(&[(0, &[1.0, 0.0]), (1, &[0.0, 1.0])]);
        let resolved = resolve(&this_meeting, &[]);
        assert_eq!(resolved.len(), 2);
        assert!(resolved.values().all(|r| *r == Resolved::New));
    }

    #[test]
    fn a_silent_cluster_matches_nobody() {
        // A zero vector has no direction. Returning 0 rather than NaN is what
        // stops it comparing equal to everything and being handed a name.
        assert_eq!(cosine(&[0.0, 0.0], &[1.0, 0.0]), 0.0);
        let this_meeting = clusters(&[(0, &[0.0, 0.0])]);
        let history = vec![seed("alice", &[1.0, 0.0], true)];
        assert_eq!(resolve(&this_meeting, &history)[&Cluster(0)], Resolved::New);
    }

    #[test]
    fn mismatched_embedding_widths_do_not_match() {
        // Two model spaces. ADR-0035 keeps model and version on the row for
        // exactly this; comparing across them yields a plausible number and
        // a meaningless one.
        assert_eq!(cosine(&[1.0, 0.0], &[1.0, 0.0, 0.0]), 0.0);
    }

    #[test]
    fn over_segmented_clusters_merge_back_into_one_voice() {
        // A clusterer splitting one person in two is commonplace, and the
        // Operator reads it as a stranger in their own meeting.
        let split = clusters(&[(0, &[1.0, 0.0, 0.0]), (1, &[0.97, 0.24, 0.0])]);
        let canonical = agglomerate(&split);
        assert_eq!(canonical[&Cluster(1)], canonical[&Cluster(0)]);
    }

    #[test]
    fn genuinely_different_voices_are_not_merged() {
        let distinct = clusters(&[(0, &[1.0, 0.0, 0.0]), (1, &[0.0, 1.0, 0.0])]);
        let canonical = agglomerate(&distinct);
        assert_ne!(canonical[&Cluster(1)], canonical[&Cluster(0)]);
    }

    #[test]
    fn the_centroid_is_weighted_by_how_much_voice_it_heard() {
        // Eight seconds and half a second are not equal evidence. Unweighted,
        // the shortest and noisiest observation pulls identity around.
        let exemplars = vec![(vec![1.0, 0.0], 8_000, false), (vec![0.0, 1.0], 500, false)];
        let centre = centroid(&exemplars).expect("a centroid");
        assert!(
            centre[0] > centre[1] * 4.0,
            "the long observation dominates: {centre:?}"
        );
    }

    #[test]
    fn negative_exemplars_never_enter_the_centroid() {
        // A negative exemplar records that a voice is *not* this Speaker.
        // Averaging it in would move the centroid toward the person it was
        // recorded to distinguish — making the same mistake more likely, from
        // the evidence that it was a mistake.
        let with_negative = vec![
            (vec![1.0, 0.0], 4_000, false),
            (vec![0.0, 1.0], 4_000, true),
        ];
        let centre = centroid(&with_negative).expect("a centroid");
        assert!(
            centre[1].abs() < 1e-6,
            "the negative is excluded: {centre:?}"
        );
    }

    #[test]
    fn exemplars_are_bounded_so_a_long_history_stays_cheap() {
        // A Speaker seen in two hundred Meetings must not carry two hundred
        // vectors into every later clustering run.
        let many: Vec<(Vec<f32>, i64, bool)> = (0..200)
            .map(|index| (vec![1.0, index as f32 / 1000.0], 1_000, false))
            .collect();
        assert!(centroid(&many).is_some());
        assert_eq!(MAX_EXEMPLARS, 32);
    }

    /// Characterization, not approval: this pins what the mint does **today**,
    /// which is the defect in `.scratch/diarization-independent/issues/13`.
    ///
    /// `centroid` tests nothing about whether its exemplars are the same
    /// voice, so it mints from ones that disagree completely. How bad the
    /// result is depends on the split: orthogonal groups of size `m` and `n`
    /// leave the smaller group at `m / sqrt(m² + n²)` against the vector
    /// filed under their name, which falls under `MATCH_FLOOR` once the
    /// minority is below about four fifths of the majority. On the real
    /// History a 4/5 split left the Operator's own exemplars at min 0.5142,
    /// and the vector matched a different participant at 0.6761 (Q247, Q248).
    ///
    /// **When ticket 13's guard lands, this test fails, and that is the
    /// signal.** Replace the body with `assert!(centroid(&mixed).is_none())`
    /// — the mint's existing "no vector this Speaker's record supports"
    /// fall-through — or with whichever subset rule the guard adopts.
    #[test]
    fn today_a_centroid_is_minted_from_exemplars_that_disagree_completely() {
        let minority = vec![1.0_f32, 0.0, 0.0];
        let majority = vec![0.0_f32, 1.0, 0.0];
        assert_eq!(
            cosine(&minority, &majority),
            0.0,
            "the premise: these are not the same voice"
        );

        let mixed: Vec<(Vec<f32>, i64, bool)> =
            std::iter::repeat_n((minority.clone(), 1_000, false), 3)
                .chain(std::iter::repeat_n((majority.clone(), 1_000, false), 6))
                .collect();

        let centre = centroid(&mixed).expect("today it mints regardless of disagreement");
        assert!(
            cosine(&centre, &minority) < MATCH_FLOOR,
            "the minority voice no longer matches its own Voiceprint: {}",
            cosine(&centre, &minority)
        );
        assert!(
            cosine(&centre, &majority) > MATCH_FLOOR,
            "while the voice that outnumbered it owns the name: {}",
            cosine(&centre, &majority)
        );
    }

    /// Ticket 14, and the assertion the characterization test was written to
    /// become: the cap is *spread* across the Meetings that contributed, not
    /// spent on a contiguous tail.
    ///
    /// The defect it replaces: exemplars are read `ORDER BY id` over UUIDv7
    /// ids minted at insert, so `.rev().take(MAX_EXEMPLARS)` took the last
    /// rows written — the end of the last Meeting that contributed any. On
    /// the real History, Jack Ahn's 888 exemplars span 7 Meetings and his
    /// tail 32 spanned 1 (Q251).
    #[test]
    fn the_cap_is_spread_across_meetings_rather_than_spent_on_a_tail() {
        let meetings: Vec<Option<&str>> = ["a", "b", "c", "d", "e"]
            .iter()
            .flat_map(|id| std::iter::repeat_n(Some(*id), 40))
            .collect();

        let chosen = spread_across_meetings(&meetings);
        assert_eq!(chosen.len(), MAX_EXEMPLARS, "the cap still binds");

        let mut share: BTreeMap<Option<&str>, usize> = BTreeMap::new();
        for index in &chosen {
            *share.entry(meetings[*index]).or_default() += 1;
        }
        assert_eq!(share.len(), 5, "every Meeting is represented: {share:?}");
        // 32 across 5 buckets: two get 7, three get 6 — never 32 and four 0s.
        assert!(
            share.values().all(|taken| (6..=7).contains(taken)),
            "and the shares are even: {share:?}"
        );

        // Recency survives as the ordering. Within a Meeting the newest rows
        // go first, so the last of Meeting a's 40 is taken and the first is
        // not — and the newest Meeting is served first of all.
        assert!(chosen.contains(&39), "the newest of Meeting a");
        assert!(!chosen.contains(&0), "not the oldest of Meeting a");
        assert_eq!(meetings[chosen[0]], Some("e"), "newest Meeting leads");
    }

    /// One Meeting, or none named at all: the selection still hands back
    /// exactly what the cap allows, newest first. The spread must not cost a
    /// Speaker heard once the identity the tail would have given them.
    #[test]
    fn a_speaker_heard_once_still_fills_the_cap_newest_first() {
        let single: Vec<Option<&str>> = std::iter::repeat_n(Some("a"), 100).collect();
        let chosen = spread_across_meetings(&single);
        assert_eq!(chosen.len(), MAX_EXEMPLARS);
        assert_eq!(chosen[0], 99, "newest first");
        assert_eq!(chosen[MAX_EXEMPLARS - 1], 100 - MAX_EXEMPLARS);

        let unattributed: Vec<Option<&str>> = std::iter::repeat_n(None, 40).collect();
        assert_eq!(
            spread_across_meetings(&unattributed).len(),
            MAX_EXEMPLARS,
            "no Meeting to spread across is not a reason to mint less"
        );

        assert!(
            spread_across_meetings(&[]).is_empty(),
            "and nothing selects nothing"
        );
    }

    /// Under the cap nothing is dropped, whatever the mix of Meetings.
    #[test]
    fn under_the_cap_every_exemplar_survives_the_selection() {
        let meetings = [Some("a"), Some("b"), None, Some("a"), Some("c")];
        let mut chosen = spread_across_meetings(&meetings);
        chosen.sort_unstable();
        assert_eq!(chosen, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn a_centroid_of_nothing_is_none_rather_than_a_zero_vector() {
        // A Speaker whose only exemplars were negative has no Voiceprint.
        // A zero vector would be a Voiceprint that matches nothing and
        // claims to exist.
        assert!(centroid(&[]).is_none());
        assert!(centroid(&[(vec![1.0, 0.0], 1_000, true)]).is_none());
    }

    fn db() -> rusqlite::Connection {
        let mut connection = rusqlite::Connection::open_in_memory().expect("open");
        crate::store::schema::migrate(&mut connection).expect("migrate");
        connection
    }

    /// A claim must not cost somebody else their own voice.
    ///
    /// The mutual-best rule is what stops one distinctive voice being handed
    /// to several clusters at once, and it asks each seed which cluster it
    /// likes best. A claimed cluster left in that comparison answers for a
    /// seed it is never going to be given: here the cluster the record
    /// already calls Alice is an exact match for Bob's seed, so Bob's own
    /// fragment loses mutual-best to it, comes back `New`, and the claimed
    /// cluster is then assigned to Alice anyway. Bob is unrecognized, and
    /// nothing in the answer says a claim did it.
    ///
    /// The numbers are chosen so that Bob's fragment passes on its own —
    /// 0.900 against his seed, 0.436 against Alice's, well clear of both
    /// floor and margin — and fails only through the claimed cluster.
    #[test]
    fn a_claimed_cluster_does_not_take_another_speakers_seed() {
        use crate::store::meetings;
        let connection = db();

        // Monday teaches both voices.
        let monday = meetings::start(&connection, Some("Monday"), None).expect("m1");
        let first = clusters(&[(0, &[0.0, 1.0]), (1, &[1.0, 0.0])]);
        let taught = persist(
            &connection,
            &monday.id,
            &first,
            &heard(&first),
            None,
            &Rebuilt::default(),
        )
        .expect("persist");
        let (alice, bob) = (taught[&Cluster(0)].clone(), taught[&Cluster(1)].clone());
        assert_ne!(alice, bob);

        // Friday: the record already says cluster 0 is Alice, and that
        // cluster's whole centroid happens to be exactly Bob's seed.
        let friday = meetings::start(&connection, Some("Friday"), None).expect("m2");
        let second = clusters(&[(0, &[1.0, 0.0]), (1, &[0.9, 0.435_889_9])]);
        let rebuilt = Rebuilt {
            claimed: [(Cluster(0), alice.clone())].into_iter().collect(),
            reseeded: BTreeSet::new(),
        };
        let map = persist(
            &connection,
            &friday.id,
            &second,
            &heard(&second),
            None,
            &rebuilt,
        )
        .expect("persist");

        assert_eq!(
            map.get(&Cluster(1)),
            Some(&bob),
            "Bob is still recognized: the claimed cluster was never his rival"
        );
        assert_eq!(
            map.get(&Cluster(0)),
            Some(&alice),
            "and the claim is honoured"
        );
        assert_eq!(
            crate::store::speakers::list(&connection)
                .expect("list")
                .len(),
            2,
            "with no third Speaker invented for a fragment that had an owner"
        );
    }

    /// A claim assigns words, so it does not need a vector to be honoured.
    ///
    /// `live::assemble` can keep a cluster's turns and still leave it out of
    /// `embeddings` when no centroid is available for it. Reading the claim
    /// out of the resolve's answer lost exactly those: the cluster owned
    /// words, the record named their owner, and the attribution was dropped
    /// because there was no vector to score.
    #[test]
    fn a_claim_is_honoured_for_a_cluster_with_no_embedding() {
        use crate::store::meetings;
        let connection = db();
        let meeting = meetings::start(&connection, Some("Monday"), None).expect("m1");
        let speaker = crate::store::speakers::create(&connection, false).expect("speaker");

        let embeddings = clusters(&[(0, &[1.0, 0.0])]);
        let mut owns_words = heard(&embeddings);
        owns_words.insert(Cluster(1));
        let rebuilt = Rebuilt {
            claimed: [(Cluster(1), speaker.id.clone())].into_iter().collect(),
            reseeded: BTreeSet::new(),
        };

        let map = persist(
            &connection,
            &meeting.id,
            &embeddings,
            &owns_words,
            None,
            &rebuilt,
        )
        .expect("persist");

        assert_eq!(
            map.get(&Cluster(1)),
            Some(&speaker.id),
            "the words are attributed even with no centroid to score"
        );
        assert!(
            crate::store::speakers::exemplars(&connection, &speaker.id)
                .expect("exemplars")
                .is_empty(),
            "and nothing was enrolled for a claim that carried no vector"
        );
    }

    /// The claim is assignment, never enrollment — restated at the boundary
    /// the previous test cannot reach, where the cluster *does* have a
    /// centroid.
    #[test]
    fn a_claimed_clusters_centroid_is_never_enrolled() {
        use crate::store::meetings;
        let connection = db();
        let meeting = meetings::start(&connection, Some("Monday"), None).expect("m1");
        let speaker = crate::store::speakers::create(&connection, false).expect("speaker");

        let embeddings = clusters(&[(0, &[1.0, 0.0])]);
        let rebuilt = Rebuilt {
            claimed: [(Cluster(0), speaker.id.clone())].into_iter().collect(),
            reseeded: BTreeSet::new(),
        };
        persist(
            &connection,
            &meeting.id,
            &embeddings,
            &heard(&embeddings),
            None,
            &rebuilt,
        )
        .expect("persist");

        assert!(
            crate::store::speakers::exemplars(&connection, &speaker.id)
                .expect("exemplars")
                .is_empty(),
            "the whole-cluster centroid is not evidence the Operator vouched for"
        );
    }

    #[test]
    fn the_same_voice_in_two_meetings_is_one_speaker() {
        // Story 28, end to end through the record. This is the assertion the
        // whole of Voiceprint storage exists to make true, and it has to be
        // a test rather than a demo.
        use crate::store::meetings;
        let connection = db();

        let monday = meetings::start(&connection, Some("Monday"), None).expect("m1");
        let first = clusters(&[(0, &[1.0, 0.0, 0.0]), (1, &[0.0, 1.0, 0.0])]);
        let monday_map = persist(
            &connection,
            &monday.id,
            &first,
            &heard(&first),
            None,
            &Rebuilt::default(),
        )
        .expect("persist");
        assert_eq!(monday_map.len(), 2, "two new voices");

        let friday = meetings::start(&connection, Some("Friday"), None).expect("m2");
        // The same first voice, heard slightly differently — a different
        // microphone, a different room.
        let second = clusters(&[(0, &[0.97, 0.05, 0.0])]);
        let friday_map = persist(
            &connection,
            &friday.id,
            &second,
            &heard(&second),
            None,
            &Rebuilt::default(),
        )
        .expect("persist");

        assert_eq!(
            friday_map[&Cluster(0)],
            monday_map[&Cluster(0)],
            "she is the same person on Friday"
        );
        assert_eq!(
            crate::store::speakers::list(&connection)
                .expect("list")
                .len(),
            2,
            "and no third Speaker was invented"
        );
    }

    #[test]
    fn a_new_voice_becomes_a_new_speaker_rather_than_joining_one() {
        use crate::store::meetings;
        let connection = db();

        let monday = meetings::start(&connection, None, None).expect("m1");
        let first = clusters(&[(0, &[1.0, 0.0, 0.0])]);
        persist(
            &connection,
            &monday.id,
            &first,
            &heard(&first),
            None,
            &Rebuilt::default(),
        )
        .expect("persist");

        let friday = meetings::start(&connection, None, None).expect("m2");
        let second = clusters(&[(0, &[0.0, 0.0, 1.0])]);
        persist(
            &connection,
            &friday.id,
            &second,
            &heard(&second),
            None,
            &Rebuilt::default(),
        )
        .expect("persist");

        assert_eq!(
            crate::store::speakers::list(&connection)
                .expect("list")
                .len(),
            2,
            "a stranger is a stranger"
        );
    }

    #[test]
    fn every_meeting_improves_the_voiceprint_it_seeded_from() {
        // ADR-0008 promises recognition that improves with every Meeting.
        // That is only true if the observation is folded back in, which is
        // what the exemplar plus recomputed centroid is for.
        use crate::store::meetings;
        let connection = db();

        let monday = meetings::start(&connection, None, None).expect("m1");
        let first = clusters(&[(0, &[1.0, 0.0, 0.0])]);
        let map = persist(
            &connection,
            &monday.id,
            &first,
            &heard(&first),
            None,
            &Rebuilt::default(),
        )
        .expect("persist");
        let speaker_id = map[&Cluster(0)].clone();
        assert_eq!(
            crate::store::speakers::exemplars(&connection, &speaker_id)
                .expect("exemplars")
                .len(),
            1
        );

        let friday = meetings::start(&connection, None, None).expect("m2");
        let second = clusters(&[(0, &[0.97, 0.05, 0.0])]);
        persist(
            &connection,
            &friday.id,
            &second,
            &heard(&second),
            None,
            &Rebuilt::default(),
        )
        .expect("persist");

        assert_eq!(
            crate::store::speakers::exemplars(&connection, &speaker_id)
                .expect("exemplars")
                .len(),
            2,
            "the second hearing was kept as evidence"
        );
        assert!(
            crate::store::speakers::get(&connection, &speaker_id)
                .expect("get")
                .expect("exists")
                .has_voiceprint
        );
    }

    #[test]

    fn a_voiceprint_from_another_model_is_never_offered_as_a_seed() {
        // ADR-0037's standing guard. The migration deletes old vectors, but
        // this is what holds if one ever survives — and it is what makes a
        // model change restart recognition honestly instead of comparing
        // numbers that do not measure the same thing.
        use crate::store::meetings;
        let connection = db();

        let monday = meetings::start(&connection, None, None).expect("m1");
        let first = clusters(&[(0, &[1.0, 0.0, 0.0])]);
        persist(
            &connection,
            &monday.id,
            &first,
            &heard(&first),
            None,
            &Rebuilt::default(),
        )
        .expect("persist");

        // Present for the model that made it.
        assert_eq!(seeds(&connection, "test", "1").expect("seeds").len(), 1);
        // Absent for any other, by name or by version.
        assert!(seeds(&connection, "other", "1").expect("seeds").is_empty());
        assert!(seeds(&connection, "test", "2").expect("seeds").is_empty());
    }

    #[test]
    fn same_width_vectors_from_different_models_do_not_recognize_each_other() {
        // The case vector length cannot catch. `cosine` scores mismatched
        // widths zero, which happens to save us for 256 against 192 and
        // would not for two 192-d models — so the guard cannot be a
        // coincidence of dimensions.
        use crate::store::meetings;
        let connection = db();

        let monday = meetings::start(&connection, None, None).expect("m1");
        let first = clusters(&[(0, &[1.0, 0.0, 0.0])]);
        let before = persist(
            &connection,
            &monday.id,
            &first,
            &heard(&first),
            None,
            &Rebuilt::default(),
        )
        .expect("persist");

        // The very same vector, stamped by a different model of equal width.
        let friday = meetings::start(&connection, None, None).expect("m2");
        let mut same: BTreeMap<Cluster, Embedding> = BTreeMap::new();
        same.insert(
            Cluster(0),
            Embedding::new(vec![1.0, 0.0, 0.0], "successor", "1", 30_000),
        );
        let after = persist(
            &connection,
            &friday.id,
            &same,
            &heard(&same),
            None,
            &Rebuilt::default(),
        )
        .expect("persist");

        assert_ne!(
            after[&Cluster(0)],
            before[&Cluster(0)],
            "an identical vector from another model must not be recognized as the same Speaker"
        );
    }

    #[test]
    fn a_deleted_voiceprint_stops_seeding_future_meetings() {
        // Story 31's real consequence: deletion has to actually stop
        // recognition, not just blank a column. If the vector kept seeding
        // the clusterer the Operator's act would be cosmetic.
        use crate::store::meetings;
        let connection = db();

        let monday = meetings::start(&connection, None, None).expect("m1");
        let first = clusters(&[(0, &[1.0, 0.0, 0.0])]);
        let map = persist(
            &connection,
            &monday.id,
            &first,
            &heard(&first),
            None,
            &Rebuilt::default(),
        )
        .expect("persist");
        let speaker_id = map[&Cluster(0)].clone();

        crate::store::speakers::delete_voiceprint(&connection, &speaker_id).expect("delete");
        assert!(seeds(&connection, "test", "1").expect("seeds").is_empty());

        let friday = meetings::start(&connection, None, None).expect("m2");
        let again = clusters(&[(0, &[1.0, 0.0, 0.0])]);
        let after = persist(
            &connection,
            &friday.id,
            &again,
            &heard(&again),
            None,
            &Rebuilt::default(),
        )
        .expect("persist");
        assert_ne!(
            after[&Cluster(0)],
            speaker_id,
            "the same voice is now a stranger, which is what deletion means"
        );
    }

    // ---- Ticket 12: what the Operator already said, read before it goes ----

    /// One Meeting, some segments, and a reconciliation over them.
    ///
    /// `owned` gives each segment its cluster and the Speaker standing on it
    /// as the machine's attribution; `None` for a cluster is silence.
    fn spoken(
        connection: &rusqlite::Connection,
        meeting_id: &str,
        owned: &[(Option<u32>, Option<&str>)],
    ) -> super::super::reconcile::Reconciliation {
        use crate::store::{meetings, speakers};
        let mut assignments = Vec::new();
        for (index, (cluster, owner)) in owned.iter().enumerate() {
            let start = index as i64 * 1_000;
            let segment = meetings::append_segment(
                connection,
                meeting_id,
                evertranscript_protocol::AudioChannel::Mic,
                start,
                start + 1_000,
                "hello",
            )
            .expect("segment");
            if let Some(owner) = owner {
                speakers::attribute_segment(
                    connection,
                    &segment.id,
                    Some(owner),
                    speakers::Attribution::Voiceprint,
                )
                .expect("attribute");
            }
            assignments.push(super::super::reconcile::Assignment {
                segment_id: segment.id,
                cluster: cluster.map(Cluster),
                straddles_boundary: false,
            });
        }
        super::super::reconcile::Reconciliation {
            assignments,
            boundary_flips: 0,
        }
    }

    fn named(connection: &rusqlite::Connection, name: &str) -> String {
        let id = crate::store::speakers::create(connection, false)
            .expect("create")
            .id;
        crate::store::speakers::rename(connection, &id, name).expect("rename");
        id
    }

    #[test]
    fn a_named_voice_keeps_its_cluster_across_a_model_change() {
        // The assertion the whole of ticket 12 exists to make true: without
        // it the first Meeting of a re-run hands a named voice to a fresh
        // pseudonym, having had every vector taken away.
        use crate::store::meetings;
        let connection = db();
        let meeting = meetings::start(&connection, None, None).expect("meeting");
        let alice = named(&connection, "Alice");

        let reconciliation = spoken(
            &connection,
            &meeting.id,
            &[(Some(0), Some(&alice)), (Some(0), Some(&alice))],
        );
        assert_eq!(
            claims(&connection, &reconciliation)
                .expect("claims")
                .claimed,
            [(Cluster(0), alice)].into_iter().collect()
        );
    }

    /// `relearnable` includes the Operator; this must not.
    ///
    /// The Operator claims like any other named Speaker, since Q237.
    ///
    /// This asserted the opposite until 2026-09-17, when the first real
    /// re-run showed what the exclusion cost: rule 3 of ADR-0029 as amended
    /// *is* the Voiceprint match, so an Operator whose Voiceprint no path
    /// rebuilds cannot be recognized after a model change at all (Q234,
    /// Q236). What still holds is what the assertion reads — a Speaker the
    /// record attributes these segments to, not a cluster vector.
    #[test]
    fn the_operator_claims_a_cluster_like_any_other_named_speaker() {
        use crate::store::{meetings, speakers};
        let connection = db();
        let meeting = meetings::start(&connection, None, None).expect("meeting");
        let me = named(&connection, "Me");
        speakers::set_operator(&connection, &me).expect("operator");

        assert!(
            speakers::relearnable(&connection)
                .expect("relearnable")
                .iter()
                .any(|speaker| speaker.id == me),
            "the premise: the query this is built on includes them"
        );

        let reconciliation = spoken(&connection, &meeting.id, &[(Some(0), Some(&me))]);
        let said = claims(&connection, &reconciliation).expect("claims");
        assert_eq!(
            said.claimed[&Cluster(0)],
            me,
            "the Operator's own attributed words are evidence about the Operator"
        );
    }

    #[test]
    fn a_correction_outranks_the_attribution_it_replaced() {
        // The Operator's word is the point of this whole path, so reading
        // `speaker_id` directly here would show them their own correction
        // being ignored.
        use crate::store::{meetings, speakers};
        let connection = db();
        let meeting = meetings::start(&connection, None, None).expect("meeting");
        let alice = named(&connection, "Alice");
        let bob = named(&connection, "Bob");

        let reconciliation = spoken(&connection, &meeting.id, &[(Some(0), Some(&alice))]);
        speakers::correct_attribution(&connection, &reconciliation.assignments[0].segment_id, &bob)
            .expect("correct");

        assert_eq!(
            claims(&connection, &reconciliation)
                .expect("claims")
                .claimed[&Cluster(0)],
            bob
        );
    }

    #[test]
    fn a_forgotten_voice_is_not_swept_back_in_by_a_rerun() {
        // Ticket 09's promise, made true of a bulk re-run and not only of
        // the moment of deletion.
        use crate::store::{meetings, speakers};
        let connection = db();
        let meeting = meetings::start(&connection, None, None).expect("meeting");
        let gone = named(&connection, "Gone");
        speakers::delete_voiceprint(&connection, &gone).expect("forget");

        let reconciliation = spoken(&connection, &meeting.id, &[(Some(0), Some(&gone))]);
        assert!(
            claims(&connection, &reconciliation)
                .expect("claims")
                .claimed
                .is_empty()
        );
    }

    #[test]
    fn a_pseudonym_claims_nothing_because_it_is_re_derived() {
        use crate::store::{meetings, speakers};
        let connection = db();
        let meeting = meetings::start(&connection, None, None).expect("meeting");
        let anonymous = speakers::create(&connection, false).expect("create").id;

        let reconciliation = spoken(&connection, &meeting.id, &[(Some(0), Some(&anonymous))]);
        assert!(
            claims(&connection, &reconciliation)
                .expect("claims")
                .claimed
                .is_empty()
        );
    }

    #[test]
    fn silence_is_in_no_cluster_and_breaks_no_claim() {
        use crate::store::meetings;
        let connection = db();
        let meeting = meetings::start(&connection, None, None).expect("meeting");
        let alice = named(&connection, "Alice");
        let bob = named(&connection, "Bob");

        let reconciliation = spoken(
            &connection,
            &meeting.id,
            &[
                (Some(0), Some(&alice)),
                (Some(0), Some(&alice)),
                // Nobody was speaking here, whatever the record says was
                // standing on it, so it is evidence about no cluster.
                (None, Some(&bob)),
                (None, None),
            ],
        );
        assert_eq!(
            claims(&connection, &reconciliation)
                .expect("claims")
                .claimed,
            [(Cluster(0), alice)].into_iter().collect()
        );
    }

    /// A name does not claim a cluster it only partly owns.
    ///
    /// The re-run redraws the clusters, so one can arrive holding a named
    /// person's words *and* audio nobody has vouched for. Claiming it whole
    /// would enroll the unvouched part under a name the Operator trusts, and
    /// the Voiceprint the re-run then builds would be cut from all of it.
    /// Abstaining costs the shortcut past the resolve, not the person: the
    /// seeding path can still rebuild them from the ranges that are theirs.
    #[test]
    fn a_name_does_not_claim_a_cluster_it_only_partly_owns() {
        use crate::store::{meetings, speakers};
        let connection = db();
        let meeting = meetings::start(&connection, None, None).expect("meeting");
        let alice = named(&connection, "Alice");
        let anonymous = speakers::create(&connection, false).expect("create").id;

        let reconciliation = spoken(
            &connection,
            &meeting.id,
            &[
                (Some(0), Some(&anonymous)),
                (Some(0), Some(&anonymous)),
                (Some(0), Some(&alice)),
            ],
        );
        assert!(
            claims(&connection, &reconciliation)
                .expect("claims")
                .claimed
                .is_empty()
        );
    }

    #[test]
    fn a_cluster_two_named_voices_share_is_claimed_by_neither() {
        // A merged cluster is the case with no right answer, and picking
        // one by comparing two UUIDs would be picking it at random.
        use crate::store::meetings;
        let connection = db();
        let meeting = meetings::start(&connection, None, None).expect("meeting");
        let alice = named(&connection, "Alice");
        let bob = named(&connection, "Bob");

        let reconciliation = spoken(
            &connection,
            &meeting.id,
            &[(Some(0), Some(&alice)), (Some(0), Some(&bob))],
        );
        assert!(
            claims(&connection, &reconciliation)
                .expect("claims")
                .claimed
                .is_empty()
        );
    }

    #[test]
    fn splitting_an_utterance_cannot_change_which_identity_is_claimed() {
        // How finely the transcript was cut is not evidence about whose
        // voice it is, so it must not be able to decide one.
        use crate::store::meetings;
        let connection = db();
        let alice = named(&connection, "Alice");
        let bob = named(&connection, "Bob");

        // Cluster 0 is all Alice's; cluster 1 holds hers and Bob's together.
        let coarse = meetings::start(&connection, None, None).expect("meeting");
        let coarse = spoken(
            &connection,
            &coarse.id,
            &[
                (Some(0), Some(&alice)),
                (Some(1), Some(&alice)),
                (Some(1), Some(&bob)),
            ],
        );
        // The same words, cut into more segments.
        let fine = meetings::start(&connection, None, None).expect("meeting");
        let fine = spoken(
            &connection,
            &fine.id,
            &[
                (Some(0), Some(&alice)),
                (Some(0), Some(&alice)),
                (Some(0), Some(&alice)),
                (Some(1), Some(&alice)),
                (Some(1), Some(&alice)),
                (Some(1), Some(&alice)),
                (Some(1), Some(&bob)),
            ],
        );

        let coarse = claims(&connection, &coarse).expect("coarse").claimed;
        assert_eq!(
            coarse,
            claims(&connection, &fine).expect("fine").claimed,
            "three Alice segments against one of Bob's is still a shared cluster"
        );
        assert_eq!(coarse, [(Cluster(0), alice)].into_iter().collect());
    }

    // ---- Ticket 12: the other half, what the correction denied ----

    #[test]
    fn a_correction_denies_the_cluster_it_took_every_word_from() {
        use crate::store::{meetings, speakers};
        let connection = db();
        let meeting = meetings::start(&connection, None, None).expect("meeting");
        let alice = named(&connection, "Alice");
        let bob = named(&connection, "Bob");

        let reconciliation = spoken(
            &connection,
            &meeting.id,
            &[(Some(0), Some(&alice)), (Some(0), Some(&alice))],
        );
        for assignment in &reconciliation.assignments {
            speakers::correct_attribution(&connection, &assignment.segment_id, &bob)
                .expect("correct");
        }

        let said = claims(&connection, &reconciliation).expect("claims");
        assert_eq!(said.claimed[&Cluster(0)], bob);
        assert_eq!(
            said.denied,
            [(alice, Cluster(0))].into_iter().collect(),
            "a correction says whose voice it was and whose it was not"
        );
    }

    /// The negative half is held to the standard the positive half is.
    ///
    /// Two of these three segments are audio the Operator never disputed.
    /// A negative exemplar cut from the whole cluster would suppress Alice
    /// using evidence that is partly *for* her.
    #[test]
    fn a_correction_over_part_of_a_cluster_denies_none_of_it() {
        use crate::store::{meetings, speakers};
        let connection = db();
        let meeting = meetings::start(&connection, None, None).expect("meeting");
        let alice = named(&connection, "Alice");
        let bob = named(&connection, "Bob");

        let reconciliation = spoken(
            &connection,
            &meeting.id,
            &[
                (Some(0), Some(&alice)),
                (Some(0), Some(&alice)),
                (Some(0), Some(&alice)),
            ],
        );
        speakers::correct_attribution(&connection, &reconciliation.assignments[0].segment_id, &bob)
            .expect("correct");

        let said = claims(&connection, &reconciliation).expect("claims");
        assert!(said.denied.is_empty(), "one word of three denies nothing");
        assert!(
            said.claimed.is_empty(),
            "and the cluster is now shared, so nobody claims it either"
        );
    }

    #[test]
    fn a_cluster_its_owner_still_owns_is_not_denied_to_them() {
        // Re-asserting an attribution is not a correction against the person
        // it was re-asserted for.
        use crate::store::{meetings, speakers};
        let connection = db();
        let meeting = meetings::start(&connection, None, None).expect("meeting");
        let alice = named(&connection, "Alice");

        let reconciliation = spoken(&connection, &meeting.id, &[(Some(0), Some(&alice))]);
        speakers::correct_attribution(
            &connection,
            &reconciliation.assignments[0].segment_id,
            &alice,
        )
        .expect("correct");

        let said = claims(&connection, &reconciliation).expect("claims");
        assert_eq!(said.claimed[&Cluster(0)], alice);
        assert!(said.denied.is_empty());
    }

    #[test]
    fn a_withheld_voiceprint_is_absent_from_the_whole_resolve() {
        // ADR-0029's third rule gates the Operator's Voiceprint on the
        // Meeting being big enough for a cross-Meeting match to mean
        // anything. A gate applied only to the "You" flag would be
        // decorative: the seed would still sit here among every other voice,
        // match, and hand the cluster to the Operator's Speaker — just
        // without anyone reconsidering the flag.
        //
        // So the assertion is about the *attribution*, not the flag: under
        // the gate, the Operator's own voice comes back a stranger.
        use crate::store::meetings;
        let connection = db();

        let monday = meetings::start(&connection, None, None).expect("m1");
        let mine = clusters(&[(0, &[1.0, 0.0, 0.0])]);
        let map = persist(
            &connection,
            &monday.id,
            &mine,
            &heard(&mine),
            None,
            &Rebuilt::default(),
        )
        .expect("persist");
        let me = map[&Cluster(0)].clone();

        // The same voice again, with that Speaker held out of the resolve.
        let friday = meetings::start(&connection, None, None).expect("m2");
        let again = clusters(&[(0, &[1.0, 0.0, 0.0])]);
        let withheld = persist(
            &connection,
            &friday.id,
            &again,
            &heard(&again),
            Some(&me),
            &Rebuilt::default(),
        )
        .expect("persist");
        assert_ne!(
            withheld[&Cluster(0)],
            me,
            "the withheld seed must not be able to claim the cluster by the back door"
        );
        // The control is `the_same_voice_in_two_meetings_is_one_speaker`:
        // identical inputs with `withheld: None` do recognize it. The
        // Voiceprint is still there — it was held out of one run, not
        // deleted — which is what `a_deleted_voiceprint_stops_seeding_future_meetings`
        // asserts the difference against.
        assert!(
            !seeds(&connection, "test", "1")
                .expect("seeds")
                .iter()
                .all(|seed| seed.speaker_id != me),
            "withholding is per-run, not destruction"
        );
    }

    #[test]
    fn a_voice_that_owns_no_words_does_not_become_a_speaker() {
        // The Registry's 378 strangers, reproduced. A cluster the clusterer
        // left standing but no transcript segment landed in — echo, a
        // cough, the far end leaking through the speakers — used to be
        // minted with a Voiceprint like anybody else.
        use crate::store::meetings;
        let connection = db();
        let meeting = meetings::start(&connection, None, None).expect("m");
        let voices = clusters(&[(0, &[1.0, 0.0, 0.0]), (1, &[0.0, 1.0, 0.0])]);

        let only_first: BTreeSet<Cluster> = [Cluster(0)].into_iter().collect();
        let assigned = persist(
            &connection,
            &meeting.id,
            &voices,
            &only_first,
            None,
            &Rebuilt::default(),
        )
        .expect("persist");

        assert_eq!(assigned.len(), 1, "the voice with words is somebody");
        assert!(!assigned.contains_key(&Cluster(1)));
        assert_eq!(
            crate::store::speakers::list(&connection)
                .expect("list")
                .len(),
            1,
            "and the wordless one was not minted"
        );
    }

    #[test]
    fn a_voice_too_brief_to_trust_is_left_unattributed_rather_than_minted() {
        // The other half of the same failure: a stranger heard for three
        // seconds is not enough evidence to store a biometric on. Its
        // segments stay honestly unattributed, and it can earn a Speaker
        // in a Meeting where it actually talks.
        use crate::store::meetings;
        let connection = db();
        let meeting = meetings::start(&connection, None, None).expect("m");
        let brief: BTreeMap<Cluster, Embedding> = [(
            Cluster(0),
            Embedding::new(vec![1.0, 0.0, 0.0], "test", "1", MIN_SPEAKER_MS - 1),
        )]
        .into_iter()
        .collect();

        let assigned = persist(
            &connection,
            &meeting.id,
            &brief,
            &heard(&brief),
            None,
            &Rebuilt::default(),
        )
        .expect("persist");

        assert!(assigned.is_empty());
        assert!(
            crate::store::speakers::list(&connection)
                .expect("list")
                .is_empty(),
            "no Speaker, no Voiceprint"
        );
    }

    #[test]
    fn a_known_voice_is_recognized_however_briefly_it_spoke() {
        // The floor gates minting, not recognition. Alice saying one
        // sentence is still Alice — "what did Alice say" has to find it —
        // and the conservative match rule is what guards against a wrong
        // name, not a duration.
        use crate::store::meetings;
        let connection = db();

        let monday = meetings::start(&connection, None, None).expect("m1");
        let first = clusters(&[(0, &[1.0, 0.0, 0.0])]);
        let map = persist(
            &connection,
            &monday.id,
            &first,
            &heard(&first),
            None,
            &Rebuilt::default(),
        )
        .expect("persist");
        let alice = map[&Cluster(0)].clone();

        let friday = meetings::start(&connection, None, None).expect("m2");
        let brief: BTreeMap<Cluster, Embedding> = [(
            Cluster(0),
            Embedding::new(vec![0.98, 0.1, 0.0], "test", "1", 2_500),
        )]
        .into_iter()
        .collect();
        let again = persist(
            &connection,
            &friday.id,
            &brief,
            &heard(&brief),
            None,
            &Rebuilt::default(),
        )
        .expect("persist");

        assert_eq!(again[&Cluster(0)], alice, "recognized");
        let evidence = crate::store::speakers::exemplars(&connection, &alice).expect("exemplars");
        assert_eq!(
            evidence.len(),
            2,
            "and the brief hearing is kept as evidence"
        );
        assert_eq!(
            evidence[1].voiced_ms, 2_500,
            "at its real weight, so it cannot pull the Voiceprint around"
        );
    }

    #[test]
    fn an_exemplar_remembers_where_the_voice_can_be_heard() {
        // What the Registry plays back. The window travels from the seam
        // to the row unchanged, and the row's Meeting is where to cut it
        // from.
        use crate::audio::CaptureOffset;
        use crate::store::meetings;
        use evertranscript_protocol::AudioChannel;
        let connection = db();
        let meeting = meetings::start(&connection, None, None).expect("m");

        let window = super::super::SampleWindow {
            channel: AudioChannel::System,
            start: CaptureOffset(61_000),
            end: CaptureOffset(71_000),
        };
        let voices: BTreeMap<Cluster, Embedding> =
            [(Cluster(0), embedding(&[1.0, 0.0, 0.0]).with_sample(window))]
                .into_iter()
                .collect();
        let map = persist(
            &connection,
            &meeting.id,
            &voices,
            &heard(&voices),
            None,
            &Rebuilt::default(),
        )
        .expect("persist");

        let source = crate::store::speakers::sample_source(&connection, &map[&Cluster(0)])
            .expect("query")
            .expect("a sample");
        assert_eq!(source.meeting_id, meeting.id);
        assert_eq!(source.sample.channel, AudioChannel::System);
        assert_eq!(
            (source.sample.start_ms, source.sample.end_ms),
            (61_000, 71_000)
        );
    }

    /// A Speaker minted in one space, with a sample to rebuild it from.
    fn old_space_speaker(
        connection: &rusqlite::Connection,
        meeting_id: &str,
        vector: &[f32],
    ) -> (String, String) {
        use crate::audio::CaptureOffset;
        use evertranscript_protocol::AudioChannel;
        let window = super::super::SampleWindow {
            channel: AudioChannel::Mic,
            start: CaptureOffset(1_000),
            end: CaptureOffset(6_000),
        };
        let voices: BTreeMap<Cluster, Embedding> =
            [(Cluster(0), embedding(vector).with_sample(window))]
                .into_iter()
                .collect();
        let map = persist(
            connection,
            meeting_id,
            &voices,
            &heard(&voices),
            None,
            &Rebuilt::default(),
        )
        .expect("persist");
        let speaker_id = map[&Cluster(0)].clone();
        let evidence = crate::store::speakers::exemplars(connection, &speaker_id).expect("rows");
        (speaker_id, evidence[0].id.clone())
    }

    #[test]
    fn evidence_from_an_old_front_end_is_rebuilt_before_it_is_compared() {
        // ADR-0035's promise, made real by the first front-end fix: the
        // vectors on file were from features the model was never trained
        // on, so offering them as seeds would have matched nothing. They
        // are re-embedded from the audio they were cut from, and until
        // then the new space simply does not see them.
        use crate::store::meetings;
        use crate::store::speakers;
        let connection = db();
        let meeting = meetings::start(&connection, None, None).expect("m");
        meetings::set_audio_path(&connection, &meeting.id, ".data/audio/m.mp3").expect("path");
        let (speaker_id, exemplar_id) =
            old_space_speaker(&connection, &meeting.id, &[1.0, 0.0, 0.0]);

        assert!(
            seeds(&connection, "test", "2")
                .expect("new space")
                .is_empty()
        );
        let stale = speakers::stale_exemplars(&connection, "test", "2").expect("stale");
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].id, exemplar_id);
        let (audio_path, sample) = stale[0].source.clone().expect("rebuildable");
        assert_eq!(audio_path, ".data/audio/m.mp3");
        assert_eq!((sample.start_ms, sample.end_ms), (1_000, 6_000));

        let rebuilt = vec![(exemplar_id, Some(vec![0.0, 1.0, 0.0]))];
        let adopted = adopt_rebuilt(&connection, &rebuilt, "test", "2").expect("adopt");
        assert_eq!(
            adopted,
            Adopted {
                rebuilt: 1,
                dropped: 0,
                speakers: 1
            }
        );

        let offered = seeds(&connection, "test", "2").expect("seeds");
        assert_eq!(offered.len(), 1);
        assert_eq!(offered[0].speaker_id, speaker_id);
        assert_eq!(offered[0].vector, vec![0.0, 1.0, 0.0], "the rebuilt vector");
        assert!(
            seeds(&connection, "test", "1")
                .expect("old space")
                .is_empty()
        );
        assert!(
            speakers::stale_exemplars(&connection, "test", "2")
                .expect("stale")
                .is_empty()
        );
        let speaker = speakers::get(&connection, &speaker_id)
            .expect("get")
            .expect("exists");
        assert_eq!(speaker.voiceprint_model_version.as_deref(), Some("2"));

        // And a re-run in the new space recognizes the rebuilt voice.
        let friday = meetings::start(&connection, None, None).expect("m2");
        let again: BTreeMap<Cluster, Embedding> = [(
            Cluster(0),
            Embedding::new(vec![0.05, 0.97, 0.0], "test", "2", 30_000),
        )]
        .into_iter()
        .collect();
        let map = persist(
            &connection,
            &friday.id,
            &again,
            &heard(&again),
            None,
            &Rebuilt::default(),
        )
        .expect("persist");
        assert_eq!(
            map[&Cluster(0)],
            speaker_id,
            "recognized across the front-end change"
        );
    }

    #[test]
    fn evidence_with_no_audio_behind_it_is_dropped_and_the_name_stays() {
        // An exemplar written before samples were kept, or whose Meeting
        // is gone, cannot be rebuilt. It goes, honestly: the Speaker keeps
        // its name and its words and loses the Voiceprint nothing can be
        // heard of, exactly as ADR-0009's deletion leaves a Speaker.
        use crate::store::speakers;
        let connection = db();
        let speaker = speakers::create(&connection, false).expect("speaker");
        speakers::rename(&connection, &speaker.id, "Alice").expect("name");
        speakers::add_exemplar(
            &connection,
            speakers::NewExemplar {
                speaker_id: &speaker.id,
                meeting_id: None,
                vector: &[1.0, 0.0, 0.0],
                model: "test",
                model_version: "1",
                voiced_ms: 12_000,
                from_operator: false,
                is_negative: false,
                sample: None,
            },
        )
        .expect("orphan");
        speakers::set_voiceprint(&connection, &speaker.id, &[1.0, 0.0, 0.0], "test", "1")
            .expect("voiceprint");

        let stale = speakers::stale_exemplars(&connection, "test", "2").expect("stale");
        assert_eq!(stale.len(), 1);
        assert!(stale[0].source.is_none(), "nothing to rebuild from");
        let adopted =
            adopt_rebuilt(&connection, &[(stale[0].id.clone(), None)], "test", "2").expect("adopt");
        assert_eq!(
            adopted,
            Adopted {
                rebuilt: 0,
                dropped: 1,
                speakers: 1
            }
        );

        let after = speakers::get(&connection, &speaker.id)
            .expect("get")
            .expect("exists");
        assert!(!after.has_voiceprint, "a stranger next time");
        assert_eq!(
            after.display_name.as_deref(),
            Some("Alice"),
            "but still Alice"
        );
        assert!(
            speakers::exemplars(&connection, &speaker.id)
                .expect("rows")
                .is_empty()
        );
        assert!(seeds(&connection, "test", "1").expect("old").is_empty());
        assert!(seeds(&connection, "test", "2").expect("new").is_empty());
    }

    #[test]
    fn a_voiceprint_nothing_can_recompute_does_not_linger_in_the_old_space() {
        // A Speaker whose only evidence is negative keeps its Voiceprint
        // through an ordinary refresh (there is nothing to replace it
        // with), but a Voiceprint from the old space is not something to
        // keep: no run would offer it, and every run would find it stale.
        use crate::store::meetings;
        use crate::store::speakers;
        let connection = db();
        let meeting = meetings::start(&connection, None, None).expect("m");
        meetings::set_audio_path(&connection, &meeting.id, ".data/audio/m.mp3").expect("path");
        let (speaker_id, exemplar_id) =
            old_space_speaker(&connection, &meeting.id, &[1.0, 0.0, 0.0]);
        connection
            .execute(
                "UPDATE speaker_exemplars SET is_negative = 1 WHERE id = ?1",
                rusqlite::params![exemplar_id],
            )
            .expect("negate");

        let adopted = adopt_rebuilt(
            &connection,
            &[(exemplar_id.clone(), Some(vec![0.0, 1.0, 0.0]))],
            "test",
            "2",
        )
        .expect("adopt");
        assert_eq!(adopted.rebuilt, 1);

        let after = speakers::get(&connection, &speaker_id)
            .expect("get")
            .expect("exists");
        assert!(!after.has_voiceprint);
        let evidence = speakers::exemplars(&connection, &speaker_id).expect("rows");
        assert_eq!(evidence.len(), 1, "the negative evidence is kept");
        assert_eq!(evidence[0].model_version, "2", "in the new space");
        assert!(
            speakers::stale_exemplars(&connection, "test", "2")
                .expect("stale")
                .is_empty()
        );
        assert!(
            speakers::speakers_with_stale_voiceprint(&connection, "test", "2")
                .expect("stale prints")
                .is_empty()
        );
    }

    #[test]
    fn a_voiceprint_averages_one_space_only() {
        // Two spaces averaged together is a vector in neither. After a
        // rebuild every row is in one space, but the refresh guards it
        // anyway: the newest evidence decides the space.
        use crate::store::meetings;
        use crate::store::speakers;
        let connection = db();
        let meeting = meetings::start(&connection, None, None).expect("m");
        let speaker = speakers::create(&connection, false).expect("speaker");
        for (vector, version) in [([1.0_f32, 0.0, 0.0], "1"), ([0.0, 1.0, 0.0], "2")] {
            speakers::add_exemplar(
                &connection,
                speakers::NewExemplar {
                    speaker_id: &speaker.id,
                    meeting_id: Some(&meeting.id),
                    vector: &vector,
                    model: "test",
                    model_version: version,
                    voiced_ms: 12_000,
                    from_operator: false,
                    is_negative: false,
                    sample: None,
                },
            )
            .expect("exemplar");
        }
        refresh_voiceprint(&connection, &speaker.id).expect("refresh");
        let offered = seeds(&connection, "test", "2").expect("seeds");
        assert_eq!(offered.len(), 1);
        assert_eq!(
            offered[0].vector,
            vec![0.0, 1.0, 0.0],
            "the old vector did not pull it"
        );
    }

    #[test]
    fn a_voiceprint_draws_on_every_meeting_the_speaker_was_heard_in() {
        // Ticket 14, through the mint path rather than the selection alone.
        // Two Meetings, forty exemplars each, and a voice that reads
        // differently in the second. Under the old tail the later Meeting
        // filled all 32 slots and owned the identity outright; the spread
        // gives each Meeting half, so the Voiceprint sits between them.
        use crate::store::meetings;
        use crate::store::speakers;
        let connection = db();
        let speaker = speakers::create(&connection, false).expect("speaker");
        let earlier = [1.0_f32, 0.0, 0.0];
        let later = [0.0_f32, 1.0, 0.0];
        for vector in [earlier, later] {
            let meeting = meetings::start(&connection, None, None).expect("m");
            for _ in 0..40 {
                speakers::add_exemplar(
                    &connection,
                    speakers::NewExemplar {
                        speaker_id: &speaker.id,
                        meeting_id: Some(&meeting.id),
                        vector: &vector,
                        model: "test",
                        model_version: "1",
                        voiced_ms: 12_000,
                        from_operator: false,
                        is_negative: false,
                        sample: None,
                    },
                )
                .expect("exemplar");
            }
        }

        refresh_voiceprint(&connection, &speaker.id).expect("refresh");
        let offered = seeds(&connection, "test", "1").expect("seeds");
        assert_eq!(offered.len(), 1);
        let minted = &offered[0].vector;
        assert!(
            cosine(minted, &earlier) > 0.6,
            "the earlier Meeting is still in it: {}",
            cosine(minted, &earlier)
        );
        assert!(
            cosine(minted, &later) > 0.6,
            "and so is the later one: {}",
            cosine(minted, &later)
        );
    }

    #[test]
    fn a_re_run_replaces_the_speakers_the_first_run_minted() {
        // What the first real re-runs showed. The previous run's
        // Voiceprints were cut from this very audio, so the second run
        // recognized every one of them from it and the floor never got a
        // say. Now the first run's evidence is withdrawn before the seeds
        // are read, and what it minted goes once the segments have moved.
        use crate::store::meetings;
        use crate::store::speakers;
        use evertranscript_protocol::AudioChannel;
        let connection = db();
        let meeting = meetings::start(&connection, None, None).expect("m");
        let segment = meetings::append_segment(
            &connection,
            &meeting.id,
            AudioChannel::System,
            0,
            5_000,
            "hi",
        )
        .expect("segment");

        let first = clusters(&[(0, &[1.0, 0.0, 0.0])]);
        let stale = persist(
            &connection,
            &meeting.id,
            &first,
            &heard(&first),
            None,
            &Rebuilt::default(),
        )
        .expect("persist")[&Cluster(0)]
            .clone();
        speakers::attribute_segment(
            &connection,
            &segment.id,
            Some(&stale),
            speakers::Attribution::Clustered,
        )
        .expect("attribute");

        // The same voice, exactly, as the re-run clusters it.
        let again = clusters(&[(0, &[1.0, 0.0, 0.0])]);
        let fresh = persist(
            &connection,
            &meeting.id,
            &again,
            &heard(&again),
            None,
            &Rebuilt::default(),
        )
        .expect("persist")[&Cluster(0)]
            .clone();
        assert_ne!(
            fresh, stale,
            "the first run's Speaker did not seed the second"
        );
        // What `reconcile::apply` does next.
        speakers::attribute_segment(
            &connection,
            &segment.id,
            Some(&fresh),
            speakers::Attribution::Clustered,
        )
        .expect("re-attribute");

        assert_eq!(speakers::sweep_unreferenced(&connection).expect("sweep"), 1);
        let left = speakers::list(&connection).expect("list");
        assert_eq!(left.len(), 1, "one voice, one Speaker");
        assert_eq!(left[0].id, fresh);
        assert_eq!(
            speakers::exemplars(&connection, &fresh)
                .expect("exemplars")
                .len(),
            1
        );
    }

    #[test]
    fn a_re_run_keeps_the_speaker_the_operator_named() {
        // The other half. Alice was named from this Meeting, so her
        // evidence from it is the Operator's word rather than the machine's
        // guess: it stays as a seed, she is recognized, and this run's
        // hearing replaces the old one instead of doubling it.
        use crate::store::meetings;
        use crate::store::speakers;
        let connection = db();
        let meeting = meetings::start(&connection, None, None).expect("m");

        let first = clusters(&[(0, &[1.0, 0.0, 0.0])]);
        let alice = persist(
            &connection,
            &meeting.id,
            &first,
            &heard(&first),
            None,
            &Rebuilt::default(),
        )
        .expect("persist")[&Cluster(0)]
            .clone();
        speakers::rename(&connection, &alice, "Alice").expect("rename");

        let again = clusters(&[(0, &[0.98, 0.1, 0.0])]);
        let recognized = persist(
            &connection,
            &meeting.id,
            &again,
            &heard(&again),
            None,
            &Rebuilt::default(),
        )
        .expect("persist")[&Cluster(0)]
            .clone();

        assert_eq!(recognized, alice, "still Alice");
        let evidence = speakers::exemplars(&connection, &alice).expect("exemplars");
        assert_eq!(evidence.len(), 1, "replaced, not doubled");
        assert_eq!(
            evidence[0].vector,
            again[&Cluster(0)].vector,
            "with this run's hearing"
        );
        assert_eq!(speakers::list(&connection).expect("list").len(), 1);
    }

    #[test]
    fn a_re_run_withdraws_one_meetings_evidence_and_leaves_the_rest() {
        // A voice heard in two Meetings is still recognized when one of
        // them is re-run: the other Meeting's evidence carries it, and the
        // re-run adds its own beside that rather than beside itself.
        use crate::store::meetings;
        use crate::store::speakers;
        let connection = db();

        let monday = meetings::start(&connection, None, None).expect("m1");
        let first = clusters(&[(0, &[1.0, 0.0, 0.0])]);
        let voice = persist(
            &connection,
            &monday.id,
            &first,
            &heard(&first),
            None,
            &Rebuilt::default(),
        )
        .expect("persist")[&Cluster(0)]
            .clone();
        let friday = meetings::start(&connection, None, None).expect("m2");
        let second = clusters(&[(0, &[0.97, 0.05, 0.0])]);
        persist(
            &connection,
            &friday.id,
            &second,
            &heard(&second),
            None,
            &Rebuilt::default(),
        )
        .expect("persist");

        let again = clusters(&[(0, &[0.99, 0.02, 0.0])]);
        let recognized = persist(
            &connection,
            &monday.id,
            &again,
            &heard(&again),
            None,
            &Rebuilt::default(),
        )
        .expect("persist")[&Cluster(0)]
            .clone();

        assert_eq!(recognized, voice);
        let evidence = speakers::exemplars(&connection, &voice).expect("exemplars");
        assert_eq!(evidence.len(), 2, "Friday's, and the new Monday");
        assert!(
            !evidence
                .iter()
                .any(|exemplar| exemplar.vector == first[&Cluster(0)].vector),
            "the old Monday hearing is gone"
        );
    }

    #[test]
    fn a_voice_whose_windows_drift_still_ends_as_one_cluster() {
        // The close-out found three speakers in a two-speaker recording,
        // two of them the same person. The cause was comparing each window
        // against one earlier member rather than against the group: A and B
        // are within threshold, B and C are, A and C are not, and a single
        // pass leaves two groups that never meet.
        let chain = clusters(&[
            (0, &[1.00, 0.00, 0.0]),
            (1, &[0.80, 0.60, 0.0]),
            (2, &[0.55, 0.84, 0.0]),
        ]);
        assert!(
            cosine(&[1.00, 0.00, 0.0], &[0.55, 0.84, 0.0]) < MERGE_THRESHOLD,
            "the end points are too far apart to join directly"
        );
        let canonical = agglomerate(&chain);
        assert_eq!(
            canonical[&Cluster(0)],
            canonical[&Cluster(2)],
            "but they are one drifting voice"
        );
    }

    #[test]
    fn the_group_name_does_not_depend_on_merge_order() {
        // Otherwise "Speaker 1" and "Speaker 2" swap between two runs over
        // the same audio, and an Operator who named one has named the other.
        let split = clusters(&[(7, &[1.0, 0.0, 0.0]), (2, &[0.97, 0.24, 0.0])]);
        assert_eq!(agglomerate(&split)[&Cluster(7)], Cluster(2));
    }

    /// Windows from `voices` distinct speakers, interleaved the way a real
    /// meeting produces them — a few windows each, round robin — with a
    /// little jitter so no two vectors are identical.
    fn many_windows(voices: usize, windows: usize) -> BTreeMap<Cluster, Embedding> {
        (0..windows)
            .map(|index| {
                let voice = index % voices;
                let mut vector = vec![0.0f32; voices];
                vector[voice] = 1.0;
                // Enough wobble to be a different vector, far too little to
                // be a different speaker.
                vector[(voice + 1) % voices] = 0.02 * ((index % 7) as f32);
                (Cluster(index as u32), embedding(&vector))
            })
            .collect()
    }

    /// A pair can be forbidden and still end up merged, unless the
    /// constraint survives the merge that carries them together.
    ///
    /// This is the whole reason constraints live on the group. A and B are
    /// forbidden and are never the closest pair, so a check that only looked
    /// at the candidate merge in front of it would never see them: what
    /// happens is A merges with C, and then B merges with *that group*,
    /// which puts A and B in one cluster without any step having compared
    /// them. The refusal has to come from the survivor inheriting A's
    /// constraint.
    /// The named form must agree with the constants form, or a grid that
    /// varies the thresholds is not measuring production's rules.
    #[test]
    fn naming_the_thresholds_defaults_to_the_shipped_ones() {
        let clusters: BTreeMap<Cluster, Embedding> = [
            (Cluster(0), embedding(&[1.0, 0.0, 0.0])),
            (Cluster(1), embedding(&[0.0, 1.0, 0.0])),
            (Cluster(2), embedding(&[0.70, 0.71, 0.0])),
        ]
        .into_iter()
        .collect();
        let seeds = vec![
            SeedVoice {
                model: "test".into(),
                model_version: "1".into(),
                speaker_id: "alice".into(),
                vector: vec![1.0, 0.0, 0.0],
                confirmed: false,
            },
            SeedVoice {
                model: "test".into(),
                model_version: "1".into(),
                speaker_id: "bob".into(),
                vector: vec![0.0, 1.0, 0.0],
                confirmed: false,
            },
        ];
        assert_eq!(
            resolve_with(&clusters, &seeds, MATCH_FLOOR, MATCH_MARGIN),
            resolve(&clusters, &seeds)
        );
    }

    #[test]
    fn a_forbidden_pair_is_still_refused_when_a_third_cluster_would_carry_them_together() {
        let unit = |vector: &[f32]| embedding(vector);
        let windows: BTreeMap<Cluster, Embedding> = [
            (Cluster(0), unit(&[1.0, 0.1, 0.0])),
            (Cluster(1), unit(&[1.0, -0.1, 0.0])),
            (Cluster(2), unit(&[1.0, 0.0, 0.0])),
        ]
        .into_iter()
        .collect();

        // Unconstrained, all three are one voice: each outer window is
        // within the threshold of the middle one, and of the group after
        // it absorbs the other.
        let free = agglomerate_with(&windows, 0.95);
        assert_eq!(
            free.values().copied().collect::<BTreeSet<_>>().len(),
            1,
            "without the constraint the three collapse through the middle window"
        );

        let forbidden: BTreeMap<Cluster, BTreeSet<Cluster>> = [
            (Cluster(0), BTreeSet::from([Cluster(1)])),
            (Cluster(1), BTreeSet::from([Cluster(0)])),
        ]
        .into_iter()
        .collect();
        let held = agglomerate_constrained(&windows, 0.95, &forbidden);
        assert_ne!(
            held[&Cluster(0)],
            held[&Cluster(1)],
            "a forbidden pair must not arrive in one cluster by way of a third"
        );
    }

    /// The constraint has to hold in the second stage too, where the things
    /// being compared are block centroids rather than windows.
    #[test]
    fn a_constraint_across_two_blocks_survives_the_second_stage() {
        // Two voices over two full blocks, so every voice appears in both
        // and it is the second stage that puts each one back together.
        let windows = many_windows(2, BLOCK * 2);
        let one_voice = |merged: &BTreeMap<Cluster, Cluster>, of: Cluster| {
            merged
                .iter()
                .filter(|(member, _)| member.0 % 2 == of.0 % 2)
                .map(|(_, canonical)| *canonical)
                .collect::<BTreeSet<_>>()
                .len()
        };

        let free = agglomerate_with(&windows, MERGE_THRESHOLD);
        assert_eq!(one_voice(&free, Cluster(0)), 1, "one voice, one cluster");

        // One window from each block, same voice — a pair the second stage
        // is exactly what would otherwise reunite.
        // The first window of the second block: `chunks(BLOCK)` splits the
        // 2 * BLOCK windows there, and it is even, so it is the same voice
        // as Cluster(0).
        let across = BLOCK as u32;
        let forbidden: BTreeMap<Cluster, BTreeSet<Cluster>> = [
            (Cluster(0), BTreeSet::from([Cluster(across)])),
            (Cluster(across), BTreeSet::from([Cluster(0)])),
        ]
        .into_iter()
        .collect();
        let held = agglomerate_constrained(&windows, MERGE_THRESHOLD, &forbidden);
        assert_eq!(
            one_voice(&held, Cluster(0)),
            2,
            "the blocks of that voice must stay apart in the second stage"
        );
        assert_eq!(
            one_voice(&held, Cluster(1)),
            1,
            "the untouched voice is unaffected"
        );
    }

    /// No constraints must mean no change, or the experiment measures the
    /// rewrite instead of the constraint.
    #[test]
    fn clustering_with_no_constraints_is_the_clustering_we_already_had() {
        let empty = BTreeMap::new();
        for (voices, count) in [(2, 40), (3, 200), (4, BLOCK + 50)] {
            let windows = many_windows(voices, count);
            for threshold in [0.30_f32, 0.60, 0.65, 0.90] {
                assert_eq!(
                    agglomerate_constrained(&windows, threshold, &empty),
                    agglomerate_with(&windows, threshold),
                    "{voices} voices, {count} windows, threshold {threshold}"
                );
            }
        }
    }

    #[test]
    fn a_meetings_worth_of_windows_clusters_in_seconds_rather_than_minutes() {
        // The whole reason for the two stages. 12,000 windows is roughly what
        // a two-hour meeting produces once the segmentation window slides at
        // one second instead of hopping by ten — and what the single cubic
        // pass could not do at all. The assertion is the *answer*; the
        // timing is only meaningful because a cubic pass over this input
        // would not finish inside anyone's patience.
        let windows = many_windows(4, 12_000);
        let started = std::time::Instant::now();
        let merged = agglomerate(&windows);
        let elapsed = started.elapsed();

        let groups: BTreeSet<Cluster> = merged.values().copied().collect();
        assert_eq!(
            groups.len(),
            4,
            "four voices went in and {} came out",
            groups.len()
        );
        assert_eq!(merged.len(), 12_000, "every window is placed");
        assert!(
            elapsed.as_secs() < 60,
            "clustering took {elapsed:?}, which is the ceiling this ticket exists to remove"
        );
    }

    #[test]
    fn two_stages_find_the_same_voices_as_one() {
        // The second stage exists to put back together what blocking split
        // up: the same speaker appears in every block, as a separate group
        // in each, and must come out as one. Asserted by construction rather
        // than by timing — one block's worth against several.
        let one_block = agglomerate(&many_windows(3, BLOCK - 1));
        let many_blocks = agglomerate(&many_windows(3, BLOCK * 3));

        let count = |merged: &BTreeMap<Cluster, Cluster>| {
            merged
                .values()
                .copied()
                .collect::<BTreeSet<Cluster>>()
                .len()
        };
        assert_eq!(count(&one_block), 3);
        assert_eq!(
            count(&many_blocks),
            3,
            "blocking must not turn one voice into one voice per block"
        );
    }

    #[test]
    fn a_voice_heard_only_in_a_later_block_is_still_its_own_speaker() {
        // The failure blocking could introduce and a single pass could not:
        // someone who arrives late lives entirely inside one block, and a
        // second stage that compared only block-to-block averages would
        // fold them into whoever dominates that block.
        let mut windows = many_windows(2, BLOCK * 2);
        // A third voice, present only in the final stretch.
        for index in (BLOCK * 2)..(BLOCK * 2 + 40) {
            windows.insert(Cluster(index as u32), embedding(&[0.0, 0.0, 1.0]));
        }
        let merged = agglomerate(&windows);
        let groups: BTreeSet<Cluster> = merged.values().copied().collect();
        assert_eq!(groups.len(), 3, "the late arrival is their own voice");
    }
}
