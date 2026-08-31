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

use super::Cluster;
use super::Embedding;

/// How similar two voices must be before they can be the same person.
///
/// From the catalog's reference numbers. Deliberately not tuned here: a
/// threshold moved without the DER measurement in ticket 09 is a threshold
/// moved on vibes.
pub const MATCH_FLOOR: f32 = 0.62;

/// How far the best candidate must beat the runner-up.
///
/// The condition that makes two similar voices produce *no* match rather
/// than a coin-flip between them. Without it, the closer two colleagues
/// sound, the more confidently the system mislabels them.
pub const MATCH_MARGIN: f32 = 0.08;

/// Agglomerative merge threshold on L2-normalized embeddings (catalog M3).
pub const MERGE_THRESHOLD: f32 = 0.6;

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
    let mut resolved = BTreeMap::new();
    if seeds.is_empty() {
        for cluster in clusters.keys() {
            resolved.insert(*cluster, Resolved::New);
        }
        return resolved;
    }

    // Every pairwise score once. Cheap — clusters are single digits and
    // seeds are bounded — and it makes the mutual-best check a lookup rather
    // than a second pass over the models.
    let scores: BTreeMap<(Cluster, usize), f32> = clusters
        .iter()
        .flat_map(|(cluster, embedding)| {
            seeds.iter().enumerate().map(move |(index, seed)| {
                ((*cluster, index), cosine(&embedding.vector, &seed.vector))
            })
        })
        .collect();

    for cluster in clusters.keys() {
        let mut ranked: Vec<(usize, f32)> = seeds
            .iter()
            .enumerate()
            .map(|(index, _)| (index, scores[&(*cluster, index)]))
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

        let clears_floor = best_score >= MATCH_FLOOR;

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
        let clears_margin = (best_score - runner_up) >= MATCH_MARGIN || confirmation_breaks_the_tie;
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
        .max_by(|a, b| scores[&(**a, seed_index)].total_cmp(&scores[&(**b, seed_index)]))
        .copied()
}

