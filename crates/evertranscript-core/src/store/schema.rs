//! The record's schema and its migrations.
//!
//! House rules, borrowed from anarlog's schema hygiene and kept deliberately:
//! every table is STRICT, every enum is a CHECK, and invariants that matter
//! are constraints rather than conventions — a rule the database enforces
//! cannot be forgotten by a future writer.

use rusqlite::Connection;

/// Ordered migrations. Append only; never edit a shipped one.
/// 1 — the record: Meetings, their Transcript segments, and the Speakers
/// attribution will point at. Voiceprint columns exist from the start so
/// M3 adds behavior, not a table rewrite.
const THE_RECORD: &str = r#"
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
"#;

/// 2 — the Mirror projection queue. Triggers mark a Meeting dirty; one
/// worker rebuilds and acks. A write landing mid-rebuild bumps the
/// generation again, so the ack does not clear it and the Mirror is
/// rebuilt once more rather than silently going stale.
const MIRROR_QUEUE: &str = r#"
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
"#;

/// 3 — full-text search over the same projection the Mirror renders, so
/// what you can find is exactly what you can read.
const TRANSCRIPT_SEARCH: &str = r#"
    CREATE VIRTUAL TABLE search_index USING fts5(
        meeting_id UNINDEXED,
        title,
        body
    );
"#;

/// 4 — what a recording lost, in the record rather than only in a log.
/// A Meeting captured with half its audio previously looked exactly like
/// a complete one; the Operator opened one-sided notes with nothing to
/// explain them. A JSON array of human-readable notes, empty when the
/// recording was whole.
const WHAT_A_RECORDING_LOST: &str = r#"
    ALTER TABLE meetings ADD COLUMN audio_notes TEXT;
"#;

/// 5 — the Watchlist: what Meeting Detection watches on this machine
/// (ADR-0024, ADR-0030). In the machine store rather than the History
/// folder, like settings: the list describes this installation, and
/// copying History to a new machine must not carry it.
///
/// The shipped defaults are seeded here rather than defaulted in code, so
/// that an empty table means the Operator removed everything and gets
/// exactly that — not a silent restoration of the defaults on next start.
const THE_WATCHLIST: &str = r#"
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
"#;

/// 6 — what the calendar knew (ADR-0036). The title already rides the
/// Meeting; these are the two facts that would otherwise be lost: which
/// event it was, and who was invited. Attendees are *stored, not
/// applied* — they become Speaker-naming suggestions in M3, and turning
/// an invitation into an attribution before Diarization exists would be
/// inventing who spoke.
const WHAT_THE_CALENDAR_KNEW: &str = r#"
    ALTER TABLE meetings ADD COLUMN calendar_event_id TEXT;
    ALTER TABLE meetings ADD COLUMN calendar_attendees TEXT;
"#;

/// 7 — what Diarization keeps (M3).
///
/// Migration 1 gave `speakers` a single `voiceprint` BLOB and nothing
/// ever wrote to it. One vector per Speaker cannot represent a voice
/// across a headset, a laptop mic and a conference phone, and ADR-0008
/// promises recognition that *improves* with every Meeting — which a
/// single overwritten vector cannot do. So the column stays as the
/// current best identity vector (what matching compares against) and the
/// observations it is built from become rows.
///
/// Keeping the exemplars, rather than only their average, is what makes
/// two later operations possible at all: re-embedding from kept audio
/// after a model upgrade (ADR-0035's stated reason for the model columns),
/// and letting an Operator correction feed evidence back in (ADR-0009 as
/// amended) instead of being a display-only annotation.
const WHAT_DIARIZATION_KEEPS: &str = r#"
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
"#;

