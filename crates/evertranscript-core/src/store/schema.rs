//! The record's schema and its migrations.
//!
//! House rules, borrowed from anarlog's schema hygiene and kept deliberately:
//! every table is STRICT, every enum is a CHECK, and invariants that matter
//! are constraints rather than conventions — a rule the database enforces
//! cannot be forgotten by a future writer.

use rusqlite::Connection;

/// Ordered migrations. Append only; never edit a shipped one.
const MIGRATIONS: &[&str] = &[
    // 1 — the record: Meetings, their Transcript segments, and the Speakers
    // attribution will point at. Voiceprint columns exist from the start so
    // M3 adds behavior, not a table rewrite.
    r#"
    CREATE TABLE meetings (
        id               TEXT PRIMARY KEY NOT NULL,
        started_at       TEXT NOT NULL,
        ended_at         TEXT,
        title            TEXT,
        detected_app     TEXT,
        mirror_filename  TEXT,
        audio_path       TEXT,
        created_at       TEXT NOT NULL,
        updated_at       TEXT NOT NULL
    ) STRICT;

    CREATE TABLE speakers (
        id                       TEXT PRIMARY KEY NOT NULL,
        display_name             TEXT,
        is_operator              INTEGER NOT NULL DEFAULT 0 CHECK (is_operator IN (0, 1)),
        -- ADR-0035: the Voiceprint lives here, in the record, unencrypted, so
        -- copying the History folder moves recognition with it.
        voiceprint               BLOB,
        voiceprint_model         TEXT,
        voiceprint_model_version TEXT,
        -- Set when the Operator names the Speaker: naming is confirmation,
        -- and confirmed Voiceprints outrank unconfirmed ones when matching
        -- (ADR-0008 as amended).
        confirmed                INTEGER NOT NULL DEFAULT 0 CHECK (confirmed IN (0, 1)),
        created_at               TEXT NOT NULL
    ) STRICT;

    CREATE TABLE transcript_segments (
        id          TEXT PRIMARY KEY NOT NULL,
        meeting_id  TEXT NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
        sequence    INTEGER NOT NULL,
        channel     TEXT NOT NULL CHECK (channel IN ('mic', 'system')),
        start_ms    INTEGER NOT NULL,
        end_ms      INTEGER NOT NULL,
        text        TEXT NOT NULL,
        speaker_id  TEXT REFERENCES speakers(id) ON DELETE SET NULL,
        UNIQUE (meeting_id, sequence)
    ) STRICT;

    CREATE INDEX transcript_segments_meeting ON transcript_segments(meeting_id, start_ms);
    CREATE INDEX meetings_started_at ON meetings(started_at DESC);
    "#,
    // 2 — the Mirror projection queue. Triggers mark a Meeting dirty; one
    // worker rebuilds and acks. A write landing mid-rebuild bumps the
    // generation again, so the ack does not clear it and the Mirror is
    // rebuilt once more rather than silently going stale.
    r#"
    CREATE TABLE mirror_dirty (
        meeting_id              TEXT PRIMARY KEY NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
        generation              INTEGER NOT NULL DEFAULT 1,
        acknowledged_generation INTEGER NOT NULL DEFAULT 0
    ) STRICT;

    CREATE TRIGGER meetings_after_insert AFTER INSERT ON meetings BEGIN
        INSERT INTO mirror_dirty (meeting_id, generation) VALUES (NEW.id, 1)
        ON CONFLICT (meeting_id) DO UPDATE SET generation = generation + 1;
    END;

    -- Deliberately scoped with UPDATE OF: the worker itself writes
    -- mirror_filename, and a trigger that fired on that write would dirty the
    -- row it just cleaned and never settle.
    CREATE TRIGGER meetings_after_update
    AFTER UPDATE OF title, started_at, ended_at, detected_app, audio_path ON meetings BEGIN
        INSERT INTO mirror_dirty (meeting_id, generation) VALUES (NEW.id, 1)
        ON CONFLICT (meeting_id) DO UPDATE SET generation = generation + 1;
    END;

    CREATE TRIGGER segments_after_insert AFTER INSERT ON transcript_segments BEGIN
        INSERT INTO mirror_dirty (meeting_id, generation) VALUES (NEW.meeting_id, 1)
        ON CONFLICT (meeting_id) DO UPDATE SET generation = generation + 1;
    END;

    CREATE TRIGGER segments_after_update AFTER UPDATE ON transcript_segments BEGIN
        INSERT INTO mirror_dirty (meeting_id, generation) VALUES (NEW.meeting_id, 1)
        ON CONFLICT (meeting_id) DO UPDATE SET generation = generation + 1;
    END;
    "#,
    // 3 — full-text search over the same projection the Mirror renders, so
    // what you can find is exactly what you can read.
    r#"
    CREATE VIRTUAL TABLE search_index USING fts5(
        meeting_id UNINDEXED,
        title,
        body
    );
    "#,
    // 4 — what a recording lost, in the record rather than only in a log.
    // A Meeting captured with half its audio previously looked exactly like
    // a complete one; the Operator opened one-sided notes with nothing to
    // explain them. A JSON array of human-readable notes, empty when the
    // recording was whole.
    r#"
    ALTER TABLE meetings ADD COLUMN audio_notes TEXT;
    "#,
    // 5 — the Watchlist: what Meeting Detection watches on this machine
    // (ADR-0024, ADR-0030). In the machine store rather than the History
    // folder, like settings: the list describes this installation, and
    // copying History to a new machine must not carry it.
    //
    // The shipped defaults are seeded here rather than defaulted in code, so
    // that an empty table means the Operator removed everything and gets
    // exactly that — not a silent restoration of the defaults on next start.
    r#"
    CREATE TABLE watchlist (
        id    TEXT PRIMARY KEY NOT NULL,
        name  TEXT NOT NULL,
        kind  TEXT NOT NULL CHECK (kind IN ('process', 'browserMeetings'))
    ) STRICT;

    INSERT INTO watchlist (id, name, kind) VALUES
        ('us.zoom.xos',                'Zoom',            'process'),
        ('com.microsoft.teams2',       'Microsoft Teams', 'process'),
        ('com.tencent.meeting',        'VooV Meeting',    'process'),
        ('com.tencent.tencentmeeting', '腾讯会议',         'process'),
        ('browser-meetings',           'Browser Meetings','browserMeetings');
    "#,
    // 6 — what the calendar knew (ADR-0036). The title already rides the
    // Meeting; these are the two facts that would otherwise be lost: which
    // event it was, and who was invited. Attendees are *stored, not
    // applied* — they become Speaker-naming suggestions in M3, and turning
    // an invitation into an attribution before Diarization exists would be
    // inventing who spoke.
    r#"
    ALTER TABLE meetings ADD COLUMN calendar_event_id TEXT;
    ALTER TABLE meetings ADD COLUMN calendar_attendees TEXT;
    "#,
    // 7 — what Diarization keeps (M3).
    //
    // Migration 1 gave `speakers` a single `voiceprint` BLOB and nothing
    // ever wrote to it. One vector per Speaker cannot represent a voice
    // across a headset, a laptop mic and a conference phone, and ADR-0008
    // promises recognition that *improves* with every Meeting — which a
    // single overwritten vector cannot do. So the column stays as the
    // current best identity vector (what matching compares against) and the
    // observations it is built from become rows.
    //
    // Keeping the exemplars, rather than only their average, is what makes
    // two later operations possible at all: re-embedding from kept audio
    // after a model upgrade (ADR-0035's stated reason for the model columns),
    // and letting an Operator correction feed evidence back in (ADR-0009 as
    // amended) instead of being a display-only annotation.
    r#"
    CREATE TABLE speaker_exemplars (
        id               TEXT PRIMARY KEY NOT NULL,
        speaker_id       TEXT NOT NULL REFERENCES speakers(id) ON DELETE CASCADE,
        -- Where this observation came from, so a model upgrade can re-embed
        -- from the audio that is still on disk rather than discarding the
        -- Speaker and starting again.
        meeting_id       TEXT REFERENCES meetings(id) ON DELETE SET NULL,
        embedding        BLOB NOT NULL,
        model            TEXT NOT NULL,
        model_version    TEXT NOT NULL,
        -- How much voiced audio this was built from. Short spans are weaker
        -- evidence and the centroid weights them accordingly.
        voiced_ms        INTEGER NOT NULL,
        -- 'machine' when clustering produced it, 'operator' when a
        -- correction did. An Operator-sourced exemplar is the strongest
        -- evidence the system has about a voice.
        source           TEXT NOT NULL CHECK (source IN ('machine', 'operator')),
        -- Negative evidence: set when a correction took a segment *away*
        -- from this Speaker (ADR-0009 as amended feeds both directions).
        is_negative      INTEGER NOT NULL DEFAULT 0 CHECK (is_negative IN (0, 1)),
        created_at       TEXT NOT NULL
    ) STRICT;

    CREATE INDEX speaker_exemplars_speaker ON speaker_exemplars(speaker_id);

    -- ADR-0009 as amended: a correction is an appended hint. The machine's
    -- conclusion on `transcript_segments.speaker_id` is never overwritten,
    -- so the record stays auditable and re-diarization stays possible; the
    -- display join and the Mirrors prefer the newest hint.
    CREATE TABLE attribution_hints (
        id                  TEXT PRIMARY KEY NOT NULL,
        segment_id          TEXT NOT NULL REFERENCES transcript_segments(id) ON DELETE CASCADE,
        speaker_id          TEXT NOT NULL REFERENCES speakers(id) ON DELETE CASCADE,
        -- Who the machine had said. Kept so the correction is legible after
        -- the fact, and so a re-diarization that reaches a different
        -- conclusion can tell "the Operator disagreed with this attribution"
        -- from "the Operator disagreed with a different one".
        replaced_speaker_id TEXT REFERENCES speakers(id) ON DELETE SET NULL,
        created_at          TEXT NOT NULL
    ) STRICT;

    CREATE INDEX attribution_hints_segment ON attribution_hints(segment_id, created_at DESC);

    -- ADR-0008 makes visible match attribution a mandatory legibility
    -- surface, not a debugging aid: a biometric guess the Operator cannot
    -- interrogate is the thing that ADR bargained against. Null means the
    -- segment predates Diarization.
    ALTER TABLE transcript_segments ADD COLUMN attribution TEXT;

    -- Mirrors are regenerable projections, never independent files
    -- (ADR-0005, ADR-0009). Writing a segment's speaker already dirties its
    -- Meeting through `segments_after_update`, but the two acts that make
    -- Diarization worth having touch no segment at all: renaming a Speaker
    -- (story 29 — one rename relabels all of History) and correcting an
    -- attribution (story 29b). Without these, both would be correct in the
    -- database and invisible in the folder the Operator actually reads.
    CREATE TRIGGER speakers_after_rename
    AFTER UPDATE OF display_name ON speakers BEGIN
        INSERT INTO mirror_dirty (meeting_id, generation)
        SELECT DISTINCT segment.meeting_id, 1
          FROM transcript_segments segment
         WHERE segment.speaker_id = NEW.id
            OR segment.id IN (
                SELECT hint.segment_id FROM attribution_hints hint
                 WHERE hint.speaker_id = NEW.id
            )
        ON CONFLICT (meeting_id) DO UPDATE SET generation = generation + 1;
    END;

    CREATE TRIGGER attribution_hints_after_insert
    AFTER INSERT ON attribution_hints BEGIN
        INSERT INTO mirror_dirty (meeting_id, generation)
        SELECT segment.meeting_id, 1
          FROM transcript_segments segment
         WHERE segment.id = NEW.segment_id
        ON CONFLICT (meeting_id) DO UPDATE SET generation = generation + 1;
    END;
    "#,
    // 8 — Operator Notes and the Summary (M4).
    //
    // **These two columns are the only mutable content in the record, and
    // the distinction is worth stating where it lives.** ADR-0009 makes the
    // Transcript and its attribution immutable: they are what happened, and
    // a record that edits itself is the opposite of a legible guarantee.
    // ADR-0018 refines that rather than contradicting it — Notes are the
    // Operator's *own writing*, not a claim about what occurred, so they
    // stay editable forever. The Summary is likewise derived rather than
    // observed: it can be regenerated, and regenerating it destroys nothing.
    //
    // Both live on the Meeting rather than in their own tables because
    // there is exactly one of each per Meeting and neither is ever queried
    // independently of it.
    r#"
    ALTER TABLE meetings ADD COLUMN notes TEXT;
    ALTER TABLE meetings ADD COLUMN summary TEXT;
    -- Which Backend produced the Summary, and when. An Operator who chose
    -- Cloud and received local quality is owed the reason (story 38), and
    -- one who chose Local is owed evidence that is what ran.
    ALTER TABLE meetings ADD COLUMN summary_backend TEXT;
    ALTER TABLE meetings ADD COLUMN summary_generated_at TEXT;

    -- Editing either has to reach the folder the Operator actually reads.
    -- `meetings_after_update` is scoped with UPDATE OF and does not list
    -- these, deliberately: adding them there would also fire on the
    -- projection worker's own writes.
    CREATE TRIGGER meetings_after_notes_or_summary
    AFTER UPDATE OF notes, summary ON meetings BEGIN
        INSERT INTO mirror_dirty (meeting_id, generation) VALUES (NEW.id, 1)
        ON CONFLICT (meeting_id) DO UPDATE SET generation = generation + 1;
    END;
    "#,
    // 9 — what a Summary lost (summary-chunking-and-suggested-title/04).
    //
    // A Summary assembled from five chunks of six is a different thing from a
    // complete one, and until now only the Core's log knew the difference —
    // which the Operator cannot read. This is the audio-notes pattern applied
    // one layer up: the record states its own incompleteness where the person
    // holding it will see it.
    //
    // **Deliberately not called `summary_notes`.** Notes are the Operator's
    // own writing (ADR-0018) and the glossary reserves the word; a
    // machine-written column wearing it would be the vocabulary collision
    // CONTEXT.md exists to prevent.
    r#"
    ALTER TABLE meetings ADD COLUMN summary_gaps TEXT;
    "#,
    // 10 — where each voice can be heard, and the Speakers that never were.
    //
    // **The sample.** An exemplar has always recorded which Meeting it came
    // from; it now records *where in it* — one channel, one stretch on the
    // capture clock — so the Registry can play the voice back rather than
    // only name it. Kept audio is a constant-bitrate frame stream
    // (ADR-0032), so a stretch is a byte range and the cut costs no decode
    // pass over the Meeting. The columns are nullable because every exemplar
    // written before this migration has no window to give.
    //
    // **The prune.** Until now Diarization minted a Speaker for every
    // cluster it found, before it knew whether the cluster owned a single
    // transcribed word — and on the first real History this product
    // accumulated, 378 of 503 Speakers owned none: three-second windows of
    // echo and crosstalk, each with a Voiceprint, each a stranger in the
    // Registry. `diarize::cluster::persist` no longer creates those. This
    // removes the ones already created, under the narrowest predicate that
    // names them: no segment attributed, no correction hint in either
    // direction, no name, not the Operator. **Contradicts ADR-0009 as
    // written ("Speaker records themselves are permanent"), and deliberately
    // so:** that guarantee exists so nothing in the record ever dangles or
    // rewrites, and a Speaker that nothing in the record references is not
    // in the record — deleting it changes no Transcript, no attribution and
    // no correction. Named Speakers are kept whatever they reference,
    // because a name is the Operator's act. Once, here, rather than as a
    // standing rule: a Speaker orphaned by a *Meeting* deletion is the case
    // "Voiceprints outlive the recordings they came from" protects, and it
    // matches this predicate too — so the rule must not run again.
    r#"
    ALTER TABLE speaker_exemplars ADD COLUMN sample_channel TEXT
        CHECK (sample_channel IN ('mic', 'system'));
    ALTER TABLE speaker_exemplars ADD COLUMN sample_start_ms INTEGER;
    ALTER TABLE speaker_exemplars ADD COLUMN sample_end_ms INTEGER;

    DELETE FROM speakers
     WHERE is_operator = 0
       AND display_name IS NULL
       AND id NOT IN (SELECT speaker_id FROM transcript_segments WHERE speaker_id IS NOT NULL)
       AND id NOT IN (SELECT speaker_id FROM attribution_hints)
       AND id NOT IN (SELECT replaced_speaker_id FROM attribution_hints
                       WHERE replaced_speaker_id IS NOT NULL);
    "#,
    // 11 — whether Diarization ever ran, so an interrupted one can be finished.
    //
    // `diarize_in_background` is detached on purpose, which means a Core that
    // stops in those minutes takes the run with it — and a Meeting that was
    // never diarized is indistinguishable in this schema from one where
    // Diarization ran and recognised nobody. Without that distinction a retry
    // either misses the first or repeats the second on every start.
    //
    // Backfilled from the evidence rather than guessed: a Meeting with an
    // attributed segment was plainly diarized. One without is left NULL, so
    // the next Core start finishes what a previous one did not — which is
    // exactly what heals the Meetings this migration was written for.
    r#"
    ALTER TABLE meetings ADD COLUMN diarized_at TEXT;

    UPDATE meetings SET diarized_at = updated_at
     WHERE id IN (SELECT DISTINCT meeting_id FROM transcript_segments
                   WHERE speaker_id IS NOT NULL);
    "#,
    // 12 — Diarization waits its turn instead of being turned away.
    //
    // M3's policy was refuse-don't-queue, which was right while the only
    // producer was a Meeting ending: a backlog competing for the machine is
    // worse than none, and a refused Meeting can be re-run on the Operator's
    // say-so. A model change re-runs all of History, and under that policy
    // every Meeting that ended during the re-run would be dropped on the
    // floor with only a log line about it.
    //
    // In the record rather than in memory because the queue has to outlive
    // the process: a Core killed mid-backlog that forgot its remaining work
    // would leave a History half-attributed, which reads exactly like
    // diarization being unreliable.
    //
    // `priority` is small-number-first, so a just-ended Meeting or an
    // Operator's request (0) goes ahead of bulk work (1) whatever the
    // arrival order, and `enqueued_at` keeps it FIFO within a priority.
    // ON DELETE CASCADE because a queued Meeting the Operator deletes is not
    // work to do later.
    r#"
    CREATE TABLE diarize_queue (
        meeting_id   TEXT PRIMARY KEY NOT NULL
                     REFERENCES meetings(id) ON DELETE CASCADE,
        priority     INTEGER NOT NULL CHECK (priority IN (0, 1)),
        enqueued_at  TEXT NOT NULL
    ) STRICT;

    CREATE INDEX diarize_queue_order ON diarize_queue (priority, enqueued_at);
    "#,
    // 13 — a deleted Voiceprint stays deleted.
    //
    // Deleting a Voiceprint is this product's one biometric control, and
    // ADR-0009 makes it a legible Operator act. After migration 12 a Speaker
    // the Operator deliberately forgot looks identical to one the model
    // change cleared: a name, and no vector. A re-run that relearns named
    // Speakers from their attributed segments would bring the forgotten
    // voice back, and the Operator would have no way to know it happened.
    //
    // So the act leaves a mark of its own, and only that act sets it. It is
    // not derivable from the columns that were already there — "named, no
    // vector" is now the ordinary state of most of the Registry.
    //
    // Nothing here is retroactive. A Voiceprint deleted before this shipped
    // left no record that it was deleted rather than never taken, and
    // marking those rows forgotten would be inventing an Operator act that
    // may never have happened.
    r#"
    ALTER TABLE speakers ADD COLUMN forgotten INTEGER NOT NULL DEFAULT 0
        CHECK (forgotten IN (0, 1));
    "#,
    // 14 — one Operator, and the fact that decides them without an act.
    //
    // `mic_isolated` is what the capture layer concluded about this Meeting:
    // the far end could not have reached the microphone, because headphones
    // were the only playing output and the microphone was never swapped.
    // ADR-0029 as amended makes that the first of the three rules that name
    // "You", and it is a fact about the recording, so it is recorded with the
    // recording rather than re-derived later from audio that no longer says.
    //
    // Nullable on purpose, with three states rather than two: 1 is isolated,
    // 0 is looked at and not isolated, and NULL is a Meeting recorded before
    // this shipped or one whose probe failed. Only 1 grants the rule, so the
    // other two behave alike today — but a re-run that walks all of History
    // (ticket 12) needs to tell "no" from "never asked", and a NOT NULL
    // DEFAULT 0 would have thrown that away on every Meeting already on disk.
    //
    // The index is the other half. The flag never had a uniqueness
    // constraint, the lookup took the first row it found, and the diarize
    // path set the flag without clearing any other — so deleting the
    // Operator's Voiceprint and re-running one Meeting put the flag on a
    // freshly minted row while the lookup still returned the old one, and
    // the Registry showed two "You". Any History that already has two is
    // reduced to one first, keeping the row with a Voiceprint because that
    // is the one recognition has been using; ties go to the oldest.
    r#"
    ALTER TABLE meetings ADD COLUMN mic_isolated INTEGER
        CHECK (mic_isolated IN (0, 1));

    UPDATE speakers SET is_operator = 0
     WHERE is_operator = 1
       AND id <> (SELECT id FROM speakers WHERE is_operator = 1
                   ORDER BY (voiceprint IS NULL), created_at, id
                   LIMIT 1);

    CREATE UNIQUE INDEX speakers_one_operator
        ON speakers (is_operator) WHERE is_operator = 1;
    "#,
];

/// The wipe a model change owes, **written and deliberately not registered**.
///
/// Ticket 05. ADR-0037: when the embedding changes, old and new vectors
/// cannot be compared, so every Voiceprint and every exemplar goes and the
/// record stays. Re-learning is ticket 12's, from the Operator's attributed
/// whole clusters — *not* from these exemplars' stored sample offsets, which
/// are the old model's choice of cuts.
///
/// **Absent from [`MIGRATIONS`] on purpose.** Applying it while the model is
/// unchanged would clear Voiceprints for no swap, and applying it without
/// ticket 12 would leave a History nobody is recognized in, which ADR-0037's
/// *Considered options* rejected by name. It is here so it is written and
/// tested before the swap rather than during it; appending it to `MIGRATIONS`
/// is the whole of its activation.
///
/// What it keeps and why:
///
/// - **`display_name`, `is_operator`, `forgotten`** — the Operator's acts,
///   none of which was a statement about a vector.
/// - **`confirmed`** — naming *is* confirmation (ADR-0008 as amended), and
///   the name survives, so the confirmation does. Clearing it would make
///   ticket 12 hand a named Speaker a Voiceprint ranking below an
///   unconfirmed one, having been vouched for.
/// - **`transcript_segments.speaker_id` and every `attribution_hints` row** —
///   after this runs they are the only surviving record of who the voices
///   are, and ticket 12's seeding reads them.
/// - **`voiceprint_model` and `voiceprint_model_version`** — the stamp
///   outlives the vector it described, because it is the only thing that can
///   tell the Registry *why* a named Speaker has nothing behind it. Three
///   states wear "no Voiceprint": one the Operator chose, one this wipe
///   caused, and one that is a voice never enrolled. The renderer's
///   `voiceprintLabel` already separates them and already reads the stamp for
///   the middle one, so nulling it here turned "cleared when the voice model
///   changed" into "No Voiceprint" — the sentence for a voice that was never
///   known — on every Speaker this ran over.
///
/// Keeping the stamp cannot revive anything, which is the property that makes
/// it the small fix rather than a new column. Both queries that could act on
/// it gate on the vector, not the stamp: [`speakers::voiceprints`], the
/// gallery every match is drawn from, selects `voiceprint IS NOT NULL AND
/// voiceprint_model = ?`, and [`speakers::speakers_with_stale_voiceprint`],
/// which drives the lazy re-embed, selects `voiceprint IS NOT NULL AND
/// (voiceprint_model IS NOT ? ...)`. A row with a stamp and no vector is
/// outside both. `has_voiceprint` on the wire is `voiceprint IS NOT NULL`
/// too, so nothing downstream reads the stamp as evidence; and with every
/// exemplar deleted there is nothing left to re-embed in any case. The stamp
/// is overwritten wholesale by [`speakers::set_voiceprint`] when a real
/// vector next arrives.
///
/// [`speakers::voiceprints`]: crate::store::speakers::voiceprints
/// [`speakers::speakers_with_stale_voiceprint`]:
///     crate::store::speakers::speakers_with_stale_voiceprint
/// [`speakers::set_voiceprint`]: crate::store::speakers::set_voiceprint
pub const PENDING_MODEL_CHANGE_WIPE: &str = r#"
    DELETE FROM speaker_exemplars;

    UPDATE speakers
       SET voiceprint = NULL
     WHERE voiceprint IS NOT NULL;
