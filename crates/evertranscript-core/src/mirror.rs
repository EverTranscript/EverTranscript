//! The Markdown Mirror: a regenerable projection of the record (ADR-0005).
//!
//! One file per Meeting at the top of the History folder, so the folder reads
//! as a folder of meeting notes to a human, to Obsidian, and to grep. It is
//! never hand-edited — Operator writing lives in Notes, which regenerate into
//! it — and it is never the source of truth.
//!
//! Regeneration is driven by the trigger-fed dirty queue rather than by
//! callers remembering to ask, which is what keeps it from silently drifting
//! out of step with the database.

use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use evertranscript_protocol::AudioChannel;
use evertranscript_protocol::Meeting;
use evertranscript_protocol::TranscriptSegment;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use tracing::debug;
use tracing::warn;

use crate::store::Store;
use crate::store::meetings;

/// How long to let writes settle before rebuilding. Long enough that a burst
/// of transcript segments produces one rebuild, short enough to feel live.
const DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(250);

/// How many Meetings to rebuild per pass.
const BATCH: u32 = 64;

/// The first 8 hex characters of the Meeting id: the durable marker in every
/// Mirror filename. Retitles rename the file; this is what survives, so it is
/// what outside references should key on.
pub fn id8(meeting_id: &str) -> String {
    meeting_id
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .take(8)
        .collect()
}

