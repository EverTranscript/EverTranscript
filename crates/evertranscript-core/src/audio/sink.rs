//! Writing a Meeting's audio to disk (ADR-0032).
//!
//! One MP3 per Meeting, encoded in this process by LAME as capture runs.
//! There is no encoder subprocess, no intermediate file, and no post-meeting
//! encode step: the joiner's interleaved `f32` goes straight into the
//! encoder and what lands on disk is the finished recording.
//!
//! **Crash safety is a property of the format rather than machinery.** MP3 is
//! a frame stream, so a Core killed mid-Meeting leaves a file that plays up
//! to its last complete frame. The chunk-and-merge design this module used to
//! carry existed only because MP4 needs a moov atom it never gets to write;
//! with that gone, the recovery pass, the checkpoint directory and the
//! lossless-concat step go with it, and the worst a kill costs is one frame
//! instead of thirty seconds.
//!
//! Output is stereo — **left = mic, right = system** — encoded in LAME's
//! *dual channel* mode, which codes the two channels independently. Joint
//! stereo is built for correlated channels and these two are deliberately
//! uncorrelated sources, so coupling them is the case mid/side is worst at.
//! At this bitrate it would probably survive either way; dual channel makes
//! the separation a property of the encoding rather than of how the bits
//! happened to be allocated, which is what ADR-0029 needs — that Enhance-era
//! re-transcription and re-diarization get cleanly separated per-channel
//! sources forever.

use std::io::Write;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Context;
use anyhow::Result;
use mp3lame_encoder::Bitrate;
use mp3lame_encoder::Builder;
use mp3lame_encoder::Encoder;
use mp3lame_encoder::FlushNoGap;
use mp3lame_encoder::InterleavedPcm;
use mp3lame_encoder::max_required_buffer_size;
use mp3lame_encoder::Mode;
use mp3lame_encoder::Quality;
use tracing::info;
use tracing::warn;

use super::SAMPLE_RATE;
use super::StereoBlock;

/// The bitrate every Meeting is encoded at.
///
/// Split across two independently coded channels, so this is 64 kbps each —
/// comfortable for speech, and 58 MB/hr against the 86 MB/hr the AAC-192k
/// this replaced actually cost (measured: a 69.4-hour recording occupying
/// 6.0 GB). ADR-0032 leaves this open pending a listening check against
/// Transcription and Diarization quality; it is the one lever over both disk
/// and fidelity, which is why it is a named constant rather than a literal.
pub const BITRATE: Bitrate = Bitrate::Kbps128;

/// Bytes one second of output occupies. Constant because the encoder is CBR,
/// which is what lets a file's duration be read from its length without
/// decoding it.
pub const BYTES_PER_SECOND: u64 = 128_000 / 8;

/// What a Meeting's audio file is called under the audio directory.
pub fn audio_path(audio_dir: &Path, meeting_key: &str) -> PathBuf {
    audio_dir.join(format!("{meeting_key}.mp3"))
}

/// Seconds of audio a file of this size holds.
///
/// Arithmetic rather than a decode pass, because the one caller is restart
/// reconciliation and it runs before the Core serves anything.
pub fn seconds_from_bytes(bytes: u64) -> f64 {
    bytes as f64 / BYTES_PER_SECOND as f64
}

/// Audio a previous Core left behind for this Meeting, with its size.
///
/// A kill leaves a playable file rather than a directory of fragments, so
/// "recovery" is now a question about one path.
pub fn orphaned_audio(audio_dir: &Path, meeting_key: &str) -> Option<(PathBuf, u64)> {
    let path = audio_path(audio_dir, meeting_key);
    let bytes = std::fs::metadata(&path).ok()?.len();
    (bytes > 0).then_some((path, bytes))
}

/// Writes one Meeting's audio.
pub struct AudioSink {
    path: PathBuf,
    out: Option<Box<dyn Write + Send>>,
    encoder: Option<Encoder>,
    total_samples: usize,
    /// Why audio was abandoned, when it was. The reason and the fact are one
    /// field because they were two, and the one that travelled to the record
    /// was the fact — so a Meeting whose encoder never started looked exactly
    /// like a Meeting nobody spoke in.
    disabled: Option<String>,
    /// Reused across blocks so a recording does not allocate a fresh output
    /// buffer forty times a second.
    encoded: Vec<u8>,
}

impl AudioSink {
    /// Creates a sink for `meeting_key` (the id8) under `audio_dir`.
    pub fn new(audio_dir: &Path, meeting_key: &str) -> Result<Self> {
        std::fs::create_dir_all(audio_dir)
            .with_context(|| format!("creating {}", audio_dir.display()))?;
        let path = audio_path(audio_dir, meeting_key);
        let file = std::fs::File::create(&path)
            .with_context(|| format!("creating {}", path.display()))?;
        Ok(Self::with_writer(
            path,
            Box::new(std::io::BufWriter::new(file)),
        ))
    }

