//! Turning a Meeting into a Summary, whichever Backend runs it.
//!
//! The part that decides *what* to ask for, independent of who answers. Two
//! things here are load-bearing.
//!
//! **Map-reduce, because meetings are long.** A ninety-minute meeting does
//! not fit in a small local model's context, and the failure mode of
//! ignoring that is not an error — it is a summary of the first ten minutes,
//! presented as a summary of the meeting. Chunks are split on sentence
//! boundaries with overlap, and a chunk that fails is tolerated: five
//! sixths of a meeting summarized is worth having, and the run fails only
//! if every chunk did.
//!
//! **The transcript is rendered with timestamps and speakers**, because the
//! output contract asks each action item to cite where it came from. A model
//! cannot cite what it was never shown.

use evertranscript_protocol::TranscriptSegment;

/// Roughly how many tokens a string is worth (catalog M4: chars × 0.35).
///
/// An estimate on purpose. The real count depends on the tokenizer, which
/// depends on the model, which the Backend seam deliberately hides — and
/// being wrong by 20% here costs a slightly smaller chunk, while asking the
/// Backend would cost the seam.
pub fn estimate_tokens(text: &str) -> usize {
    (text.chars().count() as f32 * 0.35) as usize
}

/// Below this, one pass (catalog M4).
pub const SINGLE_PASS_TOKENS: usize = 4_000;

/// Chunk size, leaving the catalog's headroom for the prompt itself.
pub const CHUNK_TOKENS: usize = SINGLE_PASS_TOKENS - 300;

/// How much each chunk repeats of the one before it.
///
/// A commitment made across a chunk boundary would otherwise be invisible to
/// both halves — the "I'll send it Friday" that follows "can you handle the
/// numbers?" in the next chunk.
pub const OVERLAP_TOKENS: usize = 100;

/// A Meeting, as the model is shown it.
pub struct Material<'a> {
    pub segments: &'a [TranscriptSegment],
    /// Speaker id → display name. Missing ids render as the channel, which
    /// is what an undiarized Meeting has.
    pub speaker_names: &'a dyn Fn(&str) -> Option<String>,
    pub notes: Option<&'a str>,
}

/// Renders the Transcript as lines a model can cite.
///
/// `[12:34] Alice: the words` — the timestamp is what makes rule 5's "Said
/// at" column possible, and the name is what stops every action item being
/// attributed to whoever spoke most.
pub fn render_transcript(material: &Material<'_>) -> String {
    let mut out = String::new();
    for segment in material.segments {
        let who = segment
            .speaker_id
            .as_deref()
            .and_then(material.speaker_names)
            .unwrap_or_else(|| match segment.channel {
                evertranscript_protocol::AudioChannel::Mic => "You".to_string(),
                evertranscript_protocol::AudioChannel::System => "Participant".to_string(),
            });
        out.push_str(&format!(
            "[{}] {who}: {}\n",
            timestamp(segment.start_ms),
            segment.text.trim()
        ));
    }
    out
}

