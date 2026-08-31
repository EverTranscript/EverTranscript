//! Attribution meets an already-published Transcript.
//!
//! Diarization is post-meeting, so by the time it has an opinion the
//! Transcript already exists: segments are written, Clients have rendered
//! them, and Mirrors are on disk. This module is the join that maps one onto
//! the other, and it is deliberately small — because the thing that makes it
//! correct is not the algorithm but the fact that both sides are measured on
//! the same clock (ADR-0029). Two clocks would turn a lookup with one right
//! answer into a correlation problem with a plausible one.
//!
//! **Midpoint-in-turn**, not overlap-majority. The catalog specifies it and
//! the reason is worth keeping: diarization's characteristic error is a turn
//! boundary landing a little early or late, and a midpoint is stable under
//! exactly that error while a majority-overlap rule flips whenever the
//! boundary crosses the halfway mark of a segment.
//!
//! The honest part is [`Reconciliation::boundary_flips`]. Where an ASR
//! segment straddles a turn boundary, the midpoint rule is not deducing an
//! answer — it is picking one. Counting those is how anyone later can tell a
//! transcript that was attributed from one that was guessed at.

use evertranscript_protocol::AudioChannel;
use evertranscript_protocol::TranscriptSegment;

use super::Cluster;
use super::Diarization;
use crate::audio::CaptureOffset;

/// One segment's attribution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assignment {
    pub segment_id: String,
    /// None where nobody was speaking at the midpoint. Silence belongs to
    /// nobody, and inventing an owner for it would attribute a pause to
    /// whoever spoke last.
    pub cluster: Option<Cluster>,
    /// True when this segment straddles a turn boundary, so the midpoint
    /// chose between two candidates rather than finding one.
    pub straddles_boundary: bool,
}

/// What the join concluded, and how much of it was guessed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Reconciliation {
    pub assignments: Vec<Assignment>,
    /// How many segments spanned a turn boundary.
    ///
    /// A quality metric, kept rather than hidden. It is the number of places
    /// where the answer depended on the rule rather than on the evidence,
    /// and a run where it is large is a run whose attribution should be
    /// trusted less — which is exactly the kind of thing a product that
    /// stores biometrics owes its Operator.
    pub boundary_flips: usize,
}

impl Reconciliation {
    /// How many segments got a Speaker at all.
    pub fn attributed(&self) -> usize {
        self.assignments
            .iter()
            .filter(|assignment| assignment.cluster.is_some())
            .count()
    }
}

/// Midpoint of a segment on the capture clock.
///
/// Averaged as `start + (end - start) / 2` rather than `(start + end) / 2`:
/// the second overflows on a long meeting's millisecond timestamps far
/// sooner than anyone expects it to, and this is the sort of arithmetic that
/// is only ever wrong on the recording somebody cared about.
fn midpoint(start_ms: i64, end_ms: i64) -> CaptureOffset {
    let start = start_ms.max(0) as u64;
    let end = end_ms.max(0) as u64;
    if end <= start {
        return CaptureOffset(start);
    }
    CaptureOffset(start + (end - start) / 2)
}

/// Attributes every segment to the voice that owned its midpoint.
pub fn reconcile(diarization: &Diarization, segments: &[TranscriptSegment]) -> Reconciliation {
    let mut assignments = Vec::with_capacity(segments.len());
    let mut boundary_flips = 0;

    for segment in segments {
        let channel = segment.channel;
        let at = midpoint(segment.start_ms, segment.end_ms);
        let cluster = diarization.turn_at(channel, at).map(|turn| turn.cluster);

        // Whether the rule had to choose. Comparing the two ends rather than
        // measuring distance to a boundary, because a segment that begins in
        // one voice and ends in another is precisely the case where the
        // midpoint is an arbitration.
        let at_start = cluster_at(diarization, channel, segment.start_ms);
        let at_end = cluster_at(diarization, channel, segment.end_ms.saturating_sub(1));
        let straddles_boundary = at_start != at_end;
        if straddles_boundary {
            boundary_flips += 1;
        }

        assignments.push(Assignment {
            segment_id: segment.id.clone(),
            cluster,
            straddles_boundary,
        });
    }

    Reconciliation {
        assignments,
        boundary_flips,
    }
}