    /// A sink writing somewhere other than a file. Tests use this to stand up
    /// a recording whose output fails part-way, which is the failure that
    /// matters now that the encoder is in-process and cannot be missing.
    pub fn with_writer(path: PathBuf, out: Box<dyn Write + Send>) -> Self {
        Self {
            path,
            out: Some(out),
            encoder: None,
            total_samples: 0,
            disabled: None,
            encoded: Vec::new(),
        }
    }

    pub fn final_path(&self) -> &Path {
        &self.path
    }

    /// True once encoding has failed and audio is being dropped. The Meeting
    /// keeps recording — losing the audio bonus must not lose the transcript.
    pub fn is_disabled(&self) -> bool {
        self.disabled.is_some()
    }

    /// Why audio was abandoned, for the recorder to put into the record.
    /// Read before `finalize`, which consumes the sink.
    pub fn disabled_reason(&self) -> Option<&str> {
        self.disabled.as_deref()
    }

    pub fn seconds_written(&self) -> f64 {
        self.total_samples as f64 / (SAMPLE_RATE as f64 * 2.0)
    }

    /// Encodes and appends a stereo block.
    pub async fn write(&mut self, block: &StereoBlock) -> Result<()> {
        if self.is_disabled() || block.samples.is_empty() {
            return Ok(());
        }
        if let Err(error) = self.write_inner(block) {
            // An encoder that will not run is a degraded recording, not a
            // failed one (ADR-0019: the record is the transcript). But it is
            // only degraded rather than indistinguishable from silence if the
            // reason survives this function, so it is kept rather than logged
            // and dropped: a `warn!` from the Core reaches no log file and no
            // window, because the Core's stderr belongs to whoever spawned it.
            warn!(%error, "audio encoding failed; continuing without audio for this Meeting");
            self.disabled = Some(format!("audio encoder: {error:#}"));
            self.encoder = None;
            self.out = None;
        }
        Ok(())
    }

    fn write_inner(&mut self, block: &StereoBlock) -> Result<()> {
        if self.encoder.is_none() {
            self.encoder = Some(build_encoder()?);
        }
        let encoder = self.encoder.as_mut().expect("just created");
        let out = self
            .out
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("the output is gone"))?;

        // The encoder writes into the vector's *spare capacity* and only then
        // sets its length, so the reserve is not an optimisation: without it
        // LAME writes past the allocation and the process dies with SIGSEGV,
        // which is exactly how this was found.
        self.encoded.clear();
        self.encoded
            .reserve(max_required_buffer_size(block.samples.len()));
        encoder
            .encode_to_vec(InterleavedPcm(block.samples.as_slice()), &mut self.encoded)
            .map_err(|error| anyhow::anyhow!("encoding: {error:?}"))?;
        out.write_all(&self.encoded).context("writing audio")?;
        self.total_samples += block.samples.len();
        Ok(())
    }

    /// Flushes the encoder's tail and closes the file.
    ///
    /// Returns `None` when there is no recording to point at — either nothing
    /// was captured, or encoding was abandoned before a byte reached disk. A
    /// Meeting with no audio is degraded, and the reason travels separately
    /// through `disabled_reason`.
    pub async fn finalize(mut self) -> Result<Option<PathBuf>> {
        let wrote_anything = self.total_samples > 0;
        let flushed = self.flush_tail();
        // The file is closed either way: dropping the writer is what commits
        // what did reach disk, and a partial recording is worth keeping.
        drop(self.out.take());

        if !wrote_anything {
            // An empty file is worse than none: it points the record at
            // something that will not play.
            let _ = std::fs::remove_file(&self.path);
            return flushed.map(|()| None);
        }
        flushed?;
        info!(path = %self.path.display(), seconds = self.seconds_written(), "audio finalized");
        Ok(Some(self.path))
    }

    fn flush_tail(&mut self) -> Result<()> {
        let (Some(encoder), Some(out)) = (self.encoder.as_mut(), self.out.as_mut()) else {
            return Ok(());
        };
        // Same contract as `encode_to_vec`, and the tail LAME still holds is
        // bounded by one frame's worth plus its own padding.
        self.encoded.clear();
        self.encoded.reserve(max_required_buffer_size(0));
        encoder
            .flush_to_vec::<FlushNoGap>(&mut self.encoded)
            .map_err(|error| anyhow::anyhow!("flushing the encoder: {error:?}"))?;
        out.write_all(&self.encoded).context("writing audio")?;
        out.flush().context("flushing audio")?;
        Ok(())
    }
}

