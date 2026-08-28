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
pub fn render(meeting: &Meeting, segments: &[TranscriptSegment]) -> String {
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
    out.push_str(&format!("speakers: [{}]\n", speaker_list(segments)));
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
    out.push_str("*Not generated yet.*\n\n");

    out.push_str("## Notes\n\n");
    out.push_str("*None yet.*\n\n");

    out.push_str("## Transcript\n\n");
    if segments.is_empty() {
        out.push_str("*No transcript yet.*\n");
    } else {
        for segment in segments {
            out.push_str(&format!(
                "**{}** ({}) {}\n\n",
                channel_label(segment.channel),
                format_timestamp(segment.start_ms),
                segment.text.trim()
            ));
        }
    }
    out
}

/// Until Diarization runs (M3), the channel is the attribution we honestly
/// have: the mic channel is where the Operator is, the system channel is
/// everyone else (ADR-0029 as amended).
fn channel_label(channel: AudioChannel) -> &'static str {
    match channel {
        AudioChannel::Mic => "You",
        AudioChannel::System => "Participants",
    }
}

fn speaker_list(segments: &[TranscriptSegment]) -> String {
    let mut labels: Vec<&str> = Vec::new();
    for segment in segments {
        let label = channel_label(segment.channel);
        if !labels.contains(&label) {
            labels.push(label);
        }
    }
    labels.join(", ")
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
                Ok(Some((meeting, segments)))
            })
            .await?;

        // The Meeting was deleted between being marked dirty and now.
        let Some((meeting, segments)) = loaded else {
            return Ok(());
        };

        let markdown = render(&meeting, &segments);
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
            id: "0199a1b2-c3d4-7e5f-8901-234567890abc".to_string(),
            started_at: "2026-08-27T10:02:00+08:00".to_string(),
            ended_at: Some("2026-08-27T10:49:00+08:00".to_string()),
            title: None,
            detected_app: Some("Zoom".to_string()),
            duration_seconds: Some(2820),
            mirror_filename: None,
            audio_path: Some(".data/audio/0199a1b2.m4a".to_string()),
            audio_notes: Vec::new(),
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
        let rendered = render(&armed, &[]);
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
        let rendered = render(&meeting, &[]);

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
        assert!(!render(&meeting(), &[]).contains("incomplete"));
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
        let rendered = render(&meeting(), &[]);
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
            },
            TranscriptSegment {
                id: "b".into(),
                sequence: 1,
                channel: AudioChannel::System,
                start_ms: 3_732_000,
                end_ms: 3_734_000,
                text: "yes, go ahead".into(),
                speaker_id: None,
            },
        ];
        let rendered = render(&meeting(), &segments);
        assert!(rendered.contains("**You** (00:12) shall we start"));
        assert!(rendered.contains("**Participants** (01:02:12) yes, go ahead"));
        assert!(rendered.contains("speakers: [You, Participants]"));
    }
}