fn cluster_at(diarization: &Diarization, channel: AudioChannel, at_ms: i64) -> Option<Cluster> {
    diarization
        .turn_at(channel, CaptureOffset(at_ms.max(0) as u64))
        .map(|turn| turn.cluster)
}

/// Writes a reconciliation onto a Meeting's segments.
///
/// Two properties matter more than the mechanics.
///
/// **It re-maps a Transcript that already exists.** Diarization is
/// post-meeting, so these segments have been written, sent to Clients, and
/// rendered into a Mirror. Attribution is an update to published rows, which
/// is why `segments_after_update` dirties the Mirror and why the caller
/// raises notifications rather than leaving Clients to discover it on reload.
///
/// **It never touches a correction hint.** Re-running Diarization — after a
/// model upgrade, or because the first run was interrupted — replaces the
/// machine's conclusion and leaves the Operator's above it (ADR-0009 as
/// amended). Corrections are the one thing in this table that a machine did
/// not produce, and re-diarization is exactly the moment they would
/// otherwise be lost.
pub fn apply(
    connection: &rusqlite::Connection,
    reconciliation: &Reconciliation,
    speakers: &std::collections::BTreeMap<Cluster, String>,
    basis: crate::store::speakers::Attribution,
) -> anyhow::Result<usize> {
    let mut written = 0;
    for assignment in &reconciliation.assignments {
        let speaker_id = assignment
            .cluster
            .and_then(|cluster| speakers.get(&cluster))
            .map(String::as_str);
        crate::store::speakers::attribute_segment(
            connection,
            &assignment.segment_id,
            speaker_id,
            basis,
        )?;
        if speaker_id.is_some() {
            written += 1;
        }
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diarize::Turn;
    use std::collections::BTreeMap;

    fn segment(id: &str, channel: AudioChannel, start_ms: i64, end_ms: i64) -> TranscriptSegment {
        TranscriptSegment {
            id: id.into(),
            sequence: 0,
            channel,
            start_ms,
            end_ms,
            text: "words".into(),
            speaker_id: None,
            attribution: None,
        }
    }

    fn diarization(turns: Vec<Turn>) -> Diarization {
        Diarization {
            turns,
            embeddings: BTreeMap::new(),
        }
    }

    #[test]
    fn a_segment_takes_the_voice_that_owned_its_midpoint() {
        let d = diarization(vec![
            Turn::new(AudioChannel::System, 0, 5_000, 0),
            Turn::new(AudioChannel::System, 5_000, 10_000, 1),
        ]);
        let segments = vec![
            segment("a", AudioChannel::System, 1_000, 2_000),
            segment("b", AudioChannel::System, 7_000, 8_000),
        ];
        let result = reconcile(&d, &segments);
        assert_eq!(result.assignments[0].cluster, Some(Cluster(0)));
        assert_eq!(result.assignments[1].cluster, Some(Cluster(1)));
        assert_eq!(result.boundary_flips, 0);
    }

    #[test]
    fn a_straddling_segment_is_counted_as_a_guess() {
        // The whole reason the metric exists. This segment begins in one
        // voice and ends in another; the midpoint picks, and the count says
        // so rather than presenting the pick as a deduction.
        let d = diarization(vec![
            Turn::new(AudioChannel::System, 0, 5_000, 0),
            Turn::new(AudioChannel::System, 5_000, 10_000, 1),
        ]);
        let segments = vec![segment("a", AudioChannel::System, 4_000, 6_000)];
        let result = reconcile(&d, &segments);
        assert_eq!(result.boundary_flips, 1);
        assert!(result.assignments[0].straddles_boundary);
        assert_eq!(
            result.assignments[0].cluster,
            Some(Cluster(1)),
            "midpoint 5000 lands in the second turn, half-open"
        );
    }

    #[test]
    fn a_boundary_that_moves_slightly_does_not_change_a_clean_segment() {
        // Why midpoint beats overlap-majority. Diarization's characteristic
        // error is a boundary landing a bit early or late; the attribution
        // of a segment well inside a turn must not depend on it.
        let segments = vec![segment("a", AudioChannel::System, 1_000, 3_000)];
        for boundary in [3_100, 3_500, 3_900] {
            let d = diarization(vec![
                Turn::new(AudioChannel::System, 0, boundary, 0),
                Turn::new(AudioChannel::System, boundary, 10_000, 1),
            ]);
            assert_eq!(
                reconcile(&d, &segments).assignments[0].cluster,
                Some(Cluster(0)),
                "boundary at {boundary} should not move this segment"
            );
        }
    }

    #[test]
    fn silence_leaves_a_segment_unattributed() {
        // A segment in a gap belongs to nobody. Attributing it to whoever
        // spoke last would put words in a real person's mouth.
        let d = diarization(vec![
            Turn::new(AudioChannel::System, 0, 1_000, 0),
            Turn::new(AudioChannel::System, 8_000, 9_000, 0),
        ]);
        let segments = vec![segment("a", AudioChannel::System, 4_000, 5_000)];
        let result = reconcile(&d, &segments);
        assert_eq!(result.assignments[0].cluster, None);
        assert_eq!(result.attributed(), 0);
    }

    #[test]
    fn the_channel_is_part_of_the_lookup() {
        // The far end and the room can speak at the same instant. A join
        // that ignored the channel would give the remote speaker's words to
        // whoever was in the room, which is the failure diarizing both
        // channels exists to prevent (ADR-0029 as amended).
        let d = diarization(vec![
            Turn::new(AudioChannel::Mic, 0, 10_000, 0),
            Turn::new(AudioChannel::System, 0, 10_000, 1),
        ]);
        let segments = vec![
            segment("mic", AudioChannel::Mic, 2_000, 3_000),
            segment("sys", AudioChannel::System, 2_000, 3_000),
        ];
        let result = reconcile(&d, &segments);
        assert_eq!(result.assignments[0].cluster, Some(Cluster(0)));
        assert_eq!(result.assignments[1].cluster, Some(Cluster(1)));
    }

    #[test]
    fn a_zero_length_segment_still_gets_an_answer() {
        // ASR does emit these. `end <= start` must not underflow into a
        // midpoint near the end of time.
        let d = diarization(vec![Turn::new(AudioChannel::Mic, 0, 5_000, 3)]);
        let segments = vec![segment("a", AudioChannel::Mic, 2_000, 2_000)];
        assert_eq!(
            reconcile(&d, &segments).assignments[0].cluster,
            Some(Cluster(3))
        );
    }

    #[test]
    fn a_long_meeting_does_not_overflow_the_midpoint() {
        // `(start + end) / 2` is the tempting form and it is wrong for
        // timestamps near i64::MAX. This is the sort of arithmetic that only
        // ever breaks on the recording somebody cared about.
        let huge = i64::MAX - 1;
        assert_eq!(midpoint(huge - 2, huge), CaptureOffset((huge - 1) as u64));
    }

    #[test]
    fn overlapped_speech_resolves_to_one_voice_rather_than_none() {
        // Two turns cover the midpoint. Attribution has to name one — a
        // transcript segment has one speaker label — and the honest signal
        // that it was contested is the boundary count, not a null.
        let d = diarization(vec![
            Turn::new(AudioChannel::System, 0, 6_000, 0),
            Turn::new(AudioChannel::System, 4_000, 9_000, 1),
        ]);
        let segments = vec![segment("a", AudioChannel::System, 4_500, 5_500)];
        let result = reconcile(&d, &segments);
        assert!(result.assignments[0].cluster.is_some());
    }

    #[test]
    fn an_empty_diarization_attributes_nothing_and_claims_nothing() {
        // The unavailable-model path (ticket 03). Every segment keeps its
        // pre-Diarization state rather than the Meeting failing.
        let segments = vec![
            segment("a", AudioChannel::Mic, 0, 1_000),
            segment("b", AudioChannel::System, 1_000, 2_000),
        ];
        let result = reconcile(&Diarization::default(), &segments);
        assert_eq!(result.attributed(), 0);
        assert_eq!(result.boundary_flips, 0);
        assert_eq!(
            result.assignments.len(),
            2,
            "every segment is accounted for"
        );
    }

    #[test]
    fn applying_attribution_updates_a_published_transcript() {
        use crate::store::meetings;
        use crate::store::speakers as speaker_store;

        let mut connection = rusqlite::Connection::open_in_memory().expect("open");
        crate::store::schema::migrate(&mut connection).expect("migrate");
        let meeting = meetings::start(&connection, Some("Standup"), None).expect("meeting");

        // The Transcript exists first. That is the whole situation this
        // module is for: diarization arrives after the words did.
        let published = meetings::append_segment(
            &connection,
            &meeting.id,
            AudioChannel::System,
            0,
            4_000,
            "good morning",
        )
        .expect("segment");
        assert!(published.speaker_id.is_none());

        let speaker = speaker_store::create(&connection, false).expect("speaker");
        let d = diarization(vec![Turn::new(AudioChannel::System, 0, 10_000, 0)]);
        let all = meetings::segments(&connection, &meeting.id).expect("segments");
        let result = reconcile(&d, &all);

        let mut map = std::collections::BTreeMap::new();
        map.insert(Cluster(0), speaker.id.clone());
        let written = apply(
            &connection,
            &result,
            &map,
            speaker_store::Attribution::Clustered,
        )
        .expect("apply");

        assert_eq!(written, 1);
        let after = meetings::segments(&connection, &meeting.id).expect("segments");
        assert_eq!(after[0].speaker_id.as_deref(), Some(speaker.id.as_str()));
    }

    #[test]
    fn re_running_diarization_preserves_the_operators_corrections() {
        // ADR-0009 as amended. A model upgrade or an interrupted first run
        // must not silently discard the one thing in the record a machine
        // did not produce.
        use crate::store::meetings;
        use crate::store::speakers as speaker_store;

        let mut connection = rusqlite::Connection::open_in_memory().expect("open");
        crate::store::schema::migrate(&mut connection).expect("migrate");
        let meeting = meetings::start(&connection, None, None).expect("meeting");
        meetings::append_segment(
            &connection,
            &meeting.id,
            AudioChannel::System,
            0,
            4_000,
            "hi",
        )
        .expect("segment");

        let machine = speaker_store::create(&connection, false).expect("machine");
        let corrected_to = speaker_store::create(&connection, false).expect("operator");
        let all = meetings::segments(&connection, &meeting.id).expect("segments");
        let segment_id = all[0].id.clone();

        let d = diarization(vec![Turn::new(AudioChannel::System, 0, 10_000, 0)]);
        let mut map = std::collections::BTreeMap::new();
        map.insert(Cluster(0), machine.id.clone());
        apply(
            &connection,
            &reconcile(&d, &all),
            &map,
            speaker_store::Attribution::Clustered,
        )
        .expect("first run");

        speaker_store::correct_attribution(&connection, &segment_id, &corrected_to.id)
            .expect("correct");

        // Re-run, reaching a different machine conclusion.
        let other = speaker_store::create(&connection, false).expect("other");
        let mut second = std::collections::BTreeMap::new();
        second.insert(Cluster(0), other.id.clone());
        apply(
            &connection,
            &reconcile(&d, &all),
            &second,
            speaker_store::Attribution::Voiceprint,
        )
        .expect("second run");

        let after = meetings::segments(&connection, &meeting.id).expect("segments");
        assert_eq!(
            after[0].speaker_id.as_deref(),
            Some(corrected_to.id.as_str()),
            "the Operator's correction still wins"
        );

        let beneath: Option<String> = connection
            .query_row(
                "SELECT speaker_id FROM transcript_segments WHERE id = ?1",
                rusqlite::params![segment_id],
                |row| row.get(0),
            )
            .expect("raw");
        assert_eq!(
            beneath,
            Some(other.id),
            "and the machine's new conclusion was still recorded beneath it"
        );
    }

    #[test]
    fn a_partially_applied_run_leaves_a_coherent_record() {
        // Cancellation mid-apply. Some segments attributed and some not is
        // an acceptable record; a half-written row is not. Every segment is
        // either its old value or its new one.
        use crate::store::meetings;
        use crate::store::speakers as speaker_store;

        let mut connection = rusqlite::Connection::open_in_memory().expect("open");
        crate::store::schema::migrate(&mut connection).expect("migrate");
        let meeting = meetings::start(&connection, None, None).expect("meeting");
        for index in 0..4 {
            meetings::append_segment(
                &connection,
                &meeting.id,
                AudioChannel::System,
                index * 1_000,
                index * 1_000 + 900,
                "words",
            )
            .expect("segment");
        }

        let speaker = speaker_store::create(&connection, false).expect("speaker");
        let d = diarization(vec![Turn::new(AudioChannel::System, 0, 10_000, 0)]);
        let all = meetings::segments(&connection, &meeting.id).expect("segments");
        let full = reconcile(&d, &all);

        // Stop after two, as a cancelled job would.
        let partial = Reconciliation {
            assignments: full.assignments[..2].to_vec(),
            boundary_flips: 0,
        };
        let mut map = std::collections::BTreeMap::new();
        map.insert(Cluster(0), speaker.id.clone());
        apply(
            &connection,
            &partial,
            &map,
            speaker_store::Attribution::Clustered,
        )
        .expect("apply");

        let after = meetings::segments(&connection, &meeting.id).expect("segments");
        assert_eq!(after.len(), 4, "no segment was lost");
        assert_eq!(
            after.iter().filter(|s| s.speaker_id.is_some()).count(),
            2,
            "attributed as far as it got"
        );
        assert!(
            after[2..].iter().all(|s| s.speaker_id.is_none()),
            "and the rest are plainly unattributed rather than wrong"
        );
    }

    #[test]
    fn a_meeting_killed_mid_diarization_reopens_coherent() {
        // The crash criterion. In-memory coherence is not the claim worth
        // making — the claim is that a Core killed while attributing leaves
        // a database the *next* Core can open and read correctly.
        use crate::store::meetings;
        use crate::store::speakers as speaker_store;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("record.db");

        let (meeting_id, speaker_id, segment_ids) = {
            let mut connection = rusqlite::Connection::open(&path).expect("open");
            crate::store::schema::migrate(&mut connection).expect("migrate");
            let meeting = meetings::start(&connection, Some("Killed"), None).expect("meeting");
            for index in 0..6 {
                meetings::append_segment(
                    &connection,
                    &meeting.id,
                    AudioChannel::System,
                    index * 1_000,
                    index * 1_000 + 900,
                    "words",
                )
                .expect("segment");
            }
            let speaker = speaker_store::create(&connection, false).expect("speaker");
            let all = meetings::segments(&connection, &meeting.id).expect("segments");
            let ids: Vec<String> = all.iter().map(|s| s.id.clone()).collect();

            let d = diarization(vec![Turn::new(AudioChannel::System, 0, 10_000, 0)]);
            let full = reconcile(&d, &all);
            // Half of the work, then the process goes away.
            let partial = Reconciliation {
                assignments: full.assignments[..3].to_vec(),
                boundary_flips: 0,
            };
            let mut map = std::collections::BTreeMap::new();
            map.insert(Cluster(0), speaker.id.clone());
            apply(
                &connection,
                &partial,
                &map,
                speaker_store::Attribution::Clustered,
            )
            .expect("apply");
            (meeting.id, speaker.id, ids)
            // `connection` drops here — the kill.
        };

        // A new Core, over the same History.
        let connection = rusqlite::Connection::open(&path).expect("reopen");
        let after = meetings::segments(&connection, &meeting_id).expect("segments");
        assert_eq!(after.len(), segment_ids.len(), "no segment was lost");

        let attributed = after.iter().filter(|s| s.speaker_id.is_some()).count();
        assert_eq!(attributed, 3, "what completed, survived");
        assert!(
            after[..3]
                .iter()
                .all(|s| s.speaker_id.as_deref() == Some(speaker_id.as_str())),
            "and is attributed to the right Speaker"
        );
        assert!(
            after[3..].iter().all(|s| s.speaker_id.is_none()),
            "the rest are plainly unattributed rather than half-written"
        );

        // And the Meeting is still diarizable: re-running is how the
        // Operator recovers from this, not a repair tool.
        let all = meetings::segments(&connection, &meeting_id).expect("segments");
        let d = diarization(vec![Turn::new(AudioChannel::System, 0, 10_000, 0)]);
        let mut map = std::collections::BTreeMap::new();
        map.insert(Cluster(0), speaker_id.clone());
        let written = apply(
            &connection,
            &reconcile(&d, &all),
            &map,
            speaker_store::Attribution::Clustered,
        )
        .expect("second run");
        assert_eq!(written, 6, "the whole Meeting attributes on a re-run");
    }
}
