use crate::db::TrackDatabase;
use crate::utils::meta_to_track_entry;
use crate::TrackEntry;
use std::collections::HashSet;
use std::path::PathBuf;

pub enum ScanMsg {
    Total(usize),
    Track(TrackEntry),
    Done,
}

/// Spawn a background thread that discovers and analyzes audio files in `folder`.
/// Tracks already present in `existing` are skipped. Results are sent over `tx`.
pub fn start_scan(
    folder: PathBuf,
    db: std::sync::Arc<TrackDatabase>,
    existing: HashSet<String>,
    tx: async_channel::Sender<ScanMsg>,
) {
    std::thread::spawn(move || {
        use rayon::prelude::*;

        let paths = match crate::audio::collect_audio_files(&folder) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("Error scanning folder: {e}");
                let _ = tx.try_send(ScanMsg::Done);
                return;
            }
        };

        let new_paths: Vec<_> = paths
            .into_iter()
            .filter(|p| !existing.contains(&*p.to_string_lossy()))
            .collect();

        let _ = tx.try_send(ScanMsg::Total(new_paths.len()));

        new_paths.par_iter().for_each(|path| {
            if let Ok(Some(meta)) = db.get_track_meta(path) {
                let _ = tx.try_send(ScanMsg::Track(meta_to_track_entry(&meta)));
                return;
            }

            match crate::audio::read_metadata(path) {
                Ok((meta, cover_art)) => {
                    let entry = meta_to_track_entry(&meta);
                    if let Err(e) = db.put_meta(&meta) {
                        eprintln!("Failed to cache track meta: {e}");
                    }
                    if let Some(bytes) = cover_art {
                        if let Err(e) = db.put_cover(path, &bytes) {
                            eprintln!("Failed to cache cover art: {e}");
                        }
                    }
                    let _ = tx.try_send(ScanMsg::Track(entry));
                }
                Err(e) => {
                    eprintln!("Failed to read metadata for {}: {e}", path.display());
                }
            }
        });

        let _ = tx.try_send(ScanMsg::Done);
    });
}