/// Turns a title into a filename-safe slug, keeping native characters.
///
/// Chinese titles stay Chinese: APFS and NTFS handle them, and transliterating
/// a Meeting called 预算评审 into ASCII would make the folder less legible to
/// the person whose meetings they are.
pub fn slugify(input: &str) -> String {
    let mut slug = String::new();
    let mut pending_separator = false;
    for character in input.chars() {
        if character.is_alphanumeric() {
            if pending_separator && !slug.is_empty() {
                slug.push('-');
            }
            pending_separator = false;
            for lowered in character.to_lowercase() {
                slug.push(lowered);
            }
        } else {
            pending_separator = true;
        }
    }
    // Filenames have limits and long titles are common; 60 characters keeps
    // the whole name comfortably short without truncating most titles.
    slug.chars()
        .take(60)
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

/// `YYYY-MM-DD-<slug>-<id8>.md` (ADR-0035 as amended).
pub fn filename(meeting: &Meeting) -> String {
    let date = meetings::local_date(&meeting.started_at);
    let source = meeting
        .title
        .as_deref()
        .filter(|title| !title.trim().is_empty())
        .or(meeting.detected_app.as_deref())
        .unwrap_or("meeting");
    let slug = {
        let slug = slugify(source);
        if slug.is_empty() {
            "meeting".to_string()
        } else {
            slug
        }
    };
    format!("{date}-{slug}-{}.md", id8(&meeting.id))
}

/// Renders the Meeting as its Mirror.
///
/// Section order is skim-first: the material that gets reread is at the top,
/// the longest section last. Sections that have not been produced yet say so
/// rather than being absent, so the shape of a Meeting's file never changes
/// as post-processing lands.
pub fn render(meeting: &Meeting, segments: &[TranscriptSegment], names: &SpeakerNames) -> String {
    let mut out = String::new();

    out.push_str("---\n");
    out.push_str(&format!("id: {}\n", meeting.id));
    out.push_str(&format!("date: {}\n", meeting.started_at));
    if let Some(app) = &meeting.detected_app {
        out.push_str(&format!("app: {app}\n"));
    }
    if let Some(duration) = meeting.duration_seconds {
        out.push_str(&format!("duration: {}\n", format_duration(duration)));
    }
    out.push_str(&format!("speakers: [{}]\n", speaker_list(segments, names)));
    // What the calendar knew (ADR-0036). The event id makes the Meeting
    // traceable back to the entry that armed it; the attendees are a record
    // of who was *invited*, which is not the same claim as who spoke and is
    // never rendered as one.
    if let Some(event) = &meeting.calendar_event_id {
        out.push_str(&format!("calendar_event: {event}\n"));
    }
    if !meeting.calendar_attendees.is_empty() {
        out.push_str(&format!(
            "invited: [{}]\n",
            meeting.calendar_attendees.join(", ")
        ));
    }
    if let Some(audio) = &meeting.audio_path {
        // In-app playback is a non-goal; the path is how "any player serves"
        // stays a real sentence now that audio lives in a hidden folder.
        out.push_str(&format!("audio: {audio}\n"));
    }
    out.push_str("---\n\n");

    let title = meeting
        .title
        .as_deref()
        .filter(|title| !title.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| fallback_title(meeting));
    out.push_str(&format!("# {title}\n\n"));

    // Directly under the title, before anything derived from the audio,
    // because everything below is only as complete as the capture was. A
    // Meeting that recorded one side of a conversation must not read like a
    // Meeting where one side stayed quiet.
    if !meeting.audio_notes.is_empty() {
        out.push_str("> **This recording is incomplete.**\n");
        for note in &meeting.audio_notes {
            out.push_str(&format!("> - {note}\n"));
        }
        out.push('\n');
    }

    out.push_str("## Summary\n\n");
    // Before the Summary, for the same reason the capture notes sit before
    // everything derived from the audio: what follows is only as complete as
    // the run that produced it, and a partial Summary must not read like a
    // complete one.
    if let Some(gaps) = meeting
        .summary_gaps
        .as_deref()
        .map(str::trim)
        .filter(|gaps| !gaps.is_empty())
    {
        out.push_str(&format!("> **This Summary is incomplete.** {gaps}\n\n"));
    }
    match meeting
        .summary
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(summary) => {
            out.push_str(summary);
            out.push_str("\n\n");
        }
        None => out.push_str("*Not generated yet.*\n\n"),
    }

    out.push_str("## Notes\n\n");
    // The Operator's own writing, reproduced exactly. No trimming beyond
    // the edges, no reflowing, no fixing of their markdown: this is the one
    // part of the file that is theirs rather than the product's.
    match meeting
        .notes
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(notes) => {
            out.push_str(notes);
            out.push_str("\n\n");
        }
        None => out.push_str("*None yet.*\n\n"),
    }

    out.push_str("## Transcript\n\n");
    if segments.is_empty() {
        out.push_str("*No transcript yet.*\n");
    } else {
        for (segment, label) in segments.iter().zip(labels_for(segments, names)) {
            out.push_str(&format!(
                "**{}** ({}) {}\n\n",
                label,
                format_timestamp(segment.start_ms),
                segment.text.trim()
            ));
        }
    }
    out
}

/// Every Speaker's name, for the renderer.
///
/// All of them rather than only this Meeting's: the map is small, and
/// filtering it would mean a correction that introduced a Speaker between
/// the segment query and this one rendered as a pseudonym.
fn speaker_names(connection: &rusqlite::Connection) -> anyhow::Result<SpeakerNames> {
    let entries = crate::store::speakers::list(connection)?
        .into_iter()
        .map(|speaker| {
            (
                speaker.id,
                SpeakerName {
                    display_name: speaker.display_name,
                    is_operator: speaker.is_operator,
                },
            )
        })
        .collect();
    Ok(SpeakerNames::from_entries(entries))
}

/// What a Meeting's Speakers are called, for rendering.
///
/// Passed in rather than looked up here because the Mirror is a pure
/// projection (ADR-0005): given the same Meeting, segments and names it
/// must produce the same bytes, and a renderer that could reach the
/// database would be one whose output depended on when it ran.
#[derive(Debug, Clone, Default)]
pub struct SpeakerNames {
    entries: BTreeMap<String, SpeakerName>,
}

#[derive(Debug, Clone)]
pub struct SpeakerName {
    pub display_name: Option<String>,
    pub is_operator: bool,
}

