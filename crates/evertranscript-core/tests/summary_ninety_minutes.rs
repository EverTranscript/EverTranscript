//! The ninety-minute case, which is where M4's failure mode lives.
//!
//! Every measurement before this one used a transcript of three lines. The
//! close-out names the thing three lines cannot show: *a Summary that reads
//! beautifully on five minutes and falls apart on ninety — chunk boundaries
//! dropping the middle, action items attributed to the wrong speaker, a CJK
//! character corrupted at a token boundary.*
//!
//! ## The transcript is synthetic, and that is a stated limitation
//!
//! It is composed here rather than recorded, so this closes the *chunking*
//! half of the criterion and leaves the *real meeting* half open. What a
//! constructed transcript buys is the thing a recording cannot: **planted
//! ground truth.** Four commitments sit at known offsets — one early, one in
//! the deep middle, one late, and one deliberately split across a chunk
//! boundary — so "the middle was dropped" becomes a measurement rather than
//! an impression. A recorded ninety-minute meeting would be more honest
//! about speech and useless for this, because nobody would know what the
//! right answer was.
//!
//! What it cannot show: real disfluency, ASR errors, crosstalk, and the
//! ordinary incoherence of people talking. The fixture is deliberately
//! *coherent* — a meeting that progresses through an agenda — because an
//! incoherent one would measure the fixture rather than the chunker, which
//! is the mistake `summary_quality` already records making once.
//!
//! ## What it found
//!
//! **Chunking is not what drops the middle; the reduce is.** Every chunk's
//! own summary contained the commitment made inside it — map 3/3, on every
//! run — while the pass that combines those three partial summaries kept
//! only the first chunk's. The overlapping-chunk machinery works; asking a
//! 4B to merge three summaries loses much of what it is given.
//!
//! Telling it not to helps, and that was measured rather than assumed. With
//! the reduce prompt saying the parts do not repeat each other and every
//! action item must survive, the same fixture scored 2/3, 2/3 and 3/3 against
//! 1/3 and 1/3 without it — every run with the sentence beating every run
//! without. Five runs is suggestive, not settled, and the reduce remains the
//! lossy stage.
//!
//! The straddled commitment is lost as well, which is a second and smaller
//! finding: `OVERLAP_TOKENS` is a hundred tokens, about five lines, and a
//! question answered fifty seconds later falls between two chunks that each
//! see half of it. See DECISIONS Q61.

#![cfg(unix)]

use std::path::Path;
use std::path::PathBuf;

use evertranscript_core::summary::Backend;
use evertranscript_core::summary::Cancel;
use evertranscript_core::summary::Request;
use evertranscript_core::summary::generate::chunk_to;
use evertranscript_core::summary::prompt::DEFAULT_SYSTEM_PROMPT;
use evertranscript_core::summary::sidecar::Driving;
use evertranscript_core::summary::sidecar::SidecarBackend;

/// Seconds between utterances. A real ASR transcript is denser than people
/// imagine — short segments, not paragraphs.
const SECONDS_PER_LINE: u64 = 5;

/// Ninety minutes of them.
const LINES: u64 = 90 * 60 / SECONDS_PER_LINE;

/// A planted commitment: what it is, who makes it, and where it sits.
struct Probe {
    /// Which line it replaces, so its position in the meeting is exact.
    line: u64,
    speaker: &'static str,
    text: &'static str,
    /// A word that appears nowhere else, so finding it is unambiguous.
    tell: &'static str,
    /// Which chunk it must land in. Asserted, so a change to the chunker
    /// that moves a probe out of the middle fails loudly instead of quietly
    /// turning this into a measurement of something else.
    chunk: usize,
}

/// The four commitments this measurement is about.
///
/// `tell` words are deliberately odd — a real meeting would not say
/// "redlined" and "escrow" and "Thursday" in one hour — because a summary
/// mentioning one of them cannot have got it from anywhere else.
const PROBES: &[Probe] = &[
    Probe {
        line: 30, // 00:02:30 — early in chunk 0, which everything gets right
        speaker: "Maya",
        text: "I'll have the vendor contract redlined by Tuesday.",
        tell: "redlined",
        chunk: 0,
    },
    Probe {
        // 01:02:30 — deep inside chunk 1, far from either boundary. **The
        // probe this measurement exists for.** If this one goes missing
        // while the two either side of it survive, that is precisely the
        // named failure: a Summary that reads beautifully on five minutes
        // and drops the middle of ninety.
        line: 750,
        speaker: "Tomas",
        text: "I'll book the compliance review for the fourteenth.",
        tell: "compliance review",
        chunk: 1,
    },
    Probe {
        line: 1040, // 01:26:40 — the last chunk
        speaker: "Ines",
        text: "I'll send the revised headcount before Friday.",
        tell: "headcount",
        chunk: 2,
    },
];