fn timestamp(ms: i64) -> String {
    let total = (ms.max(0) / 1000) as u64;
    let (hours, minutes, seconds) = (total / 3600, (total % 3600) / 60, total % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}

/// Splits a rendered transcript into overlapping chunks on line boundaries.
///
/// Lines rather than sentences, and that is a deliberate narrowing of the
/// catalog's "sentence-boundary splits": a rendered transcript is already
/// one utterance per line, so a line boundary *is* a sentence boundary here
/// and it additionally never splits a speaker attribution away from the
/// words it labels. Splitting mid-line could hand a chunk a quotation with
/// no idea who said it.
pub fn chunk(transcript: &str) -> Vec<String> {
    if estimate_tokens(transcript) <= SINGLE_PASS_TOKENS {
        return vec![transcript.to_string()];
    }

    let mut chunks = Vec::new();
    let lines: Vec<&str> = transcript.lines().collect();
    let mut start = 0;

    while start < lines.len() {
        let mut end = start;
        let mut tokens = 0;
        while end < lines.len() {
            let next = estimate_tokens(lines[end]) + 1;
            // Always take at least one line, or a single line longer than a
            // whole chunk would loop forever.
            if tokens + next > CHUNK_TOKENS && end > start {
                break;
            }
            tokens += next;
            end += 1;
        }
        chunks.push(lines[start..end].join("\n"));

        if end >= lines.len() {
            break;
        }
        // Step back far enough to repeat roughly OVERLAP_TOKENS of context.
        let mut back = end;
        let mut carried = 0;
        while back > start + 1 && carried < OVERLAP_TOKENS {
            back -= 1;
            carried += estimate_tokens(lines[back]) + 1;
        }
        start = back;
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;
    use evertranscript_protocol::AudioChannel;

    fn segment(id: &str, start_ms: i64, speaker: Option<&str>, text: &str) -> TranscriptSegment {
        TranscriptSegment {
            id: id.into(),
            sequence: 0,
            channel: AudioChannel::System,
            start_ms,
            end_ms: start_ms + 1_000,
            text: text.into(),
            speaker_id: speaker.map(str::to_string),
            attribution: None,
        }
    }

    fn no_names(_: &str) -> Option<String> {
        None
    }

    fn material<'a>(segments: &'a [TranscriptSegment]) -> Material<'a> {
        Material {
            segments,
            speaker_names: &no_names,
            notes: None,
        }
    }

    #[test]
    fn the_transcript_carries_timestamps_so_items_can_be_cited() {
        // Rule 5 asks each action item to say where it came from. A model
        // cannot cite what it was never shown.
        let segments = vec![segment("a", 754_000, None, "I'll send it Friday")];
        let rendered = render_transcript(&material(&segments));
        assert!(rendered.contains("[12:34]"), "got {rendered}");
        assert!(rendered.contains("I'll send it Friday"));
    }

    #[test]
    fn a_long_meeting_shows_hours() {
        let segments = vec![segment("a", 3_723_000, None, "still going")];
        assert!(render_transcript(&material(&segments)).contains("[1:02:03]"));
    }

    #[test]
    fn speakers_are_named_so_items_are_not_all_attributed_to_one_person() {
        let segments = vec![segment("a", 0, Some("s1"), "I'll do it")];
        let names = |id: &str| (id == "s1").then(|| "Alice".to_string());
        let rendered = render_transcript(&Material {
            segments: &segments,
            speaker_names: &names,
            notes: None,
        });
        assert!(rendered.contains("Alice: I'll do it"));
    }

    #[test]
    fn an_undiarized_meeting_still_distinguishes_the_two_channels() {
        // M1 and M2 Meetings have no Speakers at all. "You" and
        // "Participant" is less than diarization gives, and much better than
        // one undifferentiated wall.
        let segments = vec![
            TranscriptSegment {
                channel: AudioChannel::Mic,
                ..segment("a", 0, None, "shall we start")
            },
            segment("b", 2_000, None, "yes"),
        ];
        let rendered = render_transcript(&material(&segments));
        assert!(rendered.contains("You: shall we start"));
        assert!(rendered.contains("Participant: yes"));
    }

    fn long_meeting() -> Vec<TranscriptSegment> {
        (0..1_200)
            .map(|index| {
                segment(
                    &format!("s{index}"),
                    index as i64 * 5_000,
                    None,
                    "a sentence of roughly average length spoken in a meeting",
                )
            })
            .collect()
    }

    #[test]
    fn a_long_meeting_is_chunked_rather_than_truncated() {
        // The failure this exists to prevent is not an error. It is a
        // summary of the first ten minutes, presented as a summary of the
        // meeting.
        let segments = long_meeting();
        let rendered = render_transcript(&material(&segments));
        let chunks = chunk(&rendered);
        assert!(chunks.len() > 1, "got {} chunk(s)", chunks.len());
        assert!(
            chunks
                .iter()
                .all(|piece| estimate_tokens(piece) <= CHUNK_TOKENS + 200),
            "a chunk exceeded the budget"
        );
    }

    #[test]
    fn chunks_overlap_so_nothing_falls_between_them() {
        // A commitment made across a boundary would otherwise be invisible
        // to both halves.
        let segments = long_meeting();
        let chunks = chunk(&render_transcript(&material(&segments)));
        let first_end: Vec<&str> = chunks[0].lines().rev().take(3).collect();
        let second_start: Vec<&str> = chunks[1].lines().take(10).collect();
        assert!(
            first_end.iter().any(|line| second_start.contains(line)),
            "no overlap between consecutive chunks"
        );
    }

    #[test]
    fn every_line_of_the_meeting_appears_in_some_chunk() {
        // Chunking that dropped the middle would be invisible in the output
        // — the summary would simply be missing things nobody noticed.
        let segments = long_meeting();
        let rendered = render_transcript(&material(&segments));
        let chunks = chunk(&rendered);
        let joined = chunks.join("\n");
        for line in rendered.lines() {
            assert!(joined.contains(line), "lost a line: {line}");
        }
    }
}
