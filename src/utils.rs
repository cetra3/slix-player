use crate::db::TrackMeta;
use crate::TrackEntry;
use slint::SharedString;

pub fn format_duration_secs(secs: f64) -> String {
    let total = secs as u64;
    let mins = total / 60;
    let s = total % 60;
    format!("{}:{:02}", mins, s)
}

pub fn format_mtime(secs: i64) -> String {
    if secs == 0 {
        return String::from("—");
    }
    chrono::DateTime::from_timestamp(secs, 0)
        .map(|dt| dt.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| String::from("—"))
}

pub fn meta_to_track_entry(meta: &TrackMeta) -> TrackEntry {
    TrackEntry {
        artist: SharedString::from(&meta.artist),
        title: SharedString::from(&meta.title),
        duration_text: SharedString::from(format_duration_secs(meta.total_duration_secs)),
        duration_secs: meta.total_duration_secs as f32,
        modified_text: SharedString::from(format_mtime(meta.mtime_secs)),
        mtime_secs: meta.mtime_secs as i32,
        path: SharedString::from(meta.path.to_string_lossy().as_ref()),
    }
}