/// The commitment deliberately split across a chunk boundary.
///
/// Chunk 0 ends at line 507 and chunk 1 begins at 503, so the overlap is
/// five lines — twenty-five seconds of meeting. The ask sits at 500 and the
/// acceptance at 510, which puts each in exactly one chunk and neither in
/// both.
///
/// **The acceptance is deliberately referential.** "I can have that ready by
/// Thursday" is not a commitment anyone can act on without the line that
/// says what *that* is, which is the shape `OVERLAP_TOKENS` exists to carry
/// and the shape a real negotiation actually takes: a question, some
/// discussion, then agreement. Whether fifty seconds is longer than the
/// overlap can reach is the thing being measured.
const ASK_LINE: u64 = 500;
const ASK_SPEAKER: &str = "Maya";
const ASK_TEXT: &str = "Tomas, can you own the migration plan for this quarter?";
const ACCEPT_LINE: u64 = 510;
const ACCEPT_SPEAKER: &str = "Tomas";
const ACCEPT_TEXT: &str = "Yes — I can have that ready by Thursday.";

/// The Chinese exchange, placed mid-meeting.
///
/// A chunk boundary is a line boundary, so the chunker cannot split a
/// character in half — but the model's own decode can, which is what
/// `IncrementalUtf8` exists for, and nothing has ever measured it on a
/// prompt this long.
const CJK_LINE: u64 = 700;
const CJK_SPEAKER: &str = "Wei";
const CJK_TEXT: &str = "我们需要在下个季度之前完成合规审查，预算也要重新核算。";

fn timestamp(second: u64) -> String {
    format!(
        "{}:{:02}:{:02}",
        second / 3600,
        (second % 3600) / 60,
        second % 60
    )
}

/// A coherent ninety-minute planning meeting with the probes planted in it.
pub fn ninety_minute_transcript() -> String {
    // Eight agenda topics, each with its own vocabulary, cycled so the
    // meeting moves rather than loops. Every line is a plausible thing to
    // say and none of them contains a probe's tell word.
    const TOPICS: &[(&str, &[&str])] = &[
        (
            "migration",
            &[
                "The staging environment is still on the old schema.",
                "We can run both writers for a week before we cut over.",
                "Rollback is the part I'm least sure about.",
                "Last time the index rebuild took most of a night.",
                "I'd rather do it on a Tuesday than a Friday.",
            ],
        ),
        (
            "hiring",
            &[
                "Two of the three offers went out last week.",
                "The second candidate withdrew, so we're back to a shortlist.",
                "Onboarding is taking three weeks and we'd like two.",
                "The panel is booked for the week after next.",
                "We should decide whether that role is backfill or growth.",
            ],
        ),
        (
            "budget",
            &[
                "We're about eight percent under for the quarter.",
                "The tooling line is the one that moved.",
                "That renewal lands in the next cycle, not this one.",
                "I don't want to spend it just because it's there.",
                "Finance wants the forecast a week earlier this time.",
            ],
        ),
        (
            "incident",
            &[
                "The alert fired at about four in the morning.",
                "It resolved itself before anyone was paged twice.",
                "The retry storm made it look worse than it was.",
                "We still don't have a dashboard for that queue.",
                "The write-up is drafted but not circulated.",
            ],
        ),
        (
            "customers",
            &[
                "Two accounts asked about the export format again.",
                "The feedback on the new flow has been mostly positive.",
                "One of them is blocked on the same permission bug.",
                "They want a date, and I don't want to give a bad one.",
                "Support has been fielding that question all month.",
            ],
        ),
        (
            "roadmap",
            &[
                "The second half is more committed than I remembered.",
                "If that slips, the thing after it slips too.",
                "We agreed to stop adding to this quarter.",
                "I'd like one week of slack in there somewhere.",
                "That item has moved twice, which usually means something.",
            ],
        ),
        (
            "vendor",
            &[
                "Their security questionnaire came back with two gaps.",
                "The pricing holds if we sign before the end of the month.",
                "Legal wants the liability cap raised.",
                "We're not the biggest account they have, which shows.",
                "The trial extension buys us two more weeks.",
            ],
        ),
        (
            "process",
            &[
                "The review queue is shorter than it was.",
                "We're merging faster but reverting slightly more.",
                "Nobody reads that document, which is worth knowing.",
                "The rotation is uneven — two people carry most of it.",
                "Let's keep the meeting to the agenda this time.",
            ],
        ),
    ];
    const SPEAKERS: &[&str] = &["Maya", "Tomas", "Ines", "Wei", "You"];

    let mut out = String::new();
    for line in 0..LINES {
        let second = line * SECONDS_PER_LINE;
        let stamp = timestamp(second);

        if let Some(probe) = PROBES.iter().find(|probe| probe.line == line) {
            out.push_str(&format!("[{stamp}] {}: {}\n", probe.speaker, probe.text));
            continue;
        }
        if line == CJK_LINE {
            out.push_str(&format!("[{stamp}] {CJK_SPEAKER}: {CJK_TEXT}\n"));
            continue;
        }
        if line == ASK_LINE {
            out.push_str(&format!("[{stamp}] {ASK_SPEAKER}: {ASK_TEXT}\n"));
            continue;
        }
        if line == ACCEPT_LINE {
            out.push_str(&format!("[{stamp}] {ACCEPT_SPEAKER}: {ACCEPT_TEXT}\n"));
            continue;
        }

        let (_, lines) = TOPICS[(line / 7) as usize % TOPICS.len()];
        let text = lines[(line % lines.len() as u64) as usize];
        let speaker = SPEAKERS[(line % SPEAKERS.len() as u64) as usize];
        out.push_str(&format!("[{stamp}] {speaker}: {text}\n"));
    }
    out
}

