use anyhow::Result;
use fjall::{Database, KeyspaceCreateOptions, PersistMode};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TrackMeta {
    pub path: PathBuf,
    pub artist: String,
    pub title: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub total_duration_secs: f64,
    pub mtime_secs: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TrackPeaks {
    pub peaks: Vec<f32>,
    pub peaks_max: Vec<f32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlayerState {
    pub current_track: Option<String>,
    pub seek_secs: f64,
    pub volume: f32,
    pub last_folder: Option<PathBuf>,
    #[serde(default)]
    pub shuffle: bool,
    #[serde(default)]
    pub sort_column: i32,
    #[serde(default = "default_true")]
    pub sort_ascending: bool,
}

fn default_true() -> bool {
    true
}

impl Default for PlayerState {
    fn default() -> Self {
        Self {
            current_track: None,
            seek_secs: 0.0,
            volume: 1.0,
            last_folder: None,
            shuffle: false,
            sort_column: 0,
            sort_ascending: true,
        }
    }
}

const BINCODE_CONFIG: bincode::config::Configuration<bincode::config::LittleEndian, bincode::config::Varint> =
    bincode::config::standard();

pub struct TrackDatabase {
    db: Database,
    tracks: fjall::Keyspace,
    peaks: fjall::Keyspace,
    covers: fjall::Keyspace,
    state: fjall::Keyspace,
}

impl TrackDatabase {
    pub fn open<P: AsRef<Path>>(db_path: P) -> Result<Self> {
        let db = Database::builder(db_path).open()?;
        let tracks = db.keyspace("tracks", KeyspaceCreateOptions::default)?;
        let peaks = db.keyspace("peaks", KeyspaceCreateOptions::default)?;
        let covers = db.keyspace("covers", KeyspaceCreateOptions::default)?;
        let state = db.keyspace("state", KeyspaceCreateOptions::default)?;
        Ok(Self { db, tracks, peaks, covers, state })
    }

    /// Flush all pending writes to disk.
    pub fn persist(&self) -> Result<()> {
        self.db.persist(PersistMode::SyncAll)?;
        Ok(())
    }

    pub fn put_track(&self, meta: &TrackMeta, track_peaks: &TrackPeaks, cover_art: Option<&[u8]>) -> Result<()> {
        let key = meta.path.to_string_lossy().to_string();
        let meta_bytes = bincode::serde::encode_to_vec(meta, BINCODE_CONFIG)?;
        let peaks_bytes = bincode::serde::encode_to_vec(track_peaks, BINCODE_CONFIG)?;
        self.tracks.insert(key.as_bytes(), meta_bytes)?;
        self.peaks.insert(key.as_bytes(), peaks_bytes)?;
        if let Some(bytes) = cover_art {
            self.covers.insert(key.as_bytes(), bytes)?;
        }
        Ok(())
    }

    pub fn get_track_meta<P: AsRef<Path>>(&self, path: P) -> Result<Option<TrackMeta>> {
        let key = path.as_ref().to_string_lossy().to_string();
        if let Some(value) = self.tracks.get(key.as_bytes())? {
            let (meta, _) = bincode::serde::decode_from_slice::<TrackMeta, _>(&value, BINCODE_CONFIG)?;
            return Ok(Some(meta));
        }
        Ok(None)
    }

    pub fn get_track_peaks<P: AsRef<Path>>(&self, path: P) -> Result<Option<TrackPeaks>> {
        let key = path.as_ref().to_string_lossy().to_string();
        if let Some(value) = self.peaks.get(key.as_bytes())? {
            let (peaks, _) = bincode::serde::decode_from_slice::<TrackPeaks, _>(&value, BINCODE_CONFIG)?;
            return Ok(Some(peaks));
        }
        Ok(None)
    }

    pub fn get_cover_art<P: AsRef<Path>>(&self, path: P) -> Result<Option<Vec<u8>>> {
        let key = path.as_ref().to_string_lossy().to_string();
        if let Some(value) = self.covers.get(key.as_bytes())? {
            return Ok(Some(value.to_vec()));
        }
        Ok(None)
    }

    pub fn list_all_track_metas(&self) -> Result<Vec<TrackMeta>> {
        let mut metas = Vec::new();
        for guard in self.tracks.iter() {
            let (_key, value) = guard.into_inner()?;
            match bincode::serde::decode_from_slice::<TrackMeta, _>(&value, BINCODE_CONFIG) {
                Ok((meta, _)) => metas.push(meta),
                Err(e) => {
                    eprintln!("Failed to deserialize track meta: {e}");
                }
            }
        }
        Ok(metas)
    }

    pub fn get_player_state(&self) -> Result<Option<PlayerState>> {
        if let Some(value) = self.state.get(b"player")? {
            let (state, _) = bincode::serde::decode_from_slice::<PlayerState, _>(&value, BINCODE_CONFIG)?;
            Ok(Some(state))
        } else {
            Ok(None)
        }
    }

    pub fn put_player_state(&self, state: &PlayerState) -> Result<()> {
        let value = bincode::serde::encode_to_vec(state, BINCODE_CONFIG)?;
        self.state.insert(b"player", value)?;
        Ok(())
    }
}
