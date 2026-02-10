use crate::db::{TrackMeta, TrackPeaks};
use anyhow::{Context, Result};

pub struct AnalyzedTrack {
    pub meta: TrackMeta,
    pub peaks: TrackPeaks,
    pub cover_art: Option<Vec<u8>>,
}
use std::path::{Path, PathBuf};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::{MetadataOptions, MetadataRevision, StandardTagKey};
use symphonia::core::probe::Hint;

/// Peak resolution: one peak value per this many mono frames.
const PEAK_CHUNK_SIZE: usize = 1024;

fn extract_metadata(
    rev: &MetadataRevision,
    artist: &mut String,
    title: &mut String,
    cover_art: &mut Option<Vec<u8>>,
) {
    for tag in rev.tags() {
        match tag.std_key {
            Some(StandardTagKey::Artist) | Some(StandardTagKey::AlbumArtist) => {
                if artist.is_empty() {
                    *artist = tag.value.to_string();
                }
            }
            Some(StandardTagKey::TrackTitle) => {
                if title.is_empty() {
                    *title = tag.value.to_string();
                }
            }
            _ => {}
        }
    }

    if cover_art.is_none() {
        if let Some(visual) = rev.visuals().first() {
            *cover_art = Some(visual.data.to_vec());
        }
    }
}

/// Recursively collect audio files from a directory.
pub fn collect_audio_files(folder: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();

    for entry in std::fs::read_dir(folder)? {
        let entry = entry?;
        let path = entry.path();

        if path.is_dir() {
            paths.extend(collect_audio_files(&path)?);
        }

        if let Some(ext) = path.extension() {
            let ext_str = ext.to_string_lossy().to_lowercase();
            if matches!(ext_str.as_str(), "mp3" | "flac" | "wav" | "ogg" | "m4a" | "aac") {
                paths.push(path);
            }
        }
    }

    paths.sort();
    Ok(paths)
}

pub fn analyze_track(path: &Path) -> Result<AnalyzedTrack> {
    let mtime_secs = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let file =
        std::fs::File::open(path).with_context(|| format!("Failed to open {}", path.display()))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .context("Failed to probe audio format")?;

    let mut format = probed.format;
    let mut probed_metadata = probed.metadata;

    // Extract metadata
    let mut artist = String::new();
    let mut title = String::new();
    let mut cover_art = None;

    // Container-level metadata (e.g. ID3v2 in MP3)
    if let Some(md) = probed_metadata.get() {
        if let Some(rev) = md.current() {
            extract_metadata(rev, &mut artist, &mut title, &mut cover_art);
        }
    }

    // Format-level metadata
    {
        let md = format.metadata();
        if let Some(rev) = md.current() {
            extract_metadata(rev, &mut artist, &mut title, &mut cover_art);
        }
    }

    // Fallback title from filename
    if title.is_empty() {
        title = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Unknown")
            .to_string();
    }
    if artist.is_empty() {
        artist = "Unknown Artist".to_string();
    }

    // Find first audio track
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != symphonia::core::codecs::CODEC_TYPE_NULL)
        .context("No audio track found")?;

    let track_id = track.id;
    let sample_rate = track.codec_params.sample_rate.unwrap_or(44100);
    let channels = track
        .codec_params
        .channels
        .map(|c| c.count() as u16)
        .unwrap_or(2);

    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .context("Failed to create audio decoder")?;

    // Stream through packets computing RMS levels without storing all samples.
    // RMS (root mean square) better represents perceived loudness than peak values,
    // producing the varied, dynamic waveform shape SoundCloud uses.
    let mut peaks: Vec<f32> = Vec::new();
    let mut peaks_max: Vec<f32> = Vec::new();
    let mut chunk_sum_sq: f64 = 0.0;
    let mut chunk_max: f32 = 0.0;
    let mut chunk_count: usize = 0;
    let mut total_mono_frames: u64 = 0;
    let mut sample_buf: Option<SampleBuffer<f32>> = None;
    let ch = channels.max(1) as usize;

    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(symphonia::core::errors::Error::IoError(ref e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(_) => break,
        };

        if packet.track_id() != track_id {
            continue;
        }

        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(_) => continue,
        };

        if sample_buf.is_none() {
            let spec = *decoded.spec();
            let capacity = decoded.capacity() as u64;
            sample_buf = Some(SampleBuffer::<f32>::new(capacity, spec));
        }

        if let Some(ref mut buf) = sample_buf {
            buf.copy_interleaved_ref(decoded);
            let samples = buf.samples();

            // Process interleaved samples into mono RMS and peak-max values
            for frame in samples.chunks(ch) {
                let mono = frame.iter().sum::<f32>() / ch as f32;
                let abs_mono = mono.abs();
                chunk_sum_sq += (mono as f64) * (mono as f64);
                if abs_mono > chunk_max {
                    chunk_max = abs_mono;
                }
                chunk_count += 1;
                total_mono_frames += 1;

                if chunk_count >= PEAK_CHUNK_SIZE {
                    let rms = (chunk_sum_sq / chunk_count as f64).sqrt() as f32;
                    peaks.push(rms);
                    peaks_max.push(chunk_max);
                    chunk_sum_sq = 0.0;
                    chunk_max = 0.0;
                    chunk_count = 0;
                }
            }
        }
    }

    // Flush remaining chunk
    if chunk_count > 0 {
        let rms = (chunk_sum_sq / chunk_count as f64).sqrt() as f32;
        peaks.push(rms);
        peaks_max.push(chunk_max);
    }

    let total_duration_secs = total_mono_frames as f64 / sample_rate as f64;

    Ok(AnalyzedTrack {
        meta: TrackMeta {
            path: path.to_path_buf(),
            artist,
            title,
            sample_rate,
            channels,
            total_duration_secs,
            mtime_secs,
        },
        peaks: TrackPeaks { peaks, peaks_max },
        cover_art,
    })
}
