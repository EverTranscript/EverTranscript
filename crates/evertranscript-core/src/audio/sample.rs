//! Cutting one voice out of a Meeting's kept audio.
//!
//! The Voice Registry plays a Speaker back from the recording their
//! Voiceprint was taken from. That has to be cheap enough to do on a click
//! against an hour-long file, and it is, because of two properties the sink
//! chose for other reasons (ADR-0032): the file is a bare stream of
//! constant-size frames, so a stretch of time is a byte range; and the two
//! channels are coded independently, so one leg comes out clean.
//!
//! So a cut reads a few kilobytes at an offset, decodes them, keeps the
//! channel asked for, and re-encodes it mono. No pass over the Meeting, no
//! index, no cache.

use std::io::Cursor;
use std::io::Read;
use std::io::Seek;
use std::io::SeekFrom;
use std::path::Path;

use anyhow::Context;
use anyhow::Result;
use evertranscript_protocol::AudioChannel;
use mp3lame_encoder::Bitrate;
use mp3lame_encoder::Builder;
use mp3lame_encoder::FlushNoGap;
use mp3lame_encoder::Mode;
use mp3lame_encoder::MonoPcm;
use mp3lame_encoder::Quality;
use mp3lame_encoder::max_required_buffer_size;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use super::SAMPLE_RATE;
use super::sink::BYTES_PER_SECOND;

/// Samples per MPEG-1 Layer III frame. A property of the format.
const FRAME_SAMPLES: u64 = 1_152;

/// Bytes per frame in the kept audio.
///
/// Derived from the sink's rate so the two cannot disagree: 128 kbps at
/// 48 kHz is exactly 384 bytes a frame, which is what makes the stream a
/// byte-addressable array of frames rather than merely a constant *average*
/// bitrate. A rate where this division carried a remainder would need
/// LAME's padding bit, and the arithmetic below would drift a byte a frame.
const FRAME_BYTES: u64 = BYTES_PER_SECOND * FRAME_SAMPLES / SAMPLE_RATE as u64;
const _: () = assert!(
    (BYTES_PER_SECOND * FRAME_SAMPLES).is_multiple_of(SAMPLE_RATE as u64),
    "frames must be a whole number of bytes for a time to be a byte offset"
);

/// Frames read before the one the clip starts in.
///
/// Layer III carries a bit reservoir: a frame's data can begin up to 511
/// bytes inside the frames before it, and the filterbank overlaps one frame
/// into the next. A decoder started cold on the exact frame produces a
/// damaged first few; these are decoded and discarded.
const WARM_UP_FRAMES: u64 = 8;

/// The clip's own bitrate. Mono speech; 64 kbps is what each leg of the
/// recording already gets.
const CLIP_BITRATE: Bitrate = Bitrate::Kbps64;

/// A playable clip.
pub struct Clip {
    pub bytes: Vec<u8>,
    pub mime_type: &'static str,
}

/// Cuts `[start_ms, end_ms)` of one channel out of a kept recording.
///
/// Works on the MP3 the sink writes. A Meeting from before ADR-0032's
/// reversal is AAC and is decoded whole instead — slow, and those Meetings
/// are few and old, so a second code path for them is not worth its bugs.
pub fn cut(path: &Path, channel: AudioChannel, start_ms: u64, end_ms: u64) -> Result<Clip> {
    let end_ms = end_ms.max(start_ms);
    let is_mp3 = path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("mp3"));

    let (samples, rate) = if is_mp3 {
        (decode_range(path, channel, start_ms, end_ms)?, SAMPLE_RATE)
    } else {
        let decoded = crate::diarize::runner::decode(path)?;
        let leg = match channel {
            AudioChannel::Mic => decoded.mic,
            AudioChannel::System => decoded.system,
        };
        let rate = crate::diarize::fbank::SAMPLE_RATE;
        let from = (start_ms * rate as u64 / 1000) as usize;
        let to = (end_ms * rate as u64 / 1000) as usize;
        (
            leg.get(from.min(leg.len())..to.min(leg.len()))
                .unwrap_or_default()
                .to_vec(),
            rate,
        )
    };
    anyhow::ensure!(!samples.is_empty(), "the clip lies outside the recording");

    Ok(Clip {
        bytes: encode_mono(&samples, rate)?,
        mime_type: "audio/mpeg",
    })
}

