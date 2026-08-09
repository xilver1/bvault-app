//! Audio decode: container/codec -> mono f32 PCM, plus the intrinsic audio
//! properties and embedded tag metadata rekordbox stores. Pure front-end — no
//! filesystem walking, no cache, no playlist knowledge.

use symphonia::core::audio::{AudioBufferRef, Signal};
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::formats::{FormatOptions, FormatReader};
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::{MetadataOptions, StandardTagKey};
use symphonia::core::probe::Hint;
use tracing::debug;

use rekordbox_core::FileType;

use crate::{Error, Result};

/// Decoded audio plus everything derivable directly from the file itself.
pub struct DecodedAudio {
    /// Mono, f32-normalised samples, capped at `max_samples`.
    pub samples: Vec<f32>,
    /// True decoded length in seconds, measured over the *full* stream even when
    /// the sample buffer above was capped.
    pub duration_secs: f64,
    pub sample_rate: u32,
    pub bit_depth: u16,
    pub bitrate: u32,
    pub file_type: FileType,
    pub metadata: Metadata,
}

/// Tag metadata read from the stream. Every field is optional: absence is the
/// caller's problem to resolve (fall back to an ingestion-supplied name), not
/// something the decoder invents.
#[derive(Debug, Default, Clone)]
pub struct Metadata {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub genre: Option<String>,
    pub year: Option<u16>,
    pub track_number: Option<u32>,
}

/// Decode `source` to mono f32, buffering at most `max_samples` for analysis.
/// The whole stream is still walked so `duration_secs` is accurate.
///
/// `hint_ext` is the extension without the dot; it helps Symphonia probe and
/// sets the `FileType` rekordbox records.
pub fn decode(
    source: Box<dyn MediaSource>,
    hint_ext: Option<&str>,
    max_samples: usize,
) -> Result<DecodedAudio> {
    let mss = MediaSourceStream::new(source, Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = hint_ext {
        hint.with_extension(ext);
    }

    let probed = symphonia::default::get_probe().format(
        &hint,
        mss,
        &FormatOptions::default(),
        &MetadataOptions::default(),
    )?;
    let mut format = probed.format;

    // Pull track params out before the mutable borrows of decoding begin.
    let (codec_track_id, sample_rate, bit_depth, bitrate, codec_params) = {
        let track = format.default_track().ok_or(Error::NoDefaultTrack)?;
        let sample_rate = track
            .codec_params
            .sample_rate
            .ok_or(Error::UnknownSampleRate)?;
        let bit_depth = track.codec_params.bits_per_sample.unwrap_or(16) as u16;
        // kbps: from coded sample size when known, else estimate for lossless.
        let bitrate = track
            .codec_params
            .bits_per_coded_sample
            .map(|bps| (bps * sample_rate / 1000) as u32)
            .or_else(|| match bit_depth {
                16 => Some(sample_rate * 16 * 2 / 1000), // stereo 16-bit
                24 => Some(sample_rate * 24 * 2 / 1000), // stereo 24-bit
                _ => None,
            })
            .unwrap_or(320);
        (
            track.id,
            sample_rate,
            bit_depth,
            bitrate,
            track.codec_params.clone(),
        )
    };

    let mut decoder =
        symphonia::default::get_codecs().make(&codec_params, &DecoderOptions::default())?;

    let file_type = hint_ext.map(FileType::from_extension).unwrap_or_default();
    let metadata = read_metadata(&mut format);

    let mut samples: Vec<f32> = Vec::new();
    let mut total_samples = 0u64;

    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            // Clean end-of-stream: Symphonia surfaces it as an unexpected EOF.
            Err(symphonia::core::errors::Error::IoError(ref e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break
            }
            Err(e) => return Err(e.into()),
        };
        if packet.track_id() != codec_track_id {
            continue;
        }
        let decoded = decoder.decode(&packet)?;
        total_samples += decoded.frames() as u64;
        if samples.len() < max_samples {
            append_mono_f32(&decoded, &mut samples);
        }
    }

    let duration_secs = total_samples as f64 / sample_rate as f64;
    debug!(total_samples, duration_secs, "decoded audio");

    Ok(DecodedAudio {
        samples,
        duration_secs,
        sample_rate,
        bit_depth,
        bitrate,
        file_type,
        metadata,
    })
}

/// Read standard tags. Only the fields rekordbox needs are extracted.
fn read_metadata(format: &mut Box<dyn FormatReader>) -> Metadata {
    let mut m = Metadata::default();
    if let Some(rev) = format.metadata().current() {
        for tag in rev.tags() {
            match tag.std_key {
                Some(StandardTagKey::TrackTitle) => m.title = Some(tag.value.to_string()),
                Some(StandardTagKey::Artist) => m.artist = Some(tag.value.to_string()),
                Some(StandardTagKey::Album) => m.album = Some(tag.value.to_string()),
                Some(StandardTagKey::Genre) => m.genre = Some(tag.value.to_string()),
                Some(StandardTagKey::Date) => {
                    m.year = tag.value.to_string().get(..4).and_then(|s| s.parse().ok());
                }
                Some(StandardTagKey::TrackNumber) => {
                    m.track_number = tag.value.to_string().parse().ok();
                }
                _ => {}
            }
        }
    }
    m
}

/// Downmix a decoded buffer to mono f32 and append it. Formats other than the
/// three common PCM widths are skipped (rare for DJ libraries).
fn append_mono_f32(buffer: &AudioBufferRef, out: &mut Vec<f32>) {
    match buffer {
        AudioBufferRef::F32(buf) => {
            let ch = buf.spec().channels.count();
            for frame in 0..buf.frames() {
                let sum: f32 = (0..ch).map(|c| buf.chan(c)[frame]).sum();
                out.push(sum / ch as f32);
            }
        }
        AudioBufferRef::S16(buf) => {
            let ch = buf.spec().channels.count();
            for frame in 0..buf.frames() {
                let sum: f32 = (0..ch).map(|c| buf.chan(c)[frame] as f32 / 32768.0).sum();
                out.push(sum / ch as f32);
            }
        }
        AudioBufferRef::S32(buf) => {
            let ch = buf.spec().channels.count();
            for frame in 0..buf.frames() {
                let sum: f32 = (0..ch)
                    .map(|c| buf.chan(c)[frame] as f32 / 2_147_483_648.0)
                    .sum();
                out.push(sum / ch as f32);
            }
        }
        _ => debug!("unsupported sample format, skipping packet"),
    }
}