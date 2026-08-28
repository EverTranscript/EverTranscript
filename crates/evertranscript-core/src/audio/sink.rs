//! Writing a Meeting's audio to disk, crash-safely (ADR-0032).
//!
//! Audio is encoded in fixed checkpoints rather than one long stream, so a
//! Core that dies mid-meeting leaves a set of complete, playable segments
//! instead of a truncated file with no moov atom. On the next start those
//! segments are losslessly concatenated: at most the current checkpoint's
//! worth of audio is ever at risk.
//!
//! Output is one stereo AAC file per Meeting — **left = mic, right = system**
//! — so Enhance-era re-transcription and re-diarization get cleanly separated
//! sources forever.

use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;

use anyhow::Context;
use anyhow::Result;
use tokio::io::AsyncWriteExt;
use tokio::process::Child;
use tokio::process::Command;
use tracing::debug;
use tracing::info;
use tracing::warn;

use super::joiner::StereoBlock;
use super::SAMPLE_RATE;

/// How much audio each checkpoint holds. The upper bound on what a crash can
/// cost, traded against one process spawn per interval.
const CHECKPOINT_SECONDS: u64 = 30;

const CHECKPOINT_SAMPLES: usize = (CHECKPOINT_SECONDS * SAMPLE_RATE as u64) as usize;

/// Where to find ffmpeg. Packaging bundles it beside the binary; this
/// override lets a dev build use the one on PATH.
pub const FFMPEG_ENV: &str = "EVERTRANSCRIPT_FFMPEG";

pub fn ffmpeg_binary() -> String {
    std::env::var(FFMPEG_ENV).unwrap_or_else(|_| "ffmpeg".to_string())
}

/// True when an encoder is actually available. Capture still runs without
/// one — the record is the transcript; audio is a bonus (ADR-0019) — so this
/// is reported, not fatal.
pub async fn ffmpeg_available() -> bool {
    Command::new(ffmpeg_binary())
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map(|status| status.success())
        .unwrap_or(false)
}

struct Encoder {
    child: Child,
    path: PathBuf,
    samples_written: usize,
}

impl Encoder {
    async fn spawn(path: &Path) -> Result<Self> {
        let mut child = Command::new(ffmpeg_binary())
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                // Raw interleaved f32 stereo at the capture rate.
                "-f",
                "f32le",
                "-ar",
                &SAMPLE_RATE.to_string(),
                "-ac",
                "2",
                "-i",
                "pipe:0",
                "-c:a",
                "aac",
                "-b:a",
                "192k",
                "-profile:a",
                "aac_low",
                "-movflags",
                "+faststart",
                "-y",
            ])
            .arg(path)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| {
                format!(
                    "starting {} — set {FFMPEG_ENV} if it is not on PATH",
                    ffmpeg_binary()
                )
            })?;
        // Take stderr so a chatty encoder cannot fill its pipe and block.
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(async move {
                use tokio::io::AsyncBufReadExt;
                let mut lines = tokio::io::BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    warn!(target: "ffmpeg", "{line}");
                }
            });
        }
        Ok(Self {
            child,
            path: path.to_path_buf(),
            samples_written: 0,
        })
    }

    async fn write(&mut self, samples: &[f32]) -> Result<()> {
        let stdin = self
            .child
            .stdin
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("the encoder's stdin is gone"))?;
        let mut bytes = Vec::with_capacity(samples.len() * 4);
        for sample in samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        stdin.write_all(&bytes).await?;
        self.samples_written += samples.len();
        Ok(())
    }

    /// Closes stdin and waits for ffmpeg to finish the file.
    async fn finish(mut self) -> Result<PathBuf> {
        drop(self.child.stdin.take());
        let status = self.child.wait().await?;
        if !status.success() {
            anyhow::bail!(
                "ffmpeg exited with {status} writing {}",
                self.path.display()
            );
        }
        Ok(self.path)
    }
}

/// Writes one Meeting's audio.
pub struct CheckpointSink {
    checkpoint_dir: PathBuf,
    final_path: PathBuf,
    encoder: Option<Encoder>,
    next_index: usize,
    total_samples: usize,
    disabled: bool,
}

impl CheckpointSink {
    /// Creates a sink for `meeting_key` (the id8) under `audio_dir`.
    pub fn new(audio_dir: &Path, meeting_key: &str) -> Result<Self> {
        let checkpoint_dir = checkpoint_dir(audio_dir, meeting_key);
        std::fs::create_dir_all(&checkpoint_dir)
            .with_context(|| format!("creating {}", checkpoint_dir.display()))?;
        Ok(Self {
            checkpoint_dir,
            final_path: audio_dir.join(format!("{meeting_key}.m4a")),
            encoder: None,
            next_index: 0,
            total_samples: 0,
            disabled: false,
        })
    }

    pub fn final_path(&self) -> &Path {
        &self.final_path
    }