impl SpeakerNames {
    pub fn from_entries(entries: BTreeMap<String, SpeakerName>) -> Self {
        Self { entries }
    }

    fn get(&self, id: &str) -> Option<&SpeakerName> {
        self.entries.get(id)
    }
}

/// Before Diarization has run, the channel is the attribution we honestly
/// have: the mic channel is where the Operator is, the system channel is
/// everyone else (ADR-0029 as amended).
fn channel_label(channel: AudioChannel) -> &'static str {
    match channel {
        AudioChannel::Mic => "You",
        AudioChannel::System => "Participants",
    }
}

/// What each segment is labelled, in order.
///
/// Unnamed Speakers get numbered pseudonyms assigned by order of first
/// appearance in this Meeting. Stable for a given transcript, and
/// deliberately not stored: a persisted "Speaker 3" would look to the
/// Operator like a name somebody chose, and would then be wrong the moment
/// a different Meeting numbered its voices differently.
fn labels_for(segments: &[TranscriptSegment], names: &SpeakerNames) -> Vec<String> {
    let mut pseudonyms: BTreeMap<&str, usize> = BTreeMap::new();
    let mut next = 1;
    let mut labels = Vec::with_capacity(segments.len());

    // Whether Diarization has run over this Meeting at all. Found by
    // dogfooding: before it runs, "You" on the mic channel is the honest
    // best guess. *After* it runs, an unattributed mic segment means
    // Diarization looked and found no voice there — and still calling it
    // "You" put the Operator in one transcript under two different names,
    // their own and the channel's. Not knowing is a different claim from
    // not having looked, and the Mirror should not blur them.
    let diarized = segments
        .iter()
        .any(|segment| segment.attribution.is_some() || segment.speaker_id.is_some());

    for segment in segments {
        let label = match segment.speaker_id.as_deref() {
            None if diarized => "Unattributed".to_string(),
            None => channel_label(segment.channel).to_string(),
            Some(id) => match names.get(id) {
                Some(SpeakerName {
                    display_name: Some(name),
                    ..
                }) => name.clone(),
                // The Operator's own Speaker, unnamed: "You" is what ADR-0029
                // says to call them, and it is a display name rather than a
                // stored one so renaming still works.
                Some(SpeakerName {
                    is_operator: true, ..
                }) => "You".to_string(),
                _ => {
                    let number = *pseudonyms.entry(id).or_insert_with(|| {
                        let assigned = next;
                        next += 1;
                        assigned
                    });
                    format!("Speaker {number}")
                }
            },
        };
        labels.push(label);
    }
    labels
}

fn speaker_list(segments: &[TranscriptSegment], names: &SpeakerNames) -> String {
    let mut seen: Vec<String> = Vec::new();
    for label in labels_for(segments, names) {
        if !seen.contains(&label) {
            seen.push(label);
        }
    }
    seen.join(", ")
}

fn fallback_title(meeting: &Meeting) -> String {
    let date = meetings::local_date(&meeting.started_at);
    match &meeting.detected_app {
        Some(app) => format!("{app}, {date}"),
        None => format!("Meeting, {date}"),
    }
}

fn format_duration(seconds: u64) -> String {
    let (hours, minutes) = (seconds / 3600, (seconds % 3600) / 60);
    if hours > 0 {
        format!("{hours}h {minutes}m")
    } else if minutes > 0 {
        format!("{minutes}m")
    } else {
        format!("{seconds}s")
    }
}