/// The channel's samples for the range, from just the frames that hold it.
fn decode_range(
    path: &Path,
    channel: AudioChannel,
    start_ms: u64,
    end_ms: u64,
) -> Result<Vec<f32>> {
    let rate = SAMPLE_RATE as u64;
    let first_sample = start_ms * rate / 1000;
    let last_sample = end_ms * rate / 1000;

    let first_frame = (first_sample / FRAME_SAMPLES).saturating_sub(WARM_UP_FRAMES);
    let end_frame = last_sample.div_ceil(FRAME_SAMPLES);

    let mut file =
        std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    file.seek(SeekFrom::Start(first_frame * FRAME_BYTES))?;
    let mut bytes = vec![0_u8; ((end_frame - first_frame) * FRAME_BYTES) as usize];
    let read = read_fully(&mut file, &mut bytes)?;
    bytes.truncate(read);

    // Every frame decodes to the same number of samples, so the packet's
    // own timestamp — counted by the demuxer whether or not the frame
    // decodes — says exactly where in the Meeting it lands. Counting decoded
    // samples instead would slip by a frame every time one is skipped, and
    // the first ones are skipped on purpose.
    let slice_origin = first_frame * FRAME_SAMPLES;
    let stream = MediaSourceStream::new(Box::new(Cursor::new(bytes)), Default::default());
    let mut hint = Hint::new();
    hint.with_extension("mp3");
    let probed = symphonia::default::get_probe().format(
        &hint,
        stream,
        &FormatOptions::default(),
        &MetadataOptions::default(),
    )?;
    let mut format = probed.format;
    let track = format
        .tracks()
        .first()
        .ok_or_else(|| anyhow::anyhow!("no audio track in {}", path.display()))?;
    let mut decoder =
        symphonia::default::get_codecs().make(&track.codec_params, &DecoderOptions::default())?;

    let mut samples = Vec::with_capacity((last_sample - first_sample) as usize);
    while let Ok(packet) = format.next_packet() {
        let at = slice_origin + packet.ts();
        let Ok(decoded) = decoder.decode(&packet) else {
            continue;
        };
        let spec = *decoded.spec();
        let mut buffer = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
        buffer.copy_interleaved_ref(decoded);
        let channels = spec.channels.count().max(1);
        // Left is mic, right is system: the sink's convention, and the one
        // the whole product depends on.
        let leg = match channel {
            AudioChannel::Mic => 0,
            AudioChannel::System => 1.min(channels - 1),
        };
        for (index, frame) in buffer.samples().chunks(channels).enumerate() {
            let position = at + index as u64;
            if position >= first_sample && position < last_sample {
                samples.push(frame[leg]);
            }
        }
    }
    Ok(samples)
}

fn read_fully(file: &mut std::fs::File, buffer: &mut [u8]) -> Result<usize> {
    let mut filled = 0;
    while filled < buffer.len() {
        let n = file.read(&mut buffer[filled..])?;
        if n == 0 {
            break;
        }
        filled += n;
    }
    Ok(filled)
}