    /// True once encoding has failed and audio is being dropped. The Meeting
    /// keeps recording — losing the audio bonus must not lose the transcript.
    pub fn is_disabled(&self) -> bool {
        self.disabled
    }

    pub fn seconds_written(&self) -> f64 {
        self.total_samples as f64 / (SAMPLE_RATE as f64 * 2.0)
    }

    /// Appends a stereo block, rolling to a new checkpoint when the current
    /// one is full.
    pub async fn write(&mut self, block: &StereoBlock) -> Result<()> {
        if self.disabled || block.samples.is_empty() {
            return Ok(());
        }
        if let Err(error) = self.write_inner(block).await {
            // An encoder that will not run is a degraded recording, not a
            // failed one (ADR-0019: the record is the transcript).
            warn!(%error, "audio encoding failed; continuing without audio for this Meeting");
            self.disabled = true;
            self.encoder = None;
        }
        Ok(())
    }

    async fn write_inner(&mut self, block: &StereoBlock) -> Result<()> {
        if self.encoder.is_none() {
            let path = self.checkpoint_path(self.next_index);
            self.encoder = Some(Encoder::spawn(&path).await?);
            self.next_index += 1;
        }
        let encoder = self.encoder.as_mut().expect("just created");
        encoder.write(&block.samples).await?;
        self.total_samples += block.samples.len();

        if encoder.samples_written >= CHECKPOINT_SAMPLES * 2 {
            let encoder = self.encoder.take().expect("present");
            let path = encoder.finish().await?;
            debug!(path = %path.display(), "checkpoint sealed");
        }
        Ok(())
    }

    /// Seals the last checkpoint and merges everything into the final file.
    pub async fn finalize(mut self) -> Result<Option<PathBuf>> {
        if let Some(encoder) = self.encoder.take() {
            if let Err(error) = encoder.finish().await {
                warn!(%error, "the last checkpoint did not seal cleanly");
            }
        }
        let merged = merge_checkpoints(&self.checkpoint_dir, &self.final_path).await?;
        if merged.is_some() {
            if let Err(error) = std::fs::remove_dir_all(&self.checkpoint_dir) {
                warn!(%error, "could not clean up the checkpoint directory");
            }
        }
        Ok(merged)
    }
}

fn checkpoint_dir(audio_dir: &Path, meeting_key: &str) -> PathBuf {
    audio_dir.join(".checkpoints").join(meeting_key)
}

impl CheckpointSink {
    fn checkpoint_path(&self, index: usize) -> PathBuf {
        self.checkpoint_dir
            .join(format!("audio_chunk_{index:03}.m4a"))
    }
}

/// Concatenates a Meeting's checkpoints into one file without re-encoding.
async fn merge_checkpoints(checkpoint_dir: &Path, destination: &Path) -> Result<Option<PathBuf>> {
    let mut chunks: Vec<PathBuf> = match std::fs::read_dir(checkpoint_dir) {
        Ok(entries) => entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "m4a"))
            .collect(),
        Err(_) => return Ok(None),
    };
    if chunks.is_empty() {
        return Ok(None);
    }
    // Filenames are zero-padded, so lexical order is chronological order.
    chunks.sort();

    // A checkpoint that never sealed has no usable container; skipping it is
    // what makes recovery "lose at most the last chunk" rather than fail.
    chunks.retain(|path| {
        std::fs::metadata(path)
            .map(|m| m.len() > 0)
            .unwrap_or(false)
    });
    if chunks.is_empty() {
        return Ok(None);
    }

    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let list_path = checkpoint_dir.join("concat.txt");
    let list = chunks
        .iter()
        .map(|path| format!("file '{}'", path.display()))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&list_path, list)?;

    let status = Command::new(ffmpeg_binary())
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "concat",
            "-safe",
            "0",
            "-i",
        ])
        .arg(&list_path)
        .args(["-c", "copy", "-y"])
        .arg(destination)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .context("running ffmpeg to merge checkpoints")?;

    if !status.success() {
        anyhow::bail!("merging checkpoints failed with {status}");
    }
    info!(
        path = %destination.display(),
        chunks = chunks.len(),
        "audio finalized"
    );
    Ok(Some(destination.to_path_buf()))
}

/// What a recovery pass found.
#[derive(Debug, PartialEq)]
pub enum Recovery {
    /// No interrupted recording was left behind.
    Nothing,
    /// Checkpoints were merged into a playable file.
    Recovered { path: PathBuf, chunks: usize },
}

/// Merges any checkpoints a previous Core left behind.
///
/// Called at startup: a Meeting whose Core was killed mid-recording gets its
/// audio back, minus at most the checkpoint that was in flight.
pub async fn recover_interrupted(audio_dir: &Path) -> Result<Vec<Recovery>> {
    let root = audio_dir.join(".checkpoints");
    let Ok(entries) = std::fs::read_dir(&root) else {
        return Ok(Vec::new());
    };

    let mut recoveries = Vec::new();
    for entry in entries.flatten() {
        if !entry.path().is_dir() {
            continue;
        }
        let meeting_key = entry.file_name().to_string_lossy().to_string();
        let destination = audio_dir.join(format!("{meeting_key}.m4a"));
        let chunks = std::fs::read_dir(entry.path())
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "m4a"))
                    .count()
            })
            .unwrap_or(0);

        match merge_checkpoints(&entry.path(), &destination).await {
            Ok(Some(path)) => {
                let _ = std::fs::remove_dir_all(entry.path());
                info!(
                    meeting = meeting_key,
                    chunks, "recovered audio from an interrupted recording"
                );
                recoveries.push(Recovery::Recovered { path, chunks });
            }
            Ok(None) => {
                let _ = std::fs::remove_dir_all(entry.path());
            }
            Err(error) => {
                warn!(meeting = meeting_key, %error, "could not recover this recording's audio");
            }
        }
    }
    Ok(recoveries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::CaptureOffset;

    fn tone(ms: u64) -> StereoBlock {
        let frames = (SAMPLE_RATE as u64 * ms / 1000) as usize;
        let mut samples = Vec::with_capacity(frames * 2);
        for index in 0..frames {
            let phase = index as f32 / SAMPLE_RATE as f32;
            samples.push((phase * 440.0 * std::f32::consts::TAU).sin() * 0.3);
            samples.push((phase * 660.0 * std::f32::consts::TAU).sin() * 0.3);
        }
        StereoBlock {
            offset: CaptureOffset::ZERO,
            samples,
        }
    }

    async fn skip_without_ffmpeg() -> bool {
        if ffmpeg_available().await {
            return false;
        }
        eprintln!("skipping: ffmpeg is not available on this machine");
        true
    }

    #[tokio::test]
    async fn a_recording_finalizes_into_one_playable_stereo_file() {
        if skip_without_ffmpeg().await {
            return;
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let mut sink = CheckpointSink::new(dir.path(), "abcd1234").expect("sink");
        sink.write(&tone(500)).await.expect("write");
        let path = sink.finalize().await.expect("finalize").expect("a file");

        assert!(path.exists(), "the final file must exist");
        assert!(
            std::fs::metadata(&path).expect("metadata").len() > 0,
            "and must not be empty"
        );
        assert!(
            !dir.path().join(".checkpoints").join("abcd1234").exists(),
            "checkpoints are cleaned up once merged"
        );
    }

    #[tokio::test]
    async fn checkpoints_left_by_a_crash_are_recovered() {
        if skip_without_ffmpeg().await {
            return;
        }
        let dir = tempfile::tempdir().expect("tempdir");

        // A recording that never got to finalize: the sink is dropped
        // without finalize(), exactly as a kill -9 would leave it.
        {
            let mut sink = CheckpointSink::new(dir.path(), "deadbeef").expect("sink");
            sink.write(&tone(200)).await.expect("write");
            // Seal one checkpoint by hand so there is something to recover;
            // the in-flight one is the part a crash legitimately loses.
            if let Some(encoder) = sink.encoder.take() {
                encoder.finish().await.expect("seal");
            }
        }
        assert!(dir.path().join(".checkpoints/deadbeef").exists());

        let recoveries = recover_interrupted(dir.path()).await.expect("recover");
        assert_eq!(recoveries.len(), 1);
        match &recoveries[0] {
            Recovery::Recovered { path, chunks } => {
                assert!(path.exists(), "recovery must produce a playable file");
                assert_eq!(*chunks, 1);
            }
            other => panic!("expected a recovery, got {other:?}"),
        }
        assert!(
            !dir.path().join(".checkpoints/deadbeef").exists(),
            "recovered checkpoints are cleaned up"
        );
    }

    #[tokio::test]
    async fn recovery_is_a_no_op_when_nothing_was_interrupted() {
        let dir = tempfile::tempdir().expect("tempdir");
        let recoveries = recover_interrupted(dir.path()).await.expect("recover");
        assert!(recoveries.is_empty());
    }

    #[tokio::test]
    async fn a_missing_encoder_degrades_the_audio_not_the_meeting() {
        // The Meeting must keep going when ffmpeg cannot run: the record is
        // the transcript, and audio is the bonus (ADR-0019).
        let dir = tempfile::tempdir().expect("tempdir");
        temp_env_ffmpeg("definitely-not-a-real-binary-name");

        let mut sink = CheckpointSink::new(dir.path(), "abcd1234").expect("sink");
        sink.write(&tone(50)).await.expect("write must not fail");
        assert!(sink.is_disabled(), "the sink should disable itself");
        sink.write(&tone(50))
            .await
            .expect("further writes are no-ops");

        restore_env_ffmpeg();
    }

    // The env var is process-global; these two helpers keep the one test
    // that needs it from leaking into the others.
    fn temp_env_ffmpeg(value: &str) {
        unsafe { std::env::set_var(FFMPEG_ENV, value) };
    }

    fn restore_env_ffmpeg() {
        unsafe { std::env::remove_var(FFMPEG_ENV) };
    }
}
