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

            match crate::audio::analyze_track(path) {
                Ok(t) => {
                    let entry = meta_to_track_entry(&t.meta);
                    if let Err(e) = db.put_track(&t.meta, &t.peaks, t.cover_art.as_deref()) {
                        eprintln!("Failed to cache track: {e}");
                    }
                    let _ = tx.try_send(ScanMsg::Track(entry));
                }
                Err(e) => {
                    eprintln!("Failed to analyze {}: {e}", path.display());
                }
            }
        });

        let _ = tx.try_send(ScanMsg::Done);
    });
}
