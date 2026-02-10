use crate::{AppWindow, PlayState};
use anyhow::Result;
use slint::ComponentHandle;
use souvlaki::{MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, PlatformConfig};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::time::Duration;

pub struct MprisControls {
    controls: MediaControls,
    cover_art_path: Option<PathBuf>,
}

/// Metadata needed by MPRIS — derived from NowPlaying globals, no TrackMeta needed.
pub struct MprisTrackInfo<'a> {
    pub artist: &'a str,
    pub title: &'a str,
    pub duration_secs: f64,
    pub path: &'a str,
}

impl MprisControls {
    pub fn init(weak: slint::Weak<AppWindow>) -> Result<Self> {
        let config = PlatformConfig {
            dbus_name: "slix_player",
            display_name: "Slix Player",
            hwnd: None,
        };

        let mut controls = MediaControls::new(config)?;

        controls.attach(move |event: MediaControlEvent| {
            weak.upgrade_in_event_loop(move |window| {
                let ps = window.global::<PlayState>();
                match event {
                    MediaControlEvent::Toggle => {
                        ps.invoke_play_pause();
                    }
                    MediaControlEvent::Play => {
                        if !ps.get_is_playing() {
                            ps.invoke_play_pause();
                        }
                    }
                    MediaControlEvent::Pause | MediaControlEvent::Stop => {
                        if ps.get_is_playing() {
                            ps.invoke_play_pause();
                        }
                    }
                    MediaControlEvent::Next => {
                        ps.invoke_next();
                    }
                    MediaControlEvent::Previous => {
                        ps.invoke_prev();
                    }
                    _ => {}
                }
            }).ok();
        })?;

        Ok(Self {
            controls,
            cover_art_path: None,
        })
    }

    pub fn set_metadata(&mut self, info: &MprisTrackInfo, cover_art: Option<&[u8]>) {
        let cover_url_string;
        let cover_url = if let Some(bytes) = cover_art {
            if let Some(cache_dir) = dirs::cache_dir() {
                let mut hasher = DefaultHasher::new();
                info.path.hash(&mut hasher);
                let filename = format!("cover-{:016x}.img", hasher.finish());
                let cover_path = cache_dir.join("slix-player").join(filename);
                if let Some(parent) = cover_path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                if std::fs::write(&cover_path, bytes).is_ok() {
                    if let Some(ref old) = self.cover_art_path {
                        if *old != cover_path {
                            let _ = std::fs::remove_file(old);
                        }
                    }
                    self.cover_art_path = Some(cover_path.clone());
                    cover_url_string = format!("file://{}", cover_path.display());
                    Some(cover_url_string.as_str())
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            if let Some(old) = self.cover_art_path.take() {
                let _ = std::fs::remove_file(old);
            }
            None
        };

        let _ = self.controls.set_metadata(MediaMetadata {
            title: Some(info.title),
            artist: Some(info.artist),
            duration: Some(Duration::from_secs_f64(info.duration_secs)),
            cover_url,
            ..Default::default()
        });
    }

    pub fn set_playback(&mut self, playback: MediaPlayback) {
        let _ = self.controls.set_playback(playback);
    }
}
