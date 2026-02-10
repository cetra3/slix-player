use std::collections::HashMap;
use std::hash::{Hash, Hasher};

/// Poll timer interval in milliseconds.
pub const POLL_INTERVAL_MS: u64 = 50;

/// Number of poll ticks between automatic state saves (~2 seconds).
pub const SAVE_INTERVAL_TICKS: u32 = 40;

/// Sort column indices matching the UI header order.
pub const COL_ARTIST: i32 = 0;
pub const COL_TITLE: i32 = 1;
pub const COL_DURATION: i32 = 2;
pub const COL_MODIFIED: i32 = 3;

/// Result from background waveform loading thread.
pub struct WaveformResult {
    pub path: String,
    pub peaks: Vec<f32>,
    pub peaks_max: Vec<f32>,
    pub cover_art_rgba: Option<(Vec<u8>, u32, u32)>,
    pub cover_art_bytes: Option<Vec<u8>>,
}

pub struct SortState {
    pub column: i32,
    pub ascending: bool,
    pub shuffle: bool,
    pub shuffle_keys: HashMap<String, u64>,
}

impl SortState {
    pub fn new() -> Self {
        Self {
            column: 0,
            ascending: true,
            shuffle: false,
            shuffle_keys: HashMap::new(),
        }
    }

    /// Populate shuffle_keys with random ordering. The current track (if any)
    /// gets key 0 so it sorts first; all others get a seeded hash.
    pub fn generate_shuffle_keys<'a>(
        &mut self,
        paths: impl Iterator<Item = &'a str>,
        current_path: &str,
    ) {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(42);
        self.shuffle_keys.clear();
        for path in paths {
            if path == current_path {
                self.shuffle_keys.insert(path.to_string(), 0);
            } else {
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                seed.hash(&mut hasher);
                path.hash(&mut hasher);
                self.shuffle_keys.insert(path.to_string(), hasher.finish() | 1);
            }
        }
    }
}