/// 8 — Operator Notes and the Summary (M4).
///
/// **These two columns are the only mutable content in the record, and
/// the distinction is worth stating where it lives.** ADR-0009 makes the
/// Transcript and its attribution immutable: they are what happened, and
/// a record that edits itself is the opposite of a legible guarantee.
/// ADR-0018 refines that rather than contradicting it — Notes are the
/// Operator's *own writing*, not a claim about what occurred, so they
/// stay editable forever. The Summary is likewise derived rather than
/// observed: it can be regenerated, and regenerating it destroys nothing.
///
/// Both live on the Meeting rather than in their own tables because
/// there is exactly one of each per Meeting and neither is ever queried
/// independently of it.
const NOTES_AND_SUMMARY: &str = r#"
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
"#;

/// 9 — what a Summary lost (summary-chunking-and-suggested-title/04).
///
/// A Summary assembled from five chunks of six is a different thing from a
/// complete one, and until now only the Core's log knew the difference —
/// which the Operator cannot read. This is the audio-notes pattern applied
/// one layer up: the record states its own incompleteness where the person
/// holding it will see it.
///
/// **Deliberately not called `summary_notes`.** Notes are the Operator's
/// own writing (ADR-0018) and the glossary reserves the word; a
/// machine-written column wearing it would be the vocabulary collision
/// CONTEXT.md exists to prevent.
const WHAT_A_SUMMARY_LOST: &str = r#"
    ALTER TABLE meetings ADD COLUMN summary_gaps TEXT;
"#;

/// 10 — where each voice can be heard, and the Speakers that never were.
///
/// **The sample.** An exemplar has always recorded which Meeting it came
/// from; it now records *where in it* — one channel, one stretch on the
/// capture clock — so the Registry can play the voice back rather than
/// only name it. Kept audio is a constant-bitrate frame stream
/// (ADR-0032), so a stretch is a byte range and the cut costs no decode
/// pass over the Meeting. The columns are nullable because every exemplar
/// written before this migration has no window to give.
///
/// **The prune.** Until now Diarization minted a Speaker for every
/// cluster it found, before it knew whether the cluster owned a single
/// transcribed word — and on the first real History this product
/// accumulated, 378 of 503 Speakers owned none: three-second windows of
/// echo and crosstalk, each with a Voiceprint, each a stranger in the
/// Registry. `diarize::cluster::persist` no longer creates those. This
/// removes the ones already created, under the narrowest predicate that
/// names them: no segment attributed, no correction hint in either
/// direction, no name, not the Operator. **Contradicts ADR-0009 as
/// written ("Speaker records themselves are permanent"), and deliberately
/// so:** that guarantee exists so nothing in the record ever dangles or
/// rewrites, and a Speaker that nothing in the record references is not
/// in the record — deleting it changes no Transcript, no attribution and
/// no correction. Named Speakers are kept whatever they reference,
/// because a name is the Operator's act. Once, here, rather than as a
/// standing rule: a Speaker orphaned by a *Meeting* deletion is the case
/// "Voiceprints outlive the recordings they came from" protects, and it
/// matches this predicate too — so the rule must not run again.
const VOICE_SAMPLES_AND_THE_PRUNE: &str = r#"
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
"#;

/// 11 — whether Diarization ever ran, so an interrupted one can be finished.
///
/// `diarize_in_background` is detached on purpose, which means a Core that
/// stops in those minutes takes the run with it — and a Meeting that was
/// never diarized is indistinguishable in this schema from one where
/// Diarization ran and recognised nobody. Without that distinction a retry
/// either misses the first or repeats the second on every start.
///
/// Backfilled from the evidence rather than guessed: a Meeting with an
/// attributed segment was plainly diarized. One without is left NULL, so
/// the next Core start finishes what a previous one did not — which is
/// exactly what heals the Meetings this migration was written for.
const THE_DIARIZATION_MARK: &str = r#"
    ALTER TABLE meetings ADD COLUMN diarized_at TEXT;

    UPDATE meetings SET diarized_at = updated_at
     WHERE id IN (SELECT DISTINCT meeting_id FROM transcript_segments
                   WHERE speaker_id IS NOT NULL);
"#;