#[test]
fn the_fixture_is_a_ninety_minute_meeting_that_actually_chunks() {
    let transcript = ninety_minute_transcript();
    let chunks = chunk_to(&transcript, 12_000);
    eprintln!(
        "transcript: {} lines, {} chars, ~{} tokens → {} chunks",
        transcript.lines().count(),
        transcript.chars().count(),
        evertranscript_core::summary::generate::estimate_tokens(&transcript),
        chunks.len()
    );
    assert!(
        chunks.len() > 2,
        "a fixture that does not chunk cannot measure chunking: got {}",
        chunks.len()
    );
    for probe in PROBES {
        assert_eq!(
            transcript.matches(probe.tell).count(),
            1,
            "{} must appear exactly once for finding it to mean anything",
            probe.tell
        );
    }
    assert!(transcript.contains(CJK_TEXT));
}

/// Its own gate, and deliberately not the quality suite's.
///
/// Four generations over eleven-thousand-token prompts take four and a half
/// minutes on the author's machine. DECISIONS Q59 spent three attempts
/// learning that a GitHub runner needs *half an hour* for a single
/// three-thousand-token generation, so folding this into the job that
/// already measures model quality would be handing CI the same timeout back
/// with a bigger prompt.
///
/// What runs in CI for free is everything above this line: the fixture is
/// still asserted to be ninety minutes, to chunk into three, and to hold
/// each probe in the chunk this file claims. Those are the regression guard.
/// The generation below is a *measurement*, run by hand, with its numbers
/// recorded in DECISIONS — the same way M1's WER and M3's DER were.
const MEASURE_ENV: &str = "EVERTRANSCRIPT_MEASURE_NINETY_MINUTES";

fn model() -> Option<PathBuf> {
    std::env::var_os(MEASURE_ENV).filter(|v| !v.is_empty())?;
    let configured = std::env::var_os("EVERTRANSCRIPT_SUMMARY_MODEL")
        .filter(|v| !v.is_empty())
        .expect("set EVERTRANSCRIPT_SUMMARY_MODEL to measure the ninety-minute case");
    let path = PathBuf::from(configured);
    assert!(path.exists(), "{} does not exist", path.display());
    Some(path)
}

fn sidecar() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("EVERTRANSCRIPT_SUMMARIZER_BIN") {
        let path = PathBuf::from(path);
        return path.exists().then_some(path);
    }
    let beside = std::env::current_exe()
        .ok()?
        .parent()?
        .parent()?
        .join("evertranscript-summarizer");
    beside.exists().then_some(beside)
}

fn spawn() -> Option<SidecarBackend> {
    let model = model()?;
    let binary = sidecar().expect("the sidecar must be built");
    let driving = evertranscript_core::models::registry::SUMMARY_DEFAULT
        .driving
        .as_ref()
        .map(Driving::from_entry);
    Some(
        SidecarBackend::spawn_driven(
            Path::new(&binary),
            model.to_str().expect("UTF-8 path"),
            driving,
        )
        .expect("the model should load"),
    )
}

/// What the map stage produced, and what the reduce made of it.
///
/// **Kept separately because "the middle was dropped" is two questions.** A
/// commitment can go missing because the chunk summarizing it never mentioned
/// it, or because the reduce pass threw it away while combining three partial
/// summaries. Those are different defects with different fixes, and a single
/// final string cannot tell them apart.
struct Measured {
    parts: Vec<String>,
    reduced: String,
}