/// LAME, configured once and the same way for every Meeting.
fn build_encoder() -> Result<Encoder> {
    let mut builder = Builder::new().ok_or_else(|| anyhow::anyhow!("allocating the encoder"))?;
    builder
        .set_sample_rate(SAMPLE_RATE)
        .map_err(|error| anyhow::anyhow!("sample rate: {error:?}"))?;
    builder
        .set_num_channels(2)
        .map_err(|error| anyhow::anyhow!("channels: {error:?}"))?;
    builder
        .set_brate(BITRATE)
        .map_err(|error| anyhow::anyhow!("bitrate: {error:?}"))?;
    // Independent channels, not joint stereo — chosen so the separation is a
    // property of the encoding rather than of the bit allocation. Joint
    // stereo would very likely be fine at this bitrate (measured: mid/side
    // reconstructs a hard-panned signal cleanly at 128 kbps), but it is fine
    // *because* there are bits to spare, and the two legs here are
    // uncorrelated by construction — the case M/S is worst at. Dual channel
    // costs nothing and removes the dependency on that judgement.
    builder
        .set_mode(Mode::DaulChannel)
        .map_err(|error| anyhow::anyhow!("stereo mode: {error:?}"))?;
    builder
        .set_quality(Quality::Good)
        .map_err(|error| anyhow::anyhow!("quality: {error:?}"))?;
    // No VBR tag. It would have to be written at the *front* of the file
    // after encoding finishes, which means seeking back into a file a crash
    // may have already truncated — trading the crash safety this format was
    // chosen for against a duration hint nothing here reads.
    builder
        .set_to_write_vbr_tag(false)
        .map_err(|error| anyhow::anyhow!("vbr tag: {error:?}"))?;
    builder
        .build()
        .map_err(|error| anyhow::anyhow!("building the encoder: {error:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::CaptureOffset;

    /// One stereo block whose two channels carry different, constant levels.
    fn block(mic: f32, system: f32, frames: usize) -> StereoBlock {
        let mut samples = Vec::with_capacity(frames * 2);
        for _ in 0..frames {
            samples.push(mic);
            samples.push(system);
        }
        StereoBlock {
            offset: CaptureOffset::ZERO,
            samples,
        }
    }

    /// A tone on one channel, silence on the other.
    fn split_tone(frames: usize, mic_amplitude: f32) -> StereoBlock {
        let mut samples = Vec::with_capacity(frames * 2);
        for index in 0..frames {
            let phase = index as f32 / SAMPLE_RATE as f32 * 440.0 * std::f32::consts::TAU;
            samples.push(phase.sin() * mic_amplitude);
            samples.push(0.0);
        }
        StereoBlock {
            offset: CaptureOffset::ZERO,
            samples,
        }
    }

    /// Decodes a finished file back to its two channels, the way Diarization
    /// does (`diarize::runner`).
    fn decode(path: &Path) -> (Vec<f32>, Vec<f32>) {
        use symphonia::core::audio::SampleBuffer;
        use symphonia::core::codecs::DecoderOptions;
        use symphonia::core::formats::FormatOptions;
        use symphonia::core::io::MediaSourceStream;
        use symphonia::core::meta::MetadataOptions;
        use symphonia::core::probe::Hint;

        let file = std::fs::File::open(path).expect("open");
        let stream = MediaSourceStream::new(Box::new(file), Default::default());
        let mut hint = Hint::new();
        hint.with_extension("mp3");
        let probed = symphonia::default::get_probe()
            .format(
                &hint,
                stream,
                &FormatOptions::default(),
                &MetadataOptions::default(),
            )
            .expect("probe");
        let mut format = probed.format;
        let track = format.tracks().first().expect("a track").clone();
        let mut decoder = symphonia::default::get_codecs()
            .make(&track.codec_params, &DecoderOptions::default())
            .expect("decoder");

        let (mut left, mut right) = (Vec::new(), Vec::new());
        while let Ok(packet) = format.next_packet() {
            let Ok(decoded) = decoder.decode(&packet) else {
                continue;
            };
            let spec = *decoded.spec();
            let mut buffer = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
            buffer.copy_interleaved_ref(decoded);
            let channels = spec.channels.count();
            for frame in buffer.samples().chunks(channels) {
                left.push(frame[0]);
                right.push(if channels > 1 { frame[1] } else { frame[0] });
            }
        }
        (left, right)
    }

    fn peak(samples: &[f32]) -> f32 {
        samples.iter().fold(0.0f32, |max, s| max.max(s.abs()))
    }

    #[tokio::test]
    async fn a_recording_finalizes_into_one_playable_stereo_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut sink = AudioSink::new(dir.path(), "abcd1234").expect("sink");
        for _ in 0..20 {
            sink.write(&block(0.3, -0.3, SAMPLE_RATE as usize / 10))
                .await
                .expect("write");
        }
        assert!(sink.seconds_written() >= 1.9, "two seconds were written");

        let path = sink.finalize().await.expect("finalize").expect("a file");
        assert_eq!(path.extension().and_then(|e| e.to_str()), Some("mp3"));
        let (left, right) = decode(&path);
        assert!(!left.is_empty(), "the file must decode");
        assert!(
            peak(&left) > 0.1 && peak(&right) > 0.1,
            "both channels must carry audio, got {} and {}",
            peak(&left),
            peak(&right)
        );
    }

    #[tokio::test]
    async fn the_two_channels_stay_separable_through_the_encoder() {
        // ADR-0029's split has to survive storage, so Enhance-era work gets
        // cleanly separated per-channel sources forever. A tone on the
        // microphone must not appear on the system side.
        //
        // What this does and does not prove, measured: it fails with
        // `Mode::Mono` (both legs come back at an identical peak, which is
        // the smearing it exists to catch), so it guards against a downmix or
        // a joiner that duplicates a channel. It **passes** with
        // `Mode::JointStereo` too — at 128 kbps LAME uses mid/side, and a
        // hard-panned signal reconstructs cleanly through it
        // (`M = L/2, S = L/2` gives back `L` and silence). So this is a
        // regression guard on the split, not a demonstration that the mode
        // choice is load-bearing at this bitrate.
        let dir = tempfile::tempdir().expect("tempdir");
        let mut sink = AudioSink::new(dir.path(), "sepa1234").expect("sink");
        for _ in 0..40 {
            sink.write(&split_tone(SAMPLE_RATE as usize / 10, 0.7))
                .await
                .expect("write");
        }
        let path = sink.finalize().await.expect("finalize").expect("a file");

        let (mic, system) = decode(&path);
        // Skip the encoder's warm-up, where its padding lives.
        let settled = SAMPLE_RATE as usize;
        assert!(mic.len() > settled * 2, "enough audio to judge");
        let (mic_peak, system_peak) = (peak(&mic[settled..]), peak(&system[settled..]));
        assert!(mic_peak > 0.3, "the tone must survive, got {mic_peak}");
        assert!(
            system_peak < mic_peak / 10.0,
            "the silent leg must stay silent — {system_peak} against {mic_peak} is the \
             channel bleed dual-channel mode exists to prevent"
        );
    }

    #[tokio::test]
    async fn a_recording_nobody_spoke_in_leaves_no_file_to_point_at() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sink = AudioSink::new(dir.path(), "empty123").expect("sink");
        let path = sink.final_path().to_path_buf();
        assert!(sink.finalize().await.expect("finalize").is_none());
        assert!(
            !path.exists(),
            "an empty file would point the record at something that will not play"
        );
    }

    #[tokio::test]
    async fn an_output_that_refuses_degrades_the_audio_not_the_meeting() {
        struct Refusing;
        impl Write for Refusing {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("the disk said no"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let dir = tempfile::tempdir().expect("tempdir");
        let mut sink =
            AudioSink::with_writer(dir.path().join("nope.mp3"), Box::new(Refusing));
        sink.write(&block(0.3, -0.3, 4800))
            .await
            .expect("a refused write must not fail the Meeting");
        assert!(sink.is_disabled(), "the sink should disable itself");
        // And it must say why. Disabling itself quietly is what made a
        // Meeting whose audio failed indistinguishable from a Meeting nobody
        // spoke in: no samples are kept, so every later check that counts
        // bytes sees a clean, empty recording.
        let reason = sink.disabled_reason().expect("the reason must survive");
        assert!(
            reason.contains("the disk said no"),
            "the reason must name what went wrong, got {reason:?}"
        );
        sink.write(&block(0.3, -0.3, 4800))
            .await
            .expect("further writes are no-ops");
    }

    #[test]
    fn a_killed_core_leaves_audio_the_next_one_can_find() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(orphaned_audio(dir.path(), "abcd1234").is_none());

        std::fs::write(audio_path(dir.path(), "abcd1234"), b"frames").expect("write");
        let (path, bytes) = orphaned_audio(dir.path(), "abcd1234").expect("found");
        assert_eq!(bytes, 6);
        assert!(path.ends_with("abcd1234.mp3"));

        // An empty file is not a recording, and attaching one would point the
        // record at something that will not play.
        std::fs::write(audio_path(dir.path(), "empty123"), b"").expect("write");
        assert!(orphaned_audio(dir.path(), "empty123").is_none());
    }

    #[test]
    fn a_files_length_is_its_duration() {
        // CBR, which is what lets restart reconciliation date an interrupted
        // Meeting without decoding hours of audio at startup.
        assert_eq!(seconds_from_bytes(BYTES_PER_SECOND * 60), 60.0);
        assert_eq!(seconds_from_bytes(0), 0.0);
    }
}