/// 12 — Diarization waits its turn instead of being turned away.
///
/// M3's policy was refuse-don't-queue, which was right while the only
/// producer was a Meeting ending: a backlog competing for the machine is
/// worse than none, and a refused Meeting can be re-run on the Operator's
/// say-so. A model change re-runs all of History, and under that policy
/// every Meeting that ended during the re-run would be dropped on the
/// floor with only a log line about it.
///
/// In the record rather than in memory because the queue has to outlive
/// the process: a Core killed mid-backlog that forgot its remaining work
/// would leave a History half-attributed, which reads exactly like
/// diarization being unreliable.
///
/// `priority` is small-number-first, so a just-ended Meeting or an
/// Operator's request (0) goes ahead of bulk work (1) whatever the
/// arrival order, and `enqueued_at` keeps it FIFO within a priority.
/// ON DELETE CASCADE because a queued Meeting the Operator deletes is not
/// work to do later.
const DIARIZE_QUEUE: &str = r#"
    CREATE TABLE diarize_queue (
        meeting_id   TEXT PRIMARY KEY NOT NULL
                     REFERENCES meetings(id) ON DELETE CASCADE,
        priority     INTEGER NOT NULL CHECK (priority IN (0, 1)),
        enqueued_at  TEXT NOT NULL
    ) STRICT;

    CREATE INDEX diarize_queue_order ON diarize_queue (priority, enqueued_at);
"#;

/// 13 — a deleted Voiceprint stays deleted.
///
/// Deleting a Voiceprint is this product's one biometric control, and
/// ADR-0009 makes it a legible Operator act. After migration 12 a Speaker
/// the Operator deliberately forgot looks identical to one the model
/// change cleared: a name, and no vector. A re-run that relearns named
/// Speakers from their attributed segments would bring the forgotten
/// voice back, and the Operator would have no way to know it happened.
///
/// So the act leaves a mark of its own, and only that act sets it. It is
/// not derivable from the columns that were already there — "named, no
/// vector" is now the ordinary state of most of the Registry.
///
/// Nothing here is retroactive. A Voiceprint deleted before this shipped
/// left no record that it was deleted rather than never taken, and
/// marking those rows forgotten would be inventing an Operator act that
/// may never have happened.
const A_DELETED_VOICEPRINT_STAYS_DELETED: &str = r#"
    ALTER TABLE speakers ADD COLUMN forgotten INTEGER NOT NULL DEFAULT 0
        CHECK (forgotten IN (0, 1));
"#;

/// 14 — one Operator, and the fact that decides them without an act.
///
/// `mic_isolated` is what the capture layer concluded about this Meeting:
/// the far end could not have reached the microphone, because headphones
/// were the only playing output and the microphone was never swapped.
/// ADR-0029 as amended makes that the first of the three rules that name
/// "You", and it is a fact about the recording, so it is recorded with the
/// recording rather than re-derived later from audio that no longer says.
///
/// Nullable on purpose, with three states rather than two: 1 is isolated,
/// 0 is looked at and not isolated, and NULL is a Meeting recorded before
/// this shipped or one whose probe failed. Only 1 grants the rule, so the
/// other two behave alike today — but a re-run that walks all of History
/// (ticket 12) needs to tell "no" from "never asked", and a NOT NULL
/// DEFAULT 0 would have thrown that away on every Meeting already on disk.
///
/// The index is the other half. The flag never had a uniqueness
/// constraint, the lookup took the first row it found, and the diarize
/// path set the flag without clearing any other — so deleting the
/// Operator's Voiceprint and re-running one Meeting put the flag on a
/// freshly minted row while the lookup still returned the old one, and
/// the Registry showed two "You". Any History that already has two is
/// reduced to one first, keeping the row with a Voiceprint because that
/// is the one recognition has been using; ties go to the oldest.
const ONE_OPERATOR: &str = r#"
    ALTER TABLE meetings ADD COLUMN mic_isolated INTEGER
        CHECK (mic_isolated IN (0, 1));

    UPDATE speakers SET is_operator = 0
     WHERE is_operator = 1
       AND id <> (SELECT id FROM speakers WHERE is_operator = 1
                   ORDER BY (voiceprint IS NULL), created_at, id
                   LIMIT 1);

    CREATE UNIQUE INDEX speakers_one_operator
        ON speakers (is_operator) WHERE is_operator = 1;
"#;

/// Every migration this History has, in the order they apply.
///
/// `user_version` counts how many of these have run, so **the order is the
/// schema's identity**: inserting one ahead of another tells every History
/// in the field that a migration it has never seen is already applied. New
/// ones are appended, and the number in each doc comment is its position
/// here.
///
/// Named rather than written out in place so a test can say *which* upgrade
/// it is standing in front of — see [`before`].
const MIGRATIONS: &[&str] = &[
    THE_RECORD,
    MIRROR_QUEUE,
    TRANSCRIPT_SEARCH,
    WHAT_A_RECORDING_LOST,
    THE_WATCHLIST,
    WHAT_THE_CALENDAR_KNEW,
    WHAT_DIARIZATION_KEEPS,
    NOTES_AND_SUMMARY,
    WHAT_A_SUMMARY_LOST,
    VOICE_SAMPLES_AND_THE_PRUNE,
    THE_DIARIZATION_MARK,
    DIARIZE_QUEUE,
    A_DELETED_VOICEPRINT_STAYS_DELETED,
    ONE_OPERATOR,
    MODEL_CHANGE_WIPE,
    MODEL_CHANGE_RERUN,
    THE_ENROLMENT,
];

/// 15 — the wipe a model change owes.
///
/// Ticket 05. ADR-0037: when the embedding changes, old and new vectors
/// cannot be compared, so every Voiceprint and every exemplar goes and the
/// record stays. Re-learning is ticket 12's, from the Operator's attributed
/// whole clusters — *not* from these exemplars' stored sample offsets, which
/// are the old model's choice of cuts.
///
/// Written and tested before the swap, and held out of [`MIGRATIONS`] until
/// it: applying it while the model is unchanged would clear Voiceprints for
/// no swap, and applying it without ticket 12 would leave a History nobody
/// is recognized in, which ADR-0037's *Considered options* rejected by name.
/// Registered 2026-09-17 on the user's instruction (DECISIONS Q228), the day
/// ReDimNet2-B3 replaced WeSpeaker (Q226), with [`MODEL_CHANGE_RERUN`]
/// directly behind it.
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
pub const MODEL_CHANGE_WIPE: &str = r#"
    DELETE FROM speaker_exemplars;

    UPDATE speakers
       SET voiceprint = NULL
     WHERE voiceprint IS NOT NULL;
"#;

/// 16 — the backlog a model change re-runs History with, and the row that
/// makes the next start ask for it.
///
/// Ticket 12. The work itself lives in [`super::diarize_queue`], which
/// already outlives the process; what these two tables hold is only what the
/// queue cannot say — which model the backlog is for, how big it was, whether
/// the Operator stopped it, and which Meetings are its own.
///
/// **The other half of [`MODEL_CHANGE_WIPE`]**, registered directly behind
/// it (Q228): a re-run without the wipe re-diarizes a History whose
/// Voiceprints are still the old model's, and a wipe without the re-run
/// leaves a History nobody is recognized in.
///
/// **The `INSERT` at the end is what turns the wipe into a re-run.** The
/// startup gate, [`rerun::begin_if_the_model_changed`], walks History only
/// when the stored identity differs from the loaded one, and on an absent row
/// it records the loaded identity and asks for nothing — an absent row is
/// every History from before this table existed, and reading it as a change
/// would re-run all of them on an ordinary update (Q221). That guard is right
/// and stays. But it means a History this migration has just wiped, with the
/// table created empty a moment earlier, would be recorded as already in the
/// new space and never walked; the lazy path cannot rescue it either, since
/// the wipe leaves `stale_exemplars` nothing to find. So the migration writes
/// down the one fact it knows — the identity every vector the wipe removed
/// was stamped with, WeSpeaker's — as the row the gate reads at the next
/// start. That row is the wipe's stamp, not a backlog: `total` is zero, so
/// [`rerun::Rerun::requested`] answers false and no Client is shown a phantom
/// re-run in the instant between migrating and the gate. The gate then sees
/// the old identity against the new, calls [`rerun::begin`], and `record`
/// overwrites this row with the real one. A fresh install runs the same
/// sequence on an empty History and walks zero Meetings, which is harmless
/// and is pinned by `rerun::tests`.
///
/// [`rerun::begin_if_the_model_changed`]: super::rerun::begin_if_the_model_changed
/// [`rerun::begin`]: super::rerun::begin
/// [`rerun::Rerun::requested`]: super::rerun::Rerun::requested
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
///
/// It hangs off `diarize_queue` rather than off `meetings`, because what it
/// marks is a *queue row* as the re-run's, not a Meeting as forever the
/// re-run's. A row leaves the line once — walked, skipped, cancelled, or its
/// Meeting deleted — and the cascade retires the membership with it, in
/// whatever transaction removed the row. Hung off `meetings` instead, the
/// membership outlived the work: a Meeting the backlog had finished, enqueued
/// again by hand, would be joined back onto the old membership and counted
/// still owed; cancelling that fresh request would then raise `abandoned` for
/// a walk that had already happened, and a bulk stop would delete a request
/// the re-run never made. The chain through `diarize_queue` still reaches
/// `meetings`, so deleting a Meeting clears both.
pub const MODEL_CHANGE_RERUN: &str = r#"
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
                   REFERENCES diarize_queue(meeting_id) ON DELETE CASCADE
    ) STRICT;

    -- The space the wipe above just emptied, so that the next start reads a
    -- model change rather than a first start. Zero total: a stamp, not a
    -- backlog.
    INSERT INTO diarize_rerun
        (id, model, model_version, total, cancelled, abandoned, started_at)
    VALUES
        (1, 'wespeaker-voxceleb-resnet34-LM', '2', 0, 0, 0,
         strftime('%Y-%m-%dT%H:%M:%S+00:00', 'now'));
"#;