/// Merges clusters that are the same voice.
///
/// Agglomerative, single pass, at [`MERGE_THRESHOLD`]. Over-segmentation is
/// the failure this repairs: a clusterer that splits one person into two is
/// commonplace, and the Operator sees it as a stranger in their own meeting.
pub fn agglomerate(embeddings: &BTreeMap<Cluster, Embedding>) -> BTreeMap<Cluster, Cluster> {
    let mut canonical: BTreeMap<Cluster, Cluster> = BTreeMap::new();
    for (cluster, embedding) in embeddings {
        // Join the first canonical cluster close enough to be the same
        // voice; the ordering is deterministic because the map is sorted.
        let joined = canonical
            .values()
            .copied()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .find(|other| {
                embeddings.get(other).is_some_and(|theirs| {
                    cosine(&embedding.vector, &theirs.vector) >= MERGE_THRESHOLD
                })
            });
        canonical.insert(*cluster, joined.unwrap_or(*cluster));
    }
    canonical
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

/// Every voice History can offer this Meeting's clusterer.
pub fn seeds(connection: &rusqlite::Connection) -> anyhow::Result<Vec<SeedVoice>> {
    Ok(crate::store::speakers::voiceprints(connection)?
        .into_iter()
        .map(|(speaker_id, vector, confirmed)| SeedVoice {
            speaker_id,
            vector,
            confirmed,
        })
        .collect())
}

/// Resolves a Meeting's clusters to persistent Speakers.
///
/// Recognized clusters return their existing Speaker; unrecognized ones get
/// a new Speaker with no name, which is what "every voice resolves to a
/// persistent Speaker, named or not" means in ADR-0008. Either way the new
/// observation is folded back in as an exemplar and the Voiceprint is
/// recomputed, so the next Meeting's clusterer is seeded with a slightly
/// better picture than this one was — the improvement ADR-0008 promises.
pub fn persist(
    connection: &rusqlite::Connection,
    meeting_id: &str,
    embeddings: &BTreeMap<Cluster, Embedding>,
) -> anyhow::Result<BTreeMap<Cluster, String>> {
    use crate::store::speakers;

    let known = seeds(connection)?;
    let resolved = resolve(embeddings, &known);
    let mut assigned = BTreeMap::new();

    for (cluster, outcome) in resolved {
        let Some(embedding) = embeddings.get(&cluster) else {
            continue;
        };
        let speaker_id = match outcome {
            Resolved::Existing(id) => id,
            Resolved::New => speakers::create(connection, false)?.id,
        };

        speakers::add_exemplar(
            connection,
            speakers::NewExemplar {
                speaker_id: &speaker_id,
                meeting_id: Some(meeting_id),
                vector: &embedding.vector,
                model: &embedding.model,
                model_version: &embedding.model_version,
                // The seam does not carry voiced duration per cluster yet;
                // an equal weight is the honest placeholder rather than a
                // fabricated one, and ticket 03 supplies the real figure
                // when the pipeline that measures it exists.
                voiced_ms: 1,
                from_operator: false,
                is_negative: false,
            },
        )?;

        let history: Vec<(Vec<f32>, i64, bool)> = speakers::exemplars(connection, &speaker_id)?
            .into_iter()
            .map(|exemplar| (exemplar.vector, exemplar.voiced_ms, exemplar.is_negative))
            .collect();
        if let Some(vector) = centroid(&history) {
            speakers::set_voiceprint(
                connection,
                &speaker_id,
                &vector,
                &embedding.model,
                &embedding.model_version,
            )?;
        }
        assigned.insert(cluster, speaker_id);
    }
    Ok(assigned)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn embedding(vector: &[f32]) -> Embedding {
        Embedding::new(vector.to_vec(), "test", "1")
    }

    fn clusters(entries: &[(u32, &[f32])]) -> BTreeMap<Cluster, Embedding> {
        entries
            .iter()
            .map(|(index, vector)| (Cluster(*index), embedding(vector)))
            .collect()
    }

    fn seed(id: &str, vector: &[f32], confirmed: bool) -> SeedVoice {
        SeedVoice {
            speaker_id: id.into(),
            vector: vector.to_vec(),
            confirmed,
        }
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

    #[test]
    fn the_same_voice_in_two_meetings_is_one_speaker() {
        // Story 28, end to end through the record. This is the assertion the
        // whole of Voiceprint storage exists to make true, and it has to be
        // a test rather than a demo.
        use crate::store::meetings;
        let connection = db();

        let monday = meetings::start(&connection, Some("Monday"), None).expect("m1");
        let first = clusters(&[(0, &[1.0, 0.0, 0.0]), (1, &[0.0, 1.0, 0.0])]);
        let monday_map = persist(&connection, &monday.id, &first).expect("persist");
        assert_eq!(monday_map.len(), 2, "two new voices");

        let friday = meetings::start(&connection, Some("Friday"), None).expect("m2");
        // The same first voice, heard slightly differently — a different
        // microphone, a different room.
        let second = clusters(&[(0, &[0.97, 0.05, 0.0])]);
        let friday_map = persist(&connection, &friday.id, &second).expect("persist");

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
        persist(&connection, &monday.id, &clusters(&[(0, &[1.0, 0.0, 0.0])])).expect("persist");

        let friday = meetings::start(&connection, None, None).expect("m2");
        persist(&connection, &friday.id, &clusters(&[(0, &[0.0, 0.0, 1.0])])).expect("persist");

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
        let map =
            persist(&connection, &monday.id, &clusters(&[(0, &[1.0, 0.0, 0.0])])).expect("persist");
        let speaker_id = map[&Cluster(0)].clone();
        assert_eq!(
            crate::store::speakers::exemplars(&connection, &speaker_id)
                .expect("exemplars")
                .len(),
            1
        );

        let friday = meetings::start(&connection, None, None).expect("m2");
        persist(
            &connection,
            &friday.id,
            &clusters(&[(0, &[0.97, 0.05, 0.0])]),
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
    fn a_deleted_voiceprint_stops_seeding_future_meetings() {
        // Story 31's real consequence: deletion has to actually stop
        // recognition, not just blank a column. If the vector kept seeding
        // the clusterer the Operator's act would be cosmetic.
        use crate::store::meetings;
        let connection = db();

        let monday = meetings::start(&connection, None, None).expect("m1");
        let map =
            persist(&connection, &monday.id, &clusters(&[(0, &[1.0, 0.0, 0.0])])).expect("persist");
        let speaker_id = map[&Cluster(0)].clone();

        crate::store::speakers::delete_voiceprint(&connection, &speaker_id).expect("delete");
        assert!(seeds(&connection).expect("seeds").is_empty());

        let friday = meetings::start(&connection, None, None).expect("m2");
        let after =
            persist(&connection, &friday.id, &clusters(&[(0, &[1.0, 0.0, 0.0])])).expect("persist");
        assert_ne!(
            after[&Cluster(0)],
            speaker_id,
            "the same voice is now a stranger, which is what deletion means"
        );
    }
}
