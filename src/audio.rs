use crate::db::{TrackMeta, TrackPeaks};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::{MetadataOptions, MetadataRevision, StandardTagKey};
use symphonia::core::probe::Hint;

/// Number of waveform bins stored per track, regardless of length.
/// Matches SoundCloud's fixed-resolution waveform; the display never draws
/// more bars than this, so a fixed count bounds the cache and keeps a short
/// clip and a long mix at the same on-disk size.
const TARGET_BINS: usize = 1800;

/// Fine accumulation resolution: the streaming decode collects one value per
/// this many mono frames, then resamples down to TARGET_BINS. Kept small so
/// even short tracks yield plenty of detail to resample from.
const FINE_CHUNK_SIZE: usize = 1024;

/// Cover art is displayed in a 140px logical square (see ui/app.slint), so the
/// stored thumbnail is sized for ~2x HiDPI of that. Embedded art is downscaled
/// to fit this box before caching, rather than stored at full resolution.
const COVER_MAX_DIM: u32 = 320;

/// Decode embedded cover art and re-encode it as a downscaled JPEG thumbnail.
/// Falls back to the original bytes if the image cannot be decoded/encoded.
fn downscale_cover(bytes: &[u8]) -> Vec<u8> {
    let Ok(img) = image::load_from_memory(bytes) else {
        return bytes.to_vec();
    };
    // thumbnail() preserves aspect ratio, fitting within the box.
    let thumb = image::DynamicImage::ImageRgb8(img.thumbnail(COVER_MAX_DIM, COVER_MAX_DIM).to_rgb8());
    let mut out = std::io::Cursor::new(Vec::new());
    match thumb.write_to(&mut out, image::ImageFormat::Jpeg) {
        Ok(()) => out.into_inner(),
        Err(_) => bytes.to_vec(),
    }
}

/// Resample fine-grained envelopes down to exactly `target` bins.
/// RMS bins are combined as a quadratic mean (correct RMS-of-RMS, since each
/// fine bin spans the same frame count); peak-max bins are combined as a max.
/// If there are already `target` or fewer fine bins, they are returned as-is.
fn resample_envelopes(fine_rms: &[f32], fine_max: &[f32], target: usize) -> (Vec<f32>, Vec<f32>) {
    let n = fine_rms.len();
    if n <= target || target == 0 {
        return (fine_rms.to_vec(), fine_max.to_vec());
    }

    let mut rms = Vec::with_capacity(target);
    let mut max = Vec::with_capacity(target);
    for i in 0..target {
        let start = i * n / target;
        let end = ((i + 1) * n / target).max(start + 1).min(n);
        let group = &fine_rms[start..end];
        let mean_sq = group.iter().map(|&x| (x as f64) * (x as f64)).sum::<f64>() / group.len() as f64;
        rms.push(mean_sq.sqrt() as f32);
        max.push(fine_max[start..end].iter().cloned().fold(0.0f32, f32::max));
    }
    (rms, max)
}

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

/// Open a file and probe its container format.
fn open_probe(path: &Path) -> Result<symphonia::core::probe::ProbeResult> {
    let file =
        std::fs::File::open(path).with_context(|| format!("Failed to open {}", path.display()))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .context("Failed to probe audio format")
}

/// Read tags, cover art (downscaled to a thumbnail) and duration from a file's
/// container metadata without decoding the audio. This is the cheap step run
/// during a folder scan; the expensive waveform peaks are computed on demand
/// via `compute_peaks` the first time a track is played.
pub fn read_metadata(path: &Path) -> Result<(TrackMeta, Option<Vec<u8>>)> {
    let mtime_secs = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let probed = open_probe(path)?;
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

    let sample_rate = track.codec_params.sample_rate.unwrap_or(44100);
    let channels = track
        .codec_params
        .channels
        .map(|c| c.count() as u16)
        .unwrap_or(2);

    // Duration from container metadata, no decode. Some formats (e.g. VBR MP3
    // without a Xing header) report no frame count; those read as 0.0 here and
    // get their exact duration filled in by compute_peaks on first play.
    let total_duration_secs = match (track.codec_params.n_frames, track.codec_params.time_base) {
        (Some(n), Some(tb)) => {
            let t = tb.calc_time(n);
            t.seconds as f64 + t.frac
        }
        (Some(n), None) => n as f64 / sample_rate as f64,
        _ => 0.0,
    };

    let cover_art = cover_art.map(|bytes| downscale_cover(&bytes));

    Ok((
        TrackMeta {
            path: path.to_path_buf(),
            artist,
            title,
            sample_rate,
            channels,
            total_duration_secs,
            mtime_secs,
        },
        cover_art,
    ))
}

/// Decode the full audio stream to compute the waveform peaks. Returns the
/// fixed-resolution peaks plus the exact decoded duration. This is the
/// expensive step, run lazily on first play rather than during the scan.
pub fn compute_peaks(path: &Path) -> Result<(TrackPeaks, f64)> {
    let probed = open_probe(path)?;
    let mut format = probed.format;

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
    let mut fine_peaks: Vec<f32> = Vec::new();
    let mut fine_peaks_max: Vec<f32> = Vec::new();
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

                if chunk_count >= FINE_CHUNK_SIZE {
                    let rms = (chunk_sum_sq / chunk_count as f64).sqrt() as f32;
                    fine_peaks.push(rms);
                    fine_peaks_max.push(chunk_max);
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
        fine_peaks.push(rms);
        fine_peaks_max.push(chunk_max);
    }

    // Resample the fine envelopes down to a fixed bin count per track.
    let (peaks, peaks_max) = resample_envelopes(&fine_peaks, &fine_peaks_max, TARGET_BINS);

    let total_duration_secs = total_mono_frames as f64 / sample_rate as f64;

    Ok((TrackPeaks { peaks, peaks_max }, total_duration_secs))
}