"#;

/// The backlog a model change re-runs History with, **written and
/// deliberately not registered**.
///
/// Ticket 12. The work itself lives in [`super::diarize_queue`], which
/// already outlives the process; what these two tables hold is only what the
/// queue cannot say — which model the backlog is for, how big it was, whether
/// the Operator stopped it, and which Meetings are its own.
///
/// **Absent from [`MIGRATIONS`] for the same reason as
/// [`PENDING_MODEL_CHANGE_WIPE`]**, which it is the other half of: a re-run
/// without the wipe re-diarizes a History whose Voiceprints are still the old
/// model's, and a wipe without the re-run leaves a History nobody is
/// recognized in.
///
/// `diarize_rerun` is one row by primary key rather than by every writer
/// remembering: two re-runs of different models at once is not a state this
/// product has, and a table that can hold one is a table somebody will have
/// to reconcile. `cancelled` outlives the emptied queue, or the next start
/// would find a drained backlog and read it as finished; `abandoned` is what
/// cancelling threw away, without which done — total minus remaining — jumps
/// to total and an Operator who stopped a re-run at 1 of 40 is told all forty
/// were walked.
///
/// `diarize_rerun_backlog` is **which Meetings are the re-run's own**, and it
/// exists because the queue's `Back` priority is a scheduling class, not a
/// job. Production already enqueues at `Back` for Meetings that were never
/// diarized, so counting the whole backlog would report that catch-up work as
/// re-run progress and cancelling would delete it.
pub const PENDING_MODEL_CHANGE_RERUN: &str = r#"
    CREATE TABLE diarize_rerun (
        id             INTEGER PRIMARY KEY CHECK (id = 1),
        model          TEXT NOT NULL,
        model_version  TEXT NOT NULL,
        total          INTEGER NOT NULL,
        cancelled      INTEGER NOT NULL DEFAULT 0 CHECK (cancelled IN (0, 1)),
        abandoned      INTEGER NOT NULL DEFAULT 0,
        started_at     TEXT NOT NULL
    ) STRICT;

    CREATE TABLE diarize_rerun_backlog (
        meeting_id TEXT PRIMARY KEY NOT NULL
                   REFERENCES meetings(id) ON DELETE CASCADE
    ) STRICT;
"#;

/// Applies every migration the database has not seen yet.
pub fn migrate(connection: &mut Connection) -> rusqlite::Result<()> {
    let applied: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let applied = applied as usize;

    for (index, migration) in MIGRATIONS.iter().enumerate().skip(applied) {
        let transaction = connection.transaction()?;
        transaction.execute_batch(migration)?;
        // PRAGMA does not accept a bound parameter.
        transaction.pragma_update(None, "user_version", (index + 1) as i64)?;
        transaction.commit()?;
    }
    Ok(())
}

/// Connection settings every handle needs.
pub fn configure(connection: &Connection) -> rusqlite::Result<()> {
    connection.pragma_update(None, "journal_mode", "WAL")?;
    connection.pragma_update(None, "synchronous", "NORMAL")?;
    connection.pragma_update(None, "foreign_keys", true)?;
    connection.busy_timeout(std::time::Duration::from_secs(5))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_apply_and_are_idempotent() {
        let mut connection = Connection::open_in_memory().expect("open");
        configure(&connection).expect("configure");
        migrate(&mut connection).expect("migrate");
        migrate(&mut connection).expect("migrate again");

        let version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("user_version");
        assert_eq!(version as usize, MIGRATIONS.len());
    }

    #[test]
    fn a_database_already_carrying_the_diarization_mark_still_gets_the_queue() {
        // The real upgrade path for anyone who ran the build that shipped
        // migration 11. Two branches appended migrations after the same base
        // and both wanted position 11; `diarized_at` kept it because it was
        // already pushed, and the queue moved to 12 (DECISIONS Q134).
        //
        // Had the order gone the other way, a database sitting at
        // user_version 11 would have counted `diarize_queue` as already
        // applied and skipped it for good — surfacing much later, and far
        // from here, as a table that does not exist.
        let mut connection = Connection::open_in_memory().expect("open");
        configure(&connection).expect("configure");
        let shipped = 11;
        for migration in &MIGRATIONS[..shipped] {
            connection.execute_batch(migration).expect("migrate");
        }
        connection
            .pragma_update(None, "user_version", shipped as i64)
            .expect("user_version");

        migrate(&mut connection).expect("migrate the rest");

        let queue: i64 = connection
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'diarize_queue'",
                [],
                |row| row.get(0),
            )
            .expect("query");
        assert_eq!(queue, 1, "diarize_queue was skipped by the upgrade");
        // And the columns either side of the boundary are both present.
        for (table, column) in [
            ("meetings", "diarized_at"),
            ("speakers", "forgotten"),
            ("meetings", "mic_isolated"),
        ] {
            let found: i64 = connection
                .query_row(
                    &format!("SELECT count(*) FROM pragma_table_info('{table}') WHERE name = ?1"),
                    [column],
                    |row| row.get(0),
                )
                .expect("query");
            assert_eq!(found, 1, "{table}.{column} is missing after the upgrade");
        }
    }

    #[test]
    fn the_diarization_mark_is_backfilled_from_the_evidence_not_guessed() {
        // Migration 11 decides which existing Meetings the retry picks up on
        // the first start after an upgrade. A Meeting with an attributed
        // segment was plainly diarized and must not be redone; one with none
        // is exactly the case the retry exists for, and the two real
        // Meetings that prompted it had zero (DECISIONS Q125).
        let mut connection = Connection::open_in_memory().expect("open");
        configure(&connection).expect("configure");
        let before_mark = 10;
        for migration in &MIGRATIONS[..before_mark] {
            connection.execute_batch(migration).expect("migrate");
        }
        connection
            .pragma_update(None, "user_version", before_mark as i64)
            .expect("user_version");
        connection
            .execute_batch(
                "INSERT INTO meetings (id, started_at, created_at, updated_at)
                 VALUES ('heard', 'now', 'now', 'now'), ('silent', 'now', 'now', 'now');
                 INSERT INTO speakers (id, is_operator, created_at) VALUES ('s1', 0, 'now');
                 INSERT INTO transcript_segments
                     (id, meeting_id, sequence, channel, start_ms, end_ms, text, speaker_id)
                 VALUES ('a', 'heard', 0, 'mic', 0, 1, 'hi', 's1'),
                        ('b', 'silent', 0, 'mic', 0, 1, 'hi', NULL);",
            )
            .expect("seed");

        migrate(&mut connection).expect("migrate the rest");

        let marked: Vec<String> = connection
            .prepare("SELECT id FROM meetings WHERE diarized_at IS NOT NULL ORDER BY id")
            .expect("prepare")
            .query_map([], |row| row.get::<_, String>(0))
            .expect("query")
            .collect::<rusqlite::Result<Vec<_>>>()
            .expect("rows");
        assert_eq!(
            marked,
            vec!["heard".to_string()],
            "only a Meeting with words already attributed counts as diarized"
        );
    }

    #[test]
    fn the_prune_removes_only_speakers_nothing_references() {
        // Migration 10 runs once over a History that already holds the
        // Speakers the old policy minted. Everything the record points at
        // has to survive it: an attributed voice, a corrected one, a named
        // one, the Operator. Only the row nobody references goes.
        let mut connection = Connection::open_in_memory().expect("open");
        configure(&connection).expect("configure");
        // Up to the migration before the prune, then seed, then prune.
        let before_prune = 9;
        for migration in &MIGRATIONS[..before_prune] {
            connection.execute_batch(migration).expect("migrate");
        }
        connection
            .pragma_update(None, "user_version", before_prune as i64)
            .expect("user_version");
        connection
            .execute_batch(
                "INSERT INTO meetings (id, started_at, created_at, updated_at)
                 VALUES ('m', 'now', 'now', 'now');
                 INSERT INTO speakers (id, is_operator, created_at) VALUES
                     ('attributed', 0, 'now'), ('corrected-to', 0, 'now'),
                     ('corrected-from', 0, 'now'), ('junk', 0, 'now'), ('you', 1, 'now');
                 INSERT INTO speakers (id, display_name, is_operator, created_at)
                 VALUES ('named', 'Alice', 0, 'now');
                 INSERT INTO transcript_segments
                     (id, meeting_id, sequence, channel, start_ms, end_ms, text, speaker_id)
                 VALUES ('s1', 'm', 0, 'mic', 0, 1, 'hi', 'attributed'),
                        ('s2', 'm', 1, 'mic', 1, 2, 'hi', NULL);
                 INSERT INTO attribution_hints
                     (id, segment_id, speaker_id, replaced_speaker_id, created_at)
                 VALUES ('h', 's2', 'corrected-to', 'corrected-from', 'now');
                 INSERT INTO speaker_exemplars
                     (id, speaker_id, embedding, model, model_version, voiced_ms, source, created_at)
                 VALUES ('e', 'junk', x'00', 'm', '1', 1, 'machine', 'now');",
            )
            .expect("seed");

        migrate(&mut connection).expect("migrate the rest");

        let mut statement = connection
            .prepare("SELECT id FROM speakers ORDER BY id")
            .expect("prepare");
        let kept: Vec<String> = statement
            .query_map([], |row| row.get(0))
            .expect("query")
            .collect::<rusqlite::Result<_>>()
            .expect("rows");
        assert_eq!(
            kept,
            [
                "attributed",
                "corrected-from",
                "corrected-to",
                "named",
                "you"
            ],
            "everything the record references survives; only the junk goes"
        );
        let exemplars: i64 = connection
            .query_row("SELECT count(*) FROM speaker_exemplars", [], |row| {
                row.get(0)
            })
            .expect("count");
        assert_eq!(exemplars, 0, "and its Voiceprint evidence with it");
    }

    #[test]
    fn full_text_search_is_available() {
        // FTS5 is not optional for us: History search is a headline story.
        let connection = Connection::open_in_memory().expect("open");
        connection
            .execute_batch("CREATE VIRTUAL TABLE probe USING fts5(body);")
            .expect("this build of SQLite must have FTS5");
    }

    #[test]
    fn the_channel_enum_is_enforced_by_the_database() {
        let mut connection = Connection::open_in_memory().expect("open");
        configure(&connection).expect("configure");
        migrate(&mut connection).expect("migrate");
        connection
            .execute(
                "INSERT INTO meetings (id, started_at, created_at, updated_at)
                 VALUES ('m', 'now', 'now', 'now')",
                [],
            )
            .expect("insert meeting");

        let result = connection.execute(
            "INSERT INTO transcript_segments (id, meeting_id, sequence, channel, start_ms, end_ms, text)
             VALUES ('s', 'm', 0, 'telepathy', 0, 1, 'hello')",
            [],
        );
        assert!(result.is_err(), "an invalid channel must be rejected");
    }

    #[test]
    fn deleting_a_meeting_takes_its_segments_with_it() {
        let mut connection = Connection::open_in_memory().expect("open");
        configure(&connection).expect("configure");
        migrate(&mut connection).expect("migrate");
        connection
            .execute(
                "INSERT INTO meetings (id, started_at, created_at, updated_at)
                 VALUES ('m', 'now', 'now', 'now')",
                [],
            )
            .expect("insert meeting");
        connection
            .execute(
                "INSERT INTO transcript_segments (id, meeting_id, sequence, channel, start_ms, end_ms, text)
                 VALUES ('s', 'm', 0, 'mic', 0, 1, 'hello')",
                [],
            )
            .expect("insert segment");

        connection
            .execute("DELETE FROM meetings WHERE id = 'm'", [])
            .expect("delete meeting");
        let remaining: i64 = connection
            .query_row("SELECT count(*) FROM transcript_segments", [], |row| {
                row.get(0)
            })
            .expect("count");
        assert_eq!(remaining, 0, "whole-Meeting delete must be complete");
    }

    // ---- Ticket 05: the wipe that is written but not registered ----

    /// The activation gate, as a test rather than as a comment.
    ///
    /// Appending it to `MIGRATIONS` is the whole of activating it, so doing
    /// that by accident — a stray paste, a merge — would clear Voiceprints on
    /// the next Core start with no model swap behind it.
    #[test]
    fn the_pending_wipe_is_not_registered() {
        assert!(
            !MIGRATIONS.contains(&PENDING_MODEL_CHANGE_WIPE),
            "ticket 05 activates on the user's model decision and on ticket 12 existing; \
             neither has happened"
        );
    }

    /// The same gate for ticket 12's half.
    ///
    /// Registering it would create the tables on the next Core start, which
    /// is harmless on its own — nothing reads them — but it is the step that
    /// turns dormant groundwork into schema the product carries, and it
    /// belongs with the swap rather than before it.
    #[test]
    fn the_pending_rerun_is_not_registered() {
        assert!(
            !MIGRATIONS.contains(&PENDING_MODEL_CHANGE_RERUN),
            "ticket 12 activates with ticket 05 and the user's model decision"
        );
    }

    /// A History as the current build leaves one, on disk.
    ///
    /// A named Speaker with a Voiceprint and both signs of evidence, the
    /// Operator, a forgotten Speaker, a named Speaker who was never
    /// enrolled, an attributed segment and a correction hint — one of each
    /// thing the wipe promises to keep or to take.
    fn populated_history(path: &std::path::Path) -> (String, String, String, String, String) {
        use crate::diarize::live::{EMBEDDING_MODEL, EMBEDDING_MODEL_VERSION};
        use crate::store::speakers::{self, NewExemplar};
        use rusqlite::params;

        let mut connection = Connection::open(path).expect("open");
        configure(&connection).expect("configure");
        migrate(&mut connection).expect("migrate");

        connection
            .execute(
                "INSERT INTO meetings (id, started_at, created_at, updated_at, audio_path)
                 VALUES ('m1', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z',
                         '2026-01-01T00:00:00Z', 'm1.wav')",
                [],
            )
            .expect("meeting");

        let alice = speakers::create(&connection, false).expect("alice").id;
        speakers::rename(&connection, &alice, "Alice").expect("name");
        let me = speakers::create(&connection, true).expect("operator").id;
        speakers::rename(&connection, &me, "Me").expect("name");
        let gone = speakers::create(&connection, false).expect("gone").id;
        speakers::rename(&connection, &gone, "Gone").expect("name");
        // Named and never heard well enough to enrol: the third way to have
        // no Voiceprint, and the one the Registry must not confuse with
        // either of the other two.
        let fresh = speakers::create(&connection, false).expect("fresh").id;
        speakers::rename(&connection, &fresh, "Fresh").expect("name");

        for (id, vector) in [(&alice, [1.0f32, 0.0]), (&me, [0.0, 1.0])] {
            for is_negative in [false, true] {
                speakers::add_exemplar(
                    &connection,
                    NewExemplar {
                        speaker_id: id,
                        meeting_id: Some("m1"),
                        vector: &vector,
                        model: EMBEDDING_MODEL,
                        model_version: EMBEDDING_MODEL_VERSION,
                        voiced_ms: 30_000,
                        from_operator: is_negative,
                        is_negative,
                        sample: Some(speakers::Sample {
                            channel: evertranscript_protocol::AudioChannel::Mic,
                            start_ms: 0,
                            end_ms: 30_000,
                        }),
                    },
                )
                .expect("exemplar");
            }
            speakers::set_voiceprint(
                &connection,
                id,
                &vector,
                EMBEDDING_MODEL,
                EMBEDDING_MODEL_VERSION,
            )
            .expect("voiceprint");
        }

        // Forgotten after it had evidence, which is the state that has to
        // survive: the wipe must not look like an un-forgetting.
        speakers::delete_voiceprint(&connection, &gone).expect("forget");

        let segment = {
            let id = "s1".to_string();
            connection
                .execute(
                    "INSERT INTO transcript_segments (id, meeting_id, sequence, channel,
                     start_ms, end_ms, text, speaker_id)
                     VALUES (?1, 'm1', 0, 'mic', 0, 1000, 'hello', ?2)",
                    params![id, alice],
                )
                .expect("segment");
            id
        };
        // The Operator disagreeing, which is the hint that must outlive this.
        speakers::correct_attribution(&connection, &segment, &me).expect("correct");

        (alice, me, gone, fresh, segment)
    }

    /// The state the fixture leaves, so both tests below assert the same
    /// things about it and a drift shows up once rather than twice.
    fn record_survives(
        connection: &Connection,
        alice: &str,
        me: &str,
        gone: &str,
        fresh: &str,
        segment: &str,
    ) {
        use crate::store::speakers;

        let named = speakers::get(connection, alice)
            .expect("get")
            .expect("alice");
        assert_eq!(named.display_name.as_deref(), Some("Alice"));
        assert!(
            named.confirmed,
            "naming is confirmation and the name stayed"
        );
        assert!(!named.forgotten);

        let operator = speakers::operator(connection)
            .expect("operator")
            .expect("one");
        assert_eq!(operator.id, me);

        let forgotten = speakers::get(connection, gone).expect("get").expect("gone");
        assert!(forgotten.forgotten, "a forgotten Speaker stays forgotten");
        assert_eq!(forgotten.display_name.as_deref(), Some("Gone"));

        let never = speakers::get(connection, fresh)
            .expect("get")
            .expect("fresh");
        assert_eq!(never.display_name.as_deref(), Some("Fresh"));
        assert!(!never.forgotten, "nobody deleted anything here");
        assert!(
            !never.has_voiceprint && never.voiceprint_model.is_none(),
            "a voice never enrolled has no vector and no model to name"
        );

        assert_eq!(
            speakers::attributed_speaker(connection, segment).expect("attributed"),
            Some(me.to_string()),
            "the correction hint is the record of who the voice is"
        );
        let machine: Option<String> = connection
            .query_row(
                "SELECT speaker_id FROM transcript_segments WHERE id = ?1",
                rusqlite::params![segment],
                |row| row.get(0),
            )
            .expect("segment");
        assert_eq!(
            machine.as_deref(),
            Some(alice),
            "and the machine's attribution underneath it, which makes the correction legible"
        );
    }

    /// The build as it ships must leave a current History exactly alone.
    ///
    /// The control for the wipe below: if merely opening and migrating moved
    /// something, the wipe test would be measuring that instead.
    #[test]
    fn opening_a_current_history_leaves_its_voiceprints_alone() {
        use crate::diarize::live::{EMBEDDING_MODEL, EMBEDDING_MODEL_VERSION};
        use crate::store::speakers;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("history.sqlite3");
        let (alice, me, gone, fresh, segment) = populated_history(&path);

        // Closed and reopened, because the claim is about what the next Core
        // opens rather than about in-memory state.
        let mut connection = Connection::open(&path).expect("reopen");
        configure(&connection).expect("configure");
        migrate(&mut connection).expect("migrate again");

        record_survives(&connection, &alice, &me, &gone, &fresh, &segment);
        assert_eq!(
            speakers::voiceprints(&connection, EMBEDDING_MODEL, EMBEDDING_MODEL_VERSION)
                .expect("voiceprints")
                .len(),
            2,
            "Alice and the Operator are still recognizable"
        );
        assert!(
            speakers::stale_exemplars(&connection, EMBEDDING_MODEL, EMBEDDING_MODEL_VERSION)
                .expect("stale")
                .is_empty(),
            "nothing in a current History is from another space"
        );
    }

    /// Ticket 05, on disk: the vectors go and the record stays.
    #[test]
    fn the_pending_wipe_takes_every_vector_and_keeps_the_record() {
        use crate::diarize::live::{EMBEDDING_MODEL, EMBEDDING_MODEL_VERSION};
        use crate::store::speakers;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("history.sqlite3");
        let (alice, me, gone, fresh, segment) = populated_history(&path);

        {
            let connection = Connection::open(&path).expect("open to wipe");
            configure(&connection).expect("configure");
            connection
                .execute_batch(PENDING_MODEL_CHANGE_WIPE)
                .expect("wipe");
        }

        let connection = Connection::open(&path).expect("reopen");
        configure(&connection).expect("configure");

        record_survives(&connection, &alice, &me, &gone, &fresh, &segment);

        let exemplars: i64 = connection
            .query_row("SELECT count(*) FROM speaker_exemplars", [], |row| {
                row.get(0)
            })
            .expect("count");
        assert_eq!(exemplars, 0, "positive and negative alike");
        let vectors: i64 = connection
            .query_row(
                "SELECT count(*) FROM speakers WHERE voiceprint IS NOT NULL",
                [],
                |row| row.get(0),
            )
            .expect("count");
        assert_eq!(vectors, 0, "every vector, whoever it belonged to");

        // The gallery is empty even though two rows still name the model,
        // because it is drawn by vector: this is the property that lets the
        // stamp stay without becoming evidence again.
        assert!(
            speakers::voiceprints(&connection, EMBEDDING_MODEL, EMBEDDING_MODEL_VERSION)
                .expect("voiceprints")
                .is_empty(),
            "a stamp with no vector behind it must not be matchable"
        );
        assert!(
            speakers::speakers_with_stale_voiceprint(&connection, "some-next-model", "1")
                .expect("stale")
                .is_empty(),
            "and must not look to the next model like something to re-embed"
        );

        // Four Speakers, three reasons to hold no Voiceprint, and the fields
        // the Registry reads to tell them apart. Asserted as stored state, not
        // by restating the renderer's choice here: what this migration owes is
        // that the evidence stays distinguishable, and `voiceprintLabel`
        // (App.tsx) owns which sentence goes with which combination. Nulling
        // the stamp made Alice's row identical to Fresh's, which is how a
        // cleared Voiceprint came to read as a voice never known.
        let stored = |id: &str| {
            let speaker = speakers::get(&connection, id).expect("get").expect("some");
            (
                speaker.has_voiceprint,
                speaker.forgotten,
                speaker.voiceprint_model,
                speaker.voiceprint_model_version,
            )
        };

        for (who, id) in [("Alice", &alice), ("the Operator", &me)] {
            let (has_voiceprint, forgotten, model, version) = stored(id);
            assert!(!has_voiceprint, "{who} lost the vector");
            assert!(!forgotten, "{who} was not deleted by anybody");
            assert_eq!(
                (model.as_deref(), version.as_deref()),
                (Some(EMBEDDING_MODEL), Some(EMBEDDING_MODEL_VERSION)),
                "{who} keeps the stamp of the model whose vector this wipe took"
            );
        }

        let (has_voiceprint, forgotten, ..) = stored(&gone);
        assert!(!has_voiceprint);
        assert!(
            forgotten,
            "an Operator's deletion is still an Operator's deletion"
        );

        let (has_voiceprint, forgotten, model, version) = stored(&fresh);
        assert!(!has_voiceprint);
        assert!(!forgotten);
        assert_eq!(
            (model, version),
            (None, None),
            "a voice never enrolled has no model to name, and this wipe gave it none"
        );

        // The one that stops the lazy rebuild path reintroducing the old
        // model's cuts behind the wipe: with no exemplars left there is
        // nothing for `runner::rebuild` to re-embed, whatever model asks.
        for (model, version) in [
            (EMBEDDING_MODEL, EMBEDDING_MODEL_VERSION),
            ("some-next-model", "1"),
        ] {
            assert!(
                speakers::stale_exemplars(&connection, model, version)
                    .expect("stale")
                    .is_empty(),
                "{model} v{version} found evidence the wipe was supposed to have taken"
            );
        }

        // And the Speakers ticket 12 may give a Voiceprint back to are the
        // named, unforgotten ones — which is only true because the wipe kept
        // the names and the mark.
        let relearnable: Vec<String> = speakers::relearnable(&connection)
            .expect("relearnable")
            .into_iter()
            .map(|speaker| speaker.id)
            .collect();
        // The Operator is in this set because a re-run does give them a
        // Voiceprint back — by ADR-0029's channel rules, never by seeding
        // from the old model's attributions, which is ticket 12's
        // `claims` to exclude rather than this query's.
        assert!(relearnable.contains(&alice) && relearnable.contains(&me));
        // Named and never enrolled is relearnable for the plain reason: it was
        // always going to get a Voiceprint the next time it was heard.
        assert!(relearnable.contains(&fresh));
        assert!(!relearnable.contains(&gone), "still forgotten");
    }
}