/// 17 — the voice the Operator gave on purpose.
///
/// Every other Voiceprint in this schema is inferred: `diarize::operator`
/// reads the mic channel and decides who owns the laptop, and it can be
/// wrong about a colleague in the room without anything looking wrong
/// afterwards. An enrolment is the other kind of evidence — a person
/// recorded themselves and said so — and it outranks the channel entirely.
///
/// **The row exists for the audio path, and the audio path exists because of
/// [`MODEL_CHANGE_WIPE`].** That migration takes every vector and every
/// exemplar, correctly: old and new embeddings cannot be compared. It leaves
/// the Operator unrecognized until a bulk re-run relearns them from their
/// attributed clusters, which on the first real History cost seventeen
/// minutes and did not return four Meetings at all. A kept clip is immune to
/// that: the vectors go, the audio does not, and the Operator is re-embedded
/// in the new space before the backlog is touched.
///
/// Relative to the History directory, like `meetings.audio_path`, so moving
/// a History moves its enrolment with it.
///
/// **Why the exemplars themselves carry no new marking.** An enrolment's
/// exemplars are ordinary rows with `meeting_id` NULL and `source`
/// `'operator'`, which is already an unambiguous signature: an
/// Operator-sourced exemplar comes from a correction, and a correction is
/// always about a segment of some Meeting. Spelling it a third way would
/// mean a new value in `source`'s `CHECK`, and that is a `STRICT` table —
/// changing the constraint rebuilds it, which is a large and reversible-only
/// -by-restore operation to buy a synonym. This table is the authority; the
/// signature is how the exemplars are found.
const THE_ENROLMENT: &str = r#"
    CREATE TABLE speaker_enrolments (
        speaker_id   TEXT PRIMARY KEY NOT NULL
                     REFERENCES speakers(id) ON DELETE CASCADE,
        -- Where the clip is, relative to the History directory.
        audio_path   TEXT NOT NULL,
        duration_ms  INTEGER NOT NULL,
        recorded_at  TEXT NOT NULL
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

    /// How many migrations run before this one — the schema exactly as it
    /// stood the instant before that upgrade applied.
    ///
    /// Written as bare numbers, the three tests below were correct only by
    /// coincidence of the current order. Each one puts a database into the
    /// state preceding one particular migration and asserts what happens when
    /// it runs, and `MIGRATIONS[..10]` says a position rather than an upgrade:
    /// insert anything ahead of it and the test still passes while silently
    /// being about a different migration, which is the failure a test cannot
    /// report because it no longer knows what it was for.
    ///
    /// Appending — which is all registering the wipe did — never moved them. That is why this is a prefactor rather than a bug fix: it
    /// costs nothing now and removes the trap before anyone goes near the
    /// order.
    fn before(migration: &str) -> usize {
        let mut found = MIGRATIONS
            .iter()
            .enumerate()
            .filter(|(_, candidate)| **candidate == migration);
        let (index, _) = found.next().expect("a migration that is in MIGRATIONS");
        // Two identical bodies would make the answer arbitrary, and would be a
        // bug in its own right: the second could never apply to a History that
        // had the first.
        assert!(
            found.next().is_none(),
            "two migrations with the same body: the index would be a guess"
        );
        index
    }

    /// `before` can only answer if no two migrations are the same text.
    ///
    /// It is the assumption the three upgrade-path tests now rest on, and a
    /// duplicate would be a defect in its own right — the second copy could
    /// never apply to a History that already had the first, so it would sit in
    /// the list doing nothing while still advancing `user_version`.
    #[test]
    fn every_migration_is_distinct_so_naming_one_is_unambiguous() {
        let mut seen = std::collections::BTreeSet::new();
        for migration in MIGRATIONS {
            assert!(
                seen.insert(*migration),
                "two migrations share a body; `before` would be a guess"
            );
        }
        assert_eq!(seen.len(), MIGRATIONS.len());
        // And each name resolves to its own position, in the order the doc
        // comments number them.
        assert_eq!(before(THE_RECORD), 0);
        assert_eq!(before(VOICE_SAMPLES_AND_THE_PRUNE), 9);
        assert_eq!(before(THE_DIARIZATION_MARK), 10);
        assert_eq!(before(DIARIZE_QUEUE), 11);
        assert_eq!(before(ONE_OPERATOR), 13);
        assert_eq!(before(MODEL_CHANGE_WIPE), 14);
        assert_eq!(before(MODEL_CHANGE_RERUN), 15);
    }

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
        // `THE_DIARIZATION_MARK`. Two branches appended migrations after the
        // same base and both wanted position 11; `diarized_at` kept it
        // because it was
        // already pushed, and the queue moved to 12 (DECISIONS Q134).
        //
        // Had the order gone the other way, a database sitting at
        // user_version 11 would have counted `diarize_queue` as already
        // applied and skipped it for good — surfacing much later, and far
        // from here, as a table that does not exist.
        let mut connection = Connection::open_in_memory().expect("open");
        configure(&connection).expect("configure");
        let shipped = before(DIARIZE_QUEUE);
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
        let before_mark = before(THE_DIARIZATION_MARK);
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
        // `VOICE_SAMPLES_AND_THE_PRUNE` runs once over a History that
        // already holds the Speakers the old policy minted. Everything the
        // record points at has to survive it: an attributed voice, a
        // corrected one, a named one, the Operator. Only the row nobody
        // references goes.
        let mut connection = Connection::open_in_memory().expect("open");
        configure(&connection).expect("configure");
        // Up to the migration before the prune, then seed, then prune.
        let before_prune = before(VOICE_SAMPLES_AND_THE_PRUNE);
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

    /// The wipe and the re-run are one upgrade: adjacent, and in that order.
    ///
    /// Until 2026-09-17 two tests here asserted the opposite — that neither
    /// was in `MIGRATIONS` — because appending them is the whole of activating
    /// them and a stray paste would have cleared Voiceprints with no swap
    /// behind it. The swap happened (Q226) and the user said to register
    /// (Q228). What is left to guard is the pairing: a wipe registered without
    /// its re-run directly behind it leaves a History nobody is recognized in.
    ///
    /// This also asserted the pair was *last*, which it was when it was
    /// written and which the reason above never needed: the danger is a
    /// migration landing *between* them, not one landing after. Relaxed when
    /// `THE_ENROLMENT` was appended behind them.
    #[test]
    fn the_wipe_and_the_rerun_are_registered_as_a_pair() {
        let wipe = before(MODEL_CHANGE_WIPE);
        let rerun = before(MODEL_CHANGE_RERUN);
        assert_eq!(rerun, wipe + 1, "the re-run is the other half, and follows");
    }

    /// A History as the current build leaves one, on disk.
    ///
    /// A named Speaker with a Voiceprint and both signs of evidence, the
    /// Operator, a forgotten Speaker, a named Speaker who was never
    /// enrolled, an attributed segment and a correction hint — one of each
    /// thing the wipe promises to keep or to take.
    fn populated_history(path: &std::path::Path) -> (String, String, String, String, String) {
        populated_history_at(path, MIGRATIONS.len())
    }

    /// The same History as the build that had applied only the first
    /// `applied` migrations left it — the shape in the field the instant
    /// before an upgrade. Its evidence is stamped with the identity that build
    /// wrote: before the wipe that is WeSpeaker's, since a History from before
    /// the swap has nothing in the current space, and stamping it as if it
    /// did would pass the upgrade test on a History that cannot exist.
    fn populated_history_at(
        path: &std::path::Path,
        applied: usize,
    ) -> (String, String, String, String, String) {
        use crate::store::speakers::{self, NewExemplar};
        use rusqlite::params;

        let (embedding_model, embedding_model_version) = if applied <= before(MODEL_CHANGE_WIPE) {
            ("wespeaker-voxceleb-resnet34-LM", "2")
        } else {
            (
                crate::diarize::live::EMBEDDING_MODEL,
                crate::diarize::live::EMBEDDING_MODEL_VERSION,
            )
        };

        let connection = Connection::open(path).expect("open");
        configure(&connection).expect("configure");
        for migration in &MIGRATIONS[..applied] {
            connection.execute_batch(migration).expect("migrate");
        }
        connection
            .pragma_update(None, "user_version", applied as i64)
            .expect("user_version");

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
                        model: embedding_model,
                        model_version: embedding_model_version,
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
                embedding_model,
                embedding_model_version,
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

    /// Ticket 05 on disk, through the real upgrade: a History as the last
    /// WeSpeaker build left it, opened by this one. The vectors go, the record
    /// stays, and the row the re-run needs is there.
    #[test]
    fn upgrading_takes_every_vector_keeps_the_record_and_stamps_the_old_space() {
        use crate::diarize::live::{EMBEDDING_MODEL, EMBEDDING_MODEL_VERSION};
        use crate::store::speakers;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("history.sqlite3");
        let (alice, me, gone, fresh, segment) =
            populated_history_at(&path, before(MODEL_CHANGE_WIPE));

        let mut connection = Connection::open(&path).expect("reopen");
        configure(&connection).expect("configure");
        migrate(&mut connection).expect("upgrade");
        let version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("user_version");
        assert_eq!(version as usize, MIGRATIONS.len());

        record_survives(&connection, &alice, &me, &gone, &fresh, &segment);

        let stamp: (String, String, i64, i64, i64) = connection
            .query_row(
                "SELECT model, model_version, total, cancelled, abandoned
                   FROM diarize_rerun WHERE id = 1",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .expect("the wipe's stamp");
        assert_eq!(
            stamp,
            ("wespeaker-voxceleb-resnet34-LM".into(), "2".into(), 0, 0, 0),
            "the space the wipe emptied, as a zero-total row the next start reads as a change"
        );

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
                (Some("wespeaker-voxceleb-resnet34-LM"), Some("2")),
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