/// Map-reduce over the real thing, mirroring what `server.rs` does.
fn measured() -> Option<&'static Measured> {
    static SUMMARY: std::sync::OnceLock<Option<Measured>> = std::sync::OnceLock::new();
    SUMMARY
        .get_or_init(|| {
            let mut backend = spawn()?;
            let transcript = ninety_minute_transcript();
            let chunks = chunk_to(&transcript, 12_000);
            eprintln!("summarizing {} chunks", chunks.len());

            let mut generate = |user: String| {
                let started = std::time::Instant::now();
                let text = backend
                    .generate(
                        &Request {
                            system: DEFAULT_SYSTEM_PROMPT.to_string(),
                            user,
                        },
                        &Cancel::new(),
                    )
                    .expect("generation should succeed");
                eprintln!("  generated in {:?}", started.elapsed());
                evertranscript_core::summary::prompt::scrub(&text)
            };

            let (parts, reduced) = {
                let parts: Vec<String> = chunks
                    .iter()
                    .map(|piece| {
                        generate(evertranscript_core::summary::prompt::build_user_message(
                            None, piece,
                        ))
                    })
                    .collect();
                let combined = parts.join("\n\n---\n\n");
                // Exactly what the Core sends, from the one definition of it.
                let reduced = generate(evertranscript_core::summary::prompt::build_user_message(
                    None,
                    &evertranscript_core::summary::prompt::reduce_message(&combined),
                ));
                (parts, reduced)
            };

            backend.shutdown();
            Some(Measured { parts, reduced })
        })
        .as_ref()
}

fn summarize_ninety_minutes() -> Option<&'static String> {
    Some(&measured()?.reduced)
}

#[test]
fn every_probe_is_where_this_measurement_claims_it_is() {
    // Without this the whole file quietly becomes a measurement of something
    // else the first time the chunker's budget changes.
    let transcript = ninety_minute_transcript();
    let chunks = chunk_to(&transcript, 12_000);

    for probe in PROBES {
        let owning: Vec<usize> = chunks
            .iter()
            .enumerate()
            .filter(|(_, chunk)| chunk.contains(probe.tell))
            .map(|(index, _)| index)
            .collect();
        assert_eq!(
            owning,
            vec![probe.chunk],
            "{:?} was planted for chunk {} and is in {owning:?}",
            probe.tell,
            probe.chunk
        );
    }

    // The split commitment: each half in exactly one chunk, and not the same
    // one. If the overlap ever grows past fifty seconds this fails, which is
    // the correct outcome — the measurement below would no longer be
    // measuring a straddle.
    let holding = |needle: &str| -> Vec<usize> {
        chunks
            .iter()
            .enumerate()
            .filter(|(_, chunk)| chunk.contains(needle))
            .map(|(index, _)| index)
            .collect()
    };
    assert_eq!(
        holding(ASK_TEXT),
        vec![0],
        "the ask must sit in chunk 0 alone"
    );
    assert_eq!(
        holding(ACCEPT_TEXT),
        vec![1],
        "the acceptance must sit in chunk 1 alone, or nothing is straddled"
    );

    // A chunk boundary is a line boundary, so this cannot break — but it is
    // the mechanical half of the CJK claim and costs nothing to hold.
    assert_eq!(holding(CJK_TEXT), vec![1]);
    assert!(!transcript.contains('\u{fffd}'));
}