fn format_timestamp(milliseconds: i64) -> String {
    let total = (milliseconds.max(0) / 1000) as u64;
    let (hours, minutes, seconds) = (total / 3600, (total % 3600) / 60, total % 60);
    if hours > 0 {
        format!("{hours:02}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}

/// Writes Mirrors for whatever the dirty queue names.
#[derive(Clone)]
pub struct MirrorWriter {
    store: Store,
    history_dir: PathBuf,
}

impl MirrorWriter {
    pub fn new(store: Store, history_dir: PathBuf) -> Self {
        Self { store, history_dir }
    }

    /// Runs until shutdown, rebuilding whenever something is dirty.
    pub async fn run(self, wake: Arc<Notify>, shutdown: CancellationToken) {
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                _ = wake.notified() => {}
                _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {}
            }
            // Let a burst of writes settle into a single rebuild.
            tokio::time::sleep(DEBOUNCE).await;
            if let Err(error) = self.rebuild_pending().await {
                warn!(%error, "Mirror rebuild failed; will retry");
            }
        }
        debug!("mirror writer finished");
    }

    /// Rebuilds every Mirror currently marked dirty. Returns how many.
    pub async fn rebuild_pending(&self) -> Result<usize> {
        let dirty = self
            .store
            .read(move |connection| meetings::dirty_meetings(connection, BATCH))
            .await?;

        let mut rebuilt = 0;
        for (meeting_id, generation) in dirty {
            match self.rebuild_one(&meeting_id, generation).await {
                Ok(()) => rebuilt += 1,
                Err(error) => warn!(meeting_id, %error, "could not rebuild this Mirror"),
            }
        }
        Ok(rebuilt)
    }

    async fn rebuild_one(&self, meeting_id: &str, generation: i64) -> Result<()> {
        let id = meeting_id.to_string();
        let loaded = self
            .store
            .read(move |connection| {
                let Some(meeting) = meetings::get(connection, &id)? else {
                    return Ok(None);
                };
                let segments = meetings::segments(connection, &id)?;
                let names = speaker_names(connection)?;
                Ok(Some((meeting, segments, names)))
            })
            .await?;

        // The Meeting was deleted between being marked dirty and now.
        let Some((meeting, segments, names)) = loaded else {
            return Ok(());
        };

        let markdown = render(&meeting, &segments, &names);
        let next_filename = filename(&meeting);
        let destination = self.history_dir.join(&next_filename);
        write_atomically(&destination, &markdown)?;

        // A retitle changes the filename; remove the name the Mirror used to
        // have so the folder never accumulates stale copies of one Meeting.
        if let Some(previous) = meeting.mirror_filename.as_deref()
            && previous != next_filename
        {
            let stale = self.history_dir.join(previous);
            if let Err(error) = std::fs::remove_file(&stale)
                && error.kind() != std::io::ErrorKind::NotFound
            {
                warn!(path = %stale.display(), %error, "could not remove the stale Mirror");
            }
        }

        let body = segments
            .iter()
            .map(|segment| segment.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let title = meeting
            .title
            .clone()
            .unwrap_or_else(|| fallback_title(&meeting));
        let id = meeting.id.clone();

        self.store
            .write(move |connection| {
                let transaction = connection.transaction()?;
                meetings::set_mirror_filename(&transaction, &id, &next_filename)?;
                meetings::reindex(&transaction, &id, &title, &body)?;
                meetings::acknowledge(&transaction, &id, generation)?;
                transaction.commit()?;
                Ok(())
            })
            .await?;
        Ok(())
    }

    /// Removes a Meeting's Mirror from disk.
    pub fn remove(&self, mirror_filename: &str) {
        let path = self.history_dir.join(mirror_filename);
        if let Err(error) = std::fs::remove_file(&path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            warn!(path = %path.display(), %error, "could not remove the Mirror");
        }
    }

    pub fn history_dir(&self) -> &Path {
        &self.history_dir
    }
}

/// Writes through a temporary file so a reader never sees a half-written
/// Mirror — the folder is synced and watched by other tools.
fn write_atomically(destination: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = destination.with_extension("md.tmp");
    std::fs::write(&temporary, contents)?;
    std::fs::rename(&temporary, destination)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meeting() -> Meeting {
        Meeting {
            notes: None,
            summary: None,
            summary_backend: None,
            summary_generated_at: None,
            id: "0199a1b2-c3d4-7e5f-8901-234567890abc".to_string(),
            started_at: "2026-08-27T10:02:00+08:00".to_string(),
            ended_at: Some("2026-08-27T10:49:00+08:00".to_string()),
            title: None,
            detected_app: Some("Zoom".to_string()),
            duration_seconds: Some(2820),
            mirror_filename: None,
            audio_path: Some(".data/audio/0199a1b2.m4a".to_string()),
            audio_notes: Vec::new(),
            summary_gaps: None,
            calendar_event_id: None,
            calendar_attendees: Vec::new(),
        }
    }

    #[test]
    fn a_calendar_armed_meeting_records_which_event_and_who_was_invited() {
        // ADR-0036: the event id makes the Meeting traceable back to the
        // entry that armed it, and the attendees are who was *invited* —
        // never rendered as who spoke, which is M3's question and needs
        // Diarization to answer.
        let armed = Meeting {
            calendar_event_id: Some("evt-42".to_string()),
            calendar_attendees: vec!["Ada".to_string(), "Grace".to_string()],
            ..meeting()
        };
        let rendered = render(&armed, &[], &SpeakerNames::default());
        assert!(rendered.contains("calendar_event: evt-42"), "{rendered}");
        assert!(rendered.contains("invited: [Ada, Grace]"), "{rendered}");
        assert!(
            !rendered.contains("speakers: [Ada"),
            "an invitation is not an attribution"
        );
    }

    #[test]
    fn an_incomplete_recording_says_so_above_everything_derived_from_it() {
        // The Operator reads the Mirror, not the log. A Meeting that captured
        // one side of a conversation and a Meeting where one side stayed
        // quiet produce the same transcript, and only this tells them apart.
        let mut meeting = meeting();
        meeting.audio_notes = vec![
            "system audio: permission to record system audio has not been granted".to_string(),
        ];
        let rendered = render(&meeting, &[], &SpeakerNames::default());

        let warning = rendered
            .find("This recording is incomplete")
            .expect("an incomplete recording must say so");
        assert!(
            warning < rendered.find("## Summary").expect("a summary section"),
            "the warning belongs above what was derived from the missing audio"
        );
        assert!(
            rendered.contains("permission to record system audio"),
            "and must carry the reason, not just the fact"
        );
    }

    #[test]
    fn a_whole_recording_is_not_littered_with_reassurance() {
        // The note appears only when something was lost; a clean Meeting
        // says nothing, or the warning stops meaning anything.
        assert!(!render(&meeting(), &[], &SpeakerNames::default()).contains("incomplete"));
    }

    #[test]
    fn an_untitled_meeting_is_named_for_its_app_and_date() {
        assert_eq!(filename(&meeting()), "2026-08-27-zoom-0199a1b2.md");
    }

    #[test]
    fn titling_a_meeting_renames_its_mirror_but_keeps_the_id8() {
        let mut meeting = meeting();
        meeting.title = Some("Frank / Jack Sync-Up".to_string());
        assert_eq!(
            filename(&meeting),
            "2026-08-27-frank-jack-sync-up-0199a1b2.md"
        );
    }

    #[test]
    fn chinese_titles_stay_chinese() {
        let mut meeting = meeting();
        meeting.title = Some("预算评审 Q3".to_string());
        assert_eq!(filename(&meeting), "2026-08-27-预算评审-q3-0199a1b2.md");
    }

    #[test]
    fn a_title_of_only_punctuation_still_produces_a_usable_name() {
        let mut meeting = meeting();
        meeting.title = Some("!!! ??? ...".to_string());
        assert_eq!(filename(&meeting), "2026-08-27-meeting-0199a1b2.md");
    }

    #[test]
    fn the_mirror_has_every_section_even_before_post_processing() {
        let rendered = render(&meeting(), &[], &SpeakerNames::default());
        assert!(rendered.starts_with("---\n"), "frontmatter comes first");
        assert!(rendered.contains("id: 0199a1b2-c3d4-7e5f-8901-234567890abc"));
        assert!(rendered.contains("audio: .data/audio/0199a1b2.m4a"));
        assert!(rendered.contains("# Zoom, 2026-08-27"));
        assert!(rendered.contains("## Summary"));
        assert!(rendered.contains("## Notes"));
        assert!(rendered.contains("## Transcript"));
        assert!(rendered.contains("*No transcript yet.*"));

        // Skim-first order is the point of the layout.
        let summary = rendered.find("## Summary").expect("summary");
        let notes = rendered.find("## Notes").expect("notes");
        let transcript = rendered.find("## Transcript").expect("transcript");
        assert!(summary < notes && notes < transcript);
    }

    fn attributed(
        id: &str,
        sequence: i64,
        channel: AudioChannel,
        speaker: &str,
    ) -> TranscriptSegment {
        TranscriptSegment {
            id: id.into(),
            sequence,
            channel,
            start_ms: sequence * 1_000,
            end_ms: sequence * 1_000 + 900,
            text: format!("line {sequence}"),
            speaker_id: Some(speaker.into()),
            attribution: None,
        }
    }

    fn names(entries: &[(&str, Option<&str>, bool)]) -> SpeakerNames {
        SpeakerNames::from_entries(
            entries
                .iter()
                .map(|(id, name, is_operator)| {
                    (
                        (*id).to_string(),
                        SpeakerName {
                            display_name: name.map(str::to_string),
                            is_operator: *is_operator,
                        },
                    )
                })
                .collect(),
        )
    }

    #[test]
    fn a_named_speaker_is_rendered_by_name() {
        // Story 29's visible half: the rename has to reach the folder the
        // Operator actually reads, not just the database.
        let segments = vec![attributed("a", 0, AudioChannel::System, "s1")];
        let rendered = render(
            &meeting(),
            &segments,
            &names(&[("s1", Some("Alice"), false)]),
        );
        assert!(rendered.contains("**Alice**"));
        assert!(rendered.contains("speakers: [Alice]"));
    }

    #[test]
    fn unnamed_speakers_are_numbered_by_first_appearance() {
        // Stable within the Meeting, and never stored: a persisted
        // "Speaker 2" would read as a name somebody chose, and would be
        // wrong the moment another Meeting numbered its voices differently.
        let segments = vec![
            attributed("a", 0, AudioChannel::System, "zzz"),
            attributed("b", 1, AudioChannel::System, "aaa"),
            attributed("c", 2, AudioChannel::System, "zzz"),
        ];
        let rendered = render(
            &meeting(),
            &segments,
            &names(&[("zzz", None, false), ("aaa", None, false)]),
        );
        // First heard is Speaker 1, even though its id sorts last.
        assert!(rendered.contains("**Speaker 1** (00:00) line 0"));
        assert!(rendered.contains("**Speaker 2** (00:01) line 1"));
        assert!(rendered.contains("**Speaker 1** (00:02) line 2"));
    }

    #[test]
    fn the_operators_own_speaker_is_you_until_they_rename_it() {
        // ADR-0029 as amended: "You" is a display name, not a magic record.
        let segments = vec![attributed("a", 0, AudioChannel::Mic, "me")];
        assert!(
            render(&meeting(), &segments, &names(&[("me", None, true)])).contains("**You**"),
            "unnamed Operator reads as You"
        );
        assert!(
            render(
                &meeting(),
                &segments,
                &names(&[("me", Some("Frank"), true)])
            )
            .contains("**Frank**"),
            "and their chosen name wins over it"
        );
    }

    #[test]
    fn after_diarization_an_unattributed_segment_is_not_called_you() {
        // Found by dogfooding the real recording: with some segments
        // attributed to a named Speaker and others not, the Operator
        // appeared in one transcript under two names — "Frank" where
        // Diarization spoke, "You" where it had not. Once Diarization has
        // run, silence about a segment is a finding, not a channel guess.
        let segments = vec![
            attributed("a", 0, AudioChannel::Mic, "me"),
            TranscriptSegment {
                id: "b".into(),
                sequence: 1,
                channel: AudioChannel::Mic,
                start_ms: 1_000,
                end_ms: 1_900,
                text: "unattributed".into(),
                speaker_id: None,
                attribution: None,
            },
        ];
        let rendered = render(
            &meeting(),
            &segments,
            &names(&[("me", Some("Frank"), true)]),
        );
        assert!(rendered.contains("**Frank**"));
        assert!(
            !rendered.contains("**You**"),
            "one person must not appear under two names:\n{rendered}"
        );
        assert!(rendered.contains("**Unattributed**"));
    }

    #[test]
    fn unattributed_segments_still_fall_back_to_the_channel() {
        // Every Meeting recorded before M3, and every Meeting whose models
        // were missing. The channel is the honest attribution we have; a
        // fabricated "Speaker 1" would claim knowledge nobody has.
        let segments = vec![TranscriptSegment {
            id: "a".into(),
            sequence: 0,
            channel: AudioChannel::Mic,
            start_ms: 0,
            end_ms: 900,
            text: "hello".into(),
            speaker_id: None,
            attribution: None,
        }];
        let rendered = render(&meeting(), &segments, &SpeakerNames::default());
        assert!(rendered.contains("**You**"));
    }

    #[test]
    fn notes_and_summary_replace_their_placeholders() {
        // Both headings have said "*Not generated yet.*" and "*None yet.*"
        // in every Mirror since M1. This is the milestone where they stop.
        let mut meeting = meeting();
        meeting.summary = Some("# Budget review\n\nWe deferred hiring.".into());
        meeting.notes = Some("ask about the Q4 number".into());

        let rendered = render(&meeting, &[], &SpeakerNames::default());
        assert!(rendered.contains("We deferred hiring."));
        assert!(rendered.contains("ask about the Q4 number"));
        assert!(!rendered.contains("*Not generated yet.*"));
        assert!(!rendered.contains("*None yet.*"));
    }

    #[test]
    fn the_operators_notes_are_reproduced_exactly() {
        // Their writing, not the product's. No reflowing, no tidying of
        // their markdown, no "helpful" normalisation — a Mirror that edits
        // someone's prose is a Mirror they stop trusting.
        let awkward = "- one\n\n\n-   two   \n\t- three  *unclosed";
        let mut meeting = meeting();
        meeting.notes = Some(awkward.into());
        assert!(render(&meeting, &[], &SpeakerNames::default()).contains(awkward));
    }

    #[test]
    fn whitespace_only_notes_read_as_empty_rather_than_as_content() {
        // An Operator who opened the pane and typed nothing has no notes.
        let mut meeting = meeting();
        meeting.notes = Some("   \n\t\n  ".into());
        assert!(render(&meeting, &[], &SpeakerNames::default()).contains("*None yet.*"));
    }

    #[test]
    fn transcript_segments_render_with_speaker_and_timestamp() {
        let segments = vec![
            TranscriptSegment {
                id: "a".into(),
                sequence: 0,
                channel: AudioChannel::Mic,
                start_ms: 12_000,
                end_ms: 14_000,
                text: "shall we start".into(),
                speaker_id: None,
                attribution: None,
            },
            TranscriptSegment {
                id: "b".into(),
                sequence: 1,
                channel: AudioChannel::System,
                start_ms: 3_732_000,
                end_ms: 3_734_000,
                text: "yes, go ahead".into(),
                speaker_id: None,
                attribution: None,
            },
        ];
        let rendered = render(&meeting(), &segments, &SpeakerNames::default());
        assert!(rendered.contains("**You** (00:12) shall we start"));
        assert!(rendered.contains("**Participants** (01:02:12) yes, go ahead"));
        assert!(rendered.contains("speakers: [You, Participants]"));
    }
}