/// Mono MP3, so the clip plays in anything the Registry is rendered by.
fn encode_mono(samples: &[f32], rate: u32) -> Result<Vec<u8>> {
    let mut builder = Builder::new().ok_or_else(|| anyhow::anyhow!("allocating the encoder"))?;
    builder
        .set_sample_rate(rate)
        .map_err(|error| anyhow::anyhow!("sample rate: {error:?}"))?;
    builder
        .set_num_channels(1)
        .map_err(|error| anyhow::anyhow!("channels: {error:?}"))?;
    builder
        .set_brate(CLIP_BITRATE)
        .map_err(|error| anyhow::anyhow!("bitrate: {error:?}"))?;
    builder
        .set_mode(Mode::Mono)
        .map_err(|error| anyhow::anyhow!("mode: {error:?}"))?;
    builder
        .set_quality(Quality::Good)
        .map_err(|error| anyhow::anyhow!("quality: {error:?}"))?;
    builder
        .set_to_write_vbr_tag(false)
        .map_err(|error| anyhow::anyhow!("vbr tag: {error:?}"))?;
    let mut encoder = builder
        .build()
        .map_err(|error| anyhow::anyhow!("building the encoder: {error:?}"))?;

    let mut out = Vec::with_capacity(max_required_buffer_size(samples.len()));
    encoder
        .encode_to_vec(MonoPcm(samples), &mut out)
        .map_err(|error| anyhow::anyhow!("encoding: {error:?}"))?;
    out.reserve(max_required_buffer_size(0));
    encoder
        .flush_to_vec::<FlushNoGap>(&mut out)
        .map_err(|error| anyhow::anyhow!("flushing: {error:?}"))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::CaptureOffset;
    use crate::audio::StereoBlock;
    use crate::audio::sink::AudioSink;

    /// A recording with a different tone on each channel, switching pitch
    /// on the mic at a known moment, so a cut can be checked for *which*
    /// channel and *which* seconds it came from.
    async fn recording(dir: &Path) -> std::path::PathBuf {
        let mut sink = AudioSink::new(dir, "clip1234").expect("sink");
        let frames_per_block = SAMPLE_RATE as usize / 10;
        for block in 0..300 {
            // 30 s. The mic is 220 Hz for the first ten seconds and 440 Hz
            // after; the system leg is 880 Hz throughout.
            let mic_hz = if block < 100 { 220.0 } else { 440.0 };
            let mut samples = Vec::with_capacity(frames_per_block * 2);
            for index in 0..frames_per_block {
                let t = (block * frames_per_block + index) as f32 / SAMPLE_RATE as f32;
                samples.push((t * mic_hz * std::f32::consts::TAU).sin() * 0.5);
                samples.push((t * 880.0 * std::f32::consts::TAU).sin() * 0.5);
            }
            sink.write(&StereoBlock {
                offset: CaptureOffset::ZERO,
                samples,
            })
            .await
            .expect("write");
        }
        sink.finalize().await.expect("finalize").expect("a file")
    }

    /// Dominant frequency by zero-crossing rate: enough to tell three tones
    /// an octave apart from each other.
    fn dominant_hz(samples: &[f32], rate: u32) -> f32 {
        let crossings = samples
            .windows(2)
            .filter(|pair| (pair[0] < 0.0) != (pair[1] < 0.0))
            .count();
        crossings as f32 / 2.0 / (samples.len() as f32 / rate as f32)
    }

    fn decode_clip(bytes: Vec<u8>) -> (Vec<f32>, u32) {
        let stream = MediaSourceStream::new(Box::new(Cursor::new(bytes)), Default::default());
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
        let track = format.tracks().first().expect("track").clone();
        let rate = track.codec_params.sample_rate.expect("rate");
        let mut decoder = symphonia::default::get_codecs()
            .make(&track.codec_params, &DecoderOptions::default())
            .expect("decoder");
        let mut out = Vec::new();
        while let Ok(packet) = format.next_packet() {
            let Ok(decoded) = decoder.decode(&packet) else {
                continue;
            };
            let spec = *decoded.spec();
            let mut buffer = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
            buffer.copy_interleaved_ref(decoded);
            out.extend(buffer.samples().iter().step_by(spec.channels.count()));
        }
        (out, rate)
    }

    #[tokio::test]
    async fn a_cut_comes_from_the_channel_and_the_seconds_asked_for() {
        // The whole feature: the Registry says "this is Alice", and the clip
        // has to actually be Alice — her channel, her moment — cut by
        // arithmetic from the middle of a file nothing decoded in full.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = recording(dir.path()).await;

        let early = cut(&path, AudioChannel::Mic, 2_000, 6_000).expect("cut");
        let (samples, rate) = decode_clip(early.bytes);
        let settled = &samples[rate as usize / 10..];
        let hz = dominant_hz(settled, rate);
        assert!(
            (hz - 220.0).abs() < 20.0,
            "seconds 2–6 of the mic are 220 Hz, got {hz}"
        );
        let seconds = samples.len() as f32 / rate as f32;
        assert!(
            (seconds - 4.0).abs() < 0.1,
            "four seconds were asked for, got {seconds}"
        );

        let late = cut(&path, AudioChannel::Mic, 15_000, 19_000).expect("cut");
        let (samples, rate) = decode_clip(late.bytes);
        let hz = dominant_hz(&samples[rate as usize / 10..], rate);
        assert!(
            (hz - 440.0).abs() < 20.0,
            "after ten seconds the mic is 440 Hz, got {hz}"
        );

        let far_end = cut(&path, AudioChannel::System, 15_000, 19_000).expect("cut");
        let (samples, rate) = decode_clip(far_end.bytes);
        let hz = dominant_hz(&samples[rate as usize / 10..], rate);
        assert!(
            (hz - 880.0).abs() < 40.0,
            "the system leg is 880 Hz throughout, got {hz}"
        );
        assert_eq!(far_end.mime_type, "audio/mpeg");
    }

    #[tokio::test]
    async fn a_cut_past_the_end_of_the_recording_is_an_error_not_silence() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = recording(dir.path()).await;
        assert!(cut(&path, AudioChannel::Mic, 60_000, 70_000).is_err());
        assert!(cut(Path::new("/nonexistent/x.mp3"), AudioChannel::Mic, 0, 1_000).is_err());
    }

    #[test]
    fn the_frame_arithmetic_matches_the_sinks_rate() {
        // 384 bytes and 24 ms a frame at the sink's settings. If BITRATE or
        // SAMPLE_RATE ever changes, this and the const assertion above are
        // where the cut learns about it.
        assert_eq!(FRAME_BYTES, 384);
        assert_eq!(FRAME_SAMPLES * 1000 / SAMPLE_RATE as u64, 24);
    }
}