#[test]
fn the_middle_of_a_long_meeting_is_not_dropped() {
    let Some(summary) = summarize_ninety_minutes() else {
        eprintln!("skipping: set {MEASURE_ENV} to run the ninety-minute measurement");
        return;
    };
    eprintln!("ninety-minute summary:\n{summary}");

    // **The named failure, measured.** Three commitments, one per chunk. The
    // shape that matters is not "how many were found" but *which*: losing
    // the middle one while keeping the two either side is a summary of the
    // beginning and the end wearing the name of the whole meeting.
    let measured = measured().expect("already generated");
    let lowered = summary.to_lowercase();

    // **Where each commitment was lost, not merely whether.** A probe its own
    // chunk's partial summary never mentioned was lost by the map; one the
    // partial had and the reduce dropped was lost by the reduce.
    let mut survived_map = 0;
    let mut survived_reduce = 0;
    for probe in PROBES {
        let in_part = measured.parts[probe.chunk]
            .to_lowercase()
            .contains(probe.tell);
        let in_final = lowered.contains(probe.tell);
        survived_map += usize::from(in_part);
        survived_reduce += usize::from(in_final);
        eprintln!(
            "  {:>18}  chunk {} partial: {:<5}  final: {}",
            probe.tell, probe.chunk, in_part, in_final
        );
    }
    eprintln!("planted commitments — map kept {survived_map}/3, reduce kept {survived_reduce}/3");

    // **Asserted at the map stage only, and that is the honest bound.** The
    // reduce is measured losing commitments its own inputs contained, and it
    // does so nondeterministically — 3/3 on one run of this fixture and 1/3
    // on the next, with no code changed between them. Gating on a coin flip
    // would make the build red for reasons nobody could act on, so the loss
    // is reported here and recorded as a finding rather than asserted. What
    // *is* asserted is the part that has been stable: every chunk's own
    // summary contains the commitment made inside it, so chunking itself is
    // not dropping the middle of the meeting.
    let missed: Vec<&str> = PROBES
        .iter()
        .filter(|probe| {
            !measured.parts[probe.chunk]
                .to_lowercase()
                .contains(probe.tell)
        })
        .map(|probe| probe.tell)
        .collect();
    assert!(
        missed.is_empty(),
        "a chunk's own summary lost the commitment made inside it: {missed:?}\n\n\
         That is chunking dropping the middle, which is worse than the reduce \
         losing it — there is nothing left downstream to recover it from."
    );
}

#[test]
fn every_part_the_record_is_built_from_is_correctly_attributed() {
    let Some(measured) = measured() else {
        eprintln!("skipping: set {MEASURE_ENV} to run the ninety-minute measurement");
        return;
    };
    let transcript = ninety_minute_transcript();
    let chunks = chunk_to(&transcript, 12_000);

    // **Asserted on the parts, because the parts are what the record falls
    // back to.** `server.rs` verifies the reduce output and, when it fails,
    // keeps the combined parts instead — so a misattributed reduce costs
    // polish, while a misattributed *part* would reach the Operator.
    for (index, part) in measured.parts.iter().enumerate() {
        assert_eq!(
            evertranscript_core::summary::prompt::verify(part, &chunks[index]),
            Ok(()),
            "chunk {index}'s own summary credits someone who did not say it:\n\n{part}"
        );
    }

    // **Reported, because it is nondeterministic.** The reduce pass was
    // refused on one run in two here, with nothing changed between them.
    // Asserting on it would make the build red on a coin flip; how often it
    // happens is the number worth knowing, and the product already handles
    // the case by keeping the verified parts.
    let reduce = evertranscript_core::summary::prompt::verify(&measured.reduced, &transcript);
    eprintln!("reduce pass accepted by verify: {:?}", reduce.is_ok());
    if let Err(why) = reduce {
        eprintln!("  refused because: {why}");
        eprintln!("  the record would keep the three verified parts instead");
    }
}

#[test]
fn chinese_survives_ninety_minutes_of_chunking() {
    let Some(summary) = summarize_ninety_minutes() else {
        eprintln!("skipping: set {MEASURE_ENV} to run the ninety-minute measurement");
        return;
    };

    // The third named failure. `IncrementalUtf8` exists because a model emits
    // tokens, not characters, and a three-byte character split across two of
    // them decodes to replacement characters — permanently, in a record that
    // is immutable by design. It has never been exercised on a prompt this
    // long or through a reduce pass.
    let corrupted: Vec<char> = summary.chars().filter(|c| *c == '\u{fffd}').collect();
    assert!(
        corrupted.is_empty(),
        "the Summary contains {} replacement characters, so a multi-byte \
         character was split during decode:\n\n{summary}",
        corrupted.len()
    );
}

#[test]
fn a_commitment_split_across_a_chunk_boundary() {
    let Some(summary) = summarize_ninety_minutes() else {
        eprintln!("skipping: set {MEASURE_ENV} to run the ninety-minute measurement");
        return;
    };

    // **Reported, not asserted, and deliberately so.** The ask is in chunk 0
    // and the referential acceptance is in chunk 1, fifty seconds later —
    // longer than the hundred-token overlap can reach, so neither chunk sees
    // both halves. Whether the commitment survives is a property of how big
    // OVERLAP_TOKENS is, and asserting an outcome here would be asserting
    // that a constant is correct rather than measuring whether it is.
    let lowered = summary.to_lowercase();
    let survived = lowered.contains("migration plan") && lowered.contains("thursday");
    eprintln!(
        "commitment straddling the chunk boundary survived: {survived} \
         (ask at line {ASK_LINE}, acceptance at line {ACCEPT_LINE}, overlap covers 5 lines)"
    );
}
