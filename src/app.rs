use crate::db::{PlayerState, TrackDatabase};
use crate::mpris::{MprisControls, MprisTrackInfo};
use crate::state::{WaveformResult, POLL_INTERVAL_MS, SAVE_INTERVAL_TICKS};
use crate::track_list::TrackListController;
use crate::utils::{format_duration_secs, meta_to_track_entry};
use crate::{waveform, AppWindow, NowPlaying, PlayState, TrackListState, TrackEntry};
use anyhow::Result;
use slint::{ComponentHandle, SharedString};
use souvlaki::{MediaPlayback, MediaPosition};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

pub struct App {
    pub weak: slint::Weak<AppWindow>,
    pub track_list: Rc<TrackListController>,
    pub sink: Rc<rodio::Player>,
    pub db: Arc<TrackDatabase>,
    media_controls: RefCell<MprisControls>,
    waveform_tx: mpsc::Sender<String>,
    // Rust-only internal state (no Slint-side equivalent)
    current_peaks: RefCell<Vec<f32>>,
    current_peaks_max: RefCell<Vec<f32>>,
    last_waveform_width: RefCell<u32>,
    last_folder: RefCell<PathBuf>,
    pending_scroll: RefCell<i32>,
    waveform_rx: RefCell<Option<mpsc::Receiver<WaveformResult>>>,
    main_timer: RefCell<Option<slint::Timer>>,
}

impl App {
    pub fn build(
        window: &AppWindow,
        track_list: Rc<TrackListController>,
        sink: Rc<rodio::Player>,
        db: Arc<TrackDatabase>,
        last_folder: PathBuf,
    ) -> Result<Rc<Self>> {
        // Persistent background thread for loading peaks + cover art from DB.
        let (request_tx, request_rx) = mpsc::channel::<String>();
        let (result_tx, result_rx) = mpsc::channel::<WaveformResult>();

        let db_for_thread = db.clone();
        std::thread::spawn(move || {
            while let Ok(path) = request_rx.recv() {
                let track_peaks = match db_for_thread.get_track_peaks(&path) {
                    Ok(Some(p)) => p,
                    Ok(None) => continue,
                    Err(e) => {
                        eprintln!("Failed to load peaks for {path}: {e}");
                        continue;
                    }
                };

                let cover_art_bytes = match db_for_thread.get_cover_art(&path) {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        eprintln!("Failed to load cover art for {path}: {e}");
                        None
                    }
                };

                let cover_art_rgba = cover_art_bytes.as_ref().and_then(|bytes| {
                    image::load_from_memory(bytes).ok().map(|img| {
                        let rgba = img.to_rgba8();
                        let (w, h) = rgba.dimensions();
                        (rgba.into_raw(), w, h)
                    })
                });

                let _ = result_tx.send(WaveformResult {
                    path,
                    peaks: track_peaks.peaks,
                    peaks_max: track_peaks.peaks_max,
                    cover_art_rgba,
                    cover_art_bytes,
                });
            }
        });

        let media_controls = MprisControls::init(window.as_weak())?;

        Ok(Rc::new(Self {
            weak: window.as_weak(),
            track_list,
            sink,
            db,
            media_controls: RefCell::new(media_controls),
            waveform_tx: request_tx,
            current_peaks: RefCell::new(Vec::new()),
            current_peaks_max: RefCell::new(Vec::new()),
            last_waveform_width: RefCell::new(0),
            last_folder: RefCell::new(last_folder),
            pending_scroll: RefCell::new(-1),
            waveform_rx: RefCell::new(Some(result_rx)),
            main_timer: RefCell::new(None),
        }))
    }

    fn window(&self) -> AppWindow {
        self.weak.upgrade().expect("window already closed")
    }

    /// Read NowPlaying.path from the global.
    fn now_playing_path(&self) -> String {
        self.window().global::<NowPlaying>().get_path().to_string()
    }

    /// Read NowPlaying.duration-secs from the global.
    fn now_playing_duration(&self) -> f64 {
        self.window().global::<NowPlaying>().get_duration_secs() as f64
    }

    // ── App-level operations ──────────────────────────────────────────

    /// Save current player state to the database.
    pub fn save_state(&self) {
        let path = self.now_playing_path();
        let dur = self.now_playing_duration();
        let folder = self.last_folder.borrow().clone();
        let _ = self.db.put_player_state(&PlayerState {
            current_track: if path.is_empty() { None } else { Some(path) },
            seek_secs: self.sink.get_pos().as_secs_f64().min(dur),
            volume: self.sink.volume(),
            last_folder: if folder.as_os_str().is_empty() {
                None
            } else {
                Some(folder)
            },
            shuffle: self.track_list.is_shuffle(),
            sort_column: self.track_list.sort_column(),
            sort_ascending: self.track_list.sort_ascending(),
        });
    }

    /// Load a track into the audio player and update NowPlaying globals.
    fn load_track(&self, entry: &TrackEntry) -> bool {
        self.sink.stop();

        let path_str = entry.path.as_str();
        match std::fs::File::open(path_str) {
            Ok(file) => match rodio::Decoder::try_from(file) {
                Ok(source) => {
                    self.sink.append(source);
                }
                Err(e) => {
                    eprintln!("Failed to decode {path_str}: {e}");
                    return false;
                }
            },
            Err(e) => {
                eprintln!("Failed to open {path_str}: {e}");
                return false;
            }
        }

        self.current_peaks.borrow_mut().clear();
        self.current_peaks_max.borrow_mut().clear();
        *self.last_waveform_width.borrow_mut() = 0;

        let window = self.window();
        let np = window.global::<NowPlaying>();
        np.set_path(entry.path.clone());
        np.set_artist(entry.artist.clone());
        np.set_title(entry.title.clone());
        np.set_duration_secs(entry.duration_secs);
        np.set_total_time_text(SharedString::from(format_duration_secs(
            entry.duration_secs as f64,
        )));
        np.set_waveform_image(slint::Image::default());
        np.set_cover_art(slint::Image::default());

        true
    }

    /// Request the background thread to load peaks + cover art from the DB.
    fn start_waveform_load(&self, path: String) {
        let _ = self.waveform_tx.send(path);
    }

    /// Called from the poll timer. Drains all pending results from the background
    /// loader and applies the latest one matching the current track.
    fn poll_waveform_ready(&self) {
        let latest = {
            let rx = self.waveform_rx.borrow();
            let Some(rx) = rx.as_ref() else { return };
            let mut latest = None;
            while let Ok(result) = rx.try_recv() {
                latest = Some(result);
            }
            latest
        };

        let Some(result) = latest else { return };

        let window = self.window();
        let np = window.global::<NowPlaying>();

        if result.path != np.get_path().as_str() {
            return;
        }

        let scale = window.window().scale_factor();

        let w = window.get_waveform_area_width() as u32;
        if w > 0 {
            np.set_waveform_image(waveform::render_waveform(
                &result.peaks,
                &result.peaks_max,
                w,
                waveform::WAVEFORM_HEIGHT,
                scale,
            ));
        }

        match result.cover_art_rgba {
            Some((rgba, cw, ch)) => {
                np.set_cover_art(waveform::image_from_rgba(&rgba, cw, ch));
            }
            None => {
                np.set_cover_art(waveform::render_cover_art(
                    &result.peaks,
                    &result.peaks_max,
                    scale,
                ));
            }
        }

        // Cache peaks for resize
        if w > 0 {
            *self.last_waveform_width.borrow_mut() = w;
        }
        *self.current_peaks.borrow_mut() = result.peaks;
        *self.current_peaks_max.borrow_mut() = result.peaks_max;

        // Update MPRIS metadata from NowPlaying globals
        let artist = np.get_artist().to_string();
        let title = np.get_title().to_string();
        let path = np.get_path().to_string();
        let duration_secs = np.get_duration_secs() as f64;
        self.media_controls.borrow_mut().set_metadata(
            &MprisTrackInfo {
                artist: &artist,
                title: &title,
                duration_secs,
                path: &path,
            },
            result.cover_art_bytes.as_deref(),
        );
    }

    // ── State restoration ─────────────────────────────────────────────

    /// Restore previously saved state (tracks, shuffle, track position) on startup.
    pub fn restore_state(self: &Rc<Self>, saved_state: &PlayerState) {
        let last_folder = self.last_folder.borrow().clone();

        let metas = match self.db.list_all_track_metas() {
            Ok(t) => t,
            Err(e) => {
                eprintln!("Failed to load cached tracks: {e}");
                return;
            }
        };

        let mut saved_entry: Option<TrackEntry> = None;
        for m in &metas {
            if !last_folder.as_os_str().is_empty() && !m.path.starts_with(&last_folder) {
                continue;
            }
            let entry = meta_to_track_entry(m);
            if saved_entry.is_none() {
                if let Some(ref saved_path) = saved_state.current_track {
                    if entry.path.as_str() == saved_path.as_str() {
                        saved_entry = Some(entry.clone());
                    }
                }
            }
            self.track_list.push_track(entry);
        }

        // Force a full model rebuild after the bulk load so column widths
        // are calculated correctly from the start.
        self.track_list.resync(true);

        self.track_list.restore_sort(saved_state.sort_column, saved_state.sort_ascending);

        if saved_state.shuffle {
            let current_path = saved_state.current_track.as_deref().unwrap_or("");
            self.track_list.restore_shuffle(current_path);
        }

        if let Some(ref entry) = saved_entry {
            if self.load_track(entry) {
                self.start_waveform_load(entry.path.to_string());
                if saved_state.seek_secs > 0.0 {
                    let _ = self
                        .sink
                        .try_seek(Duration::from_secs_f64(saved_state.seek_secs));
                }
                self.sink.pause();
            }
            let idx = self.track_list.find_index_by_path(entry.path.as_str());
            *self.pending_scroll.borrow_mut() = idx;
        }

        let last_folder = self.last_folder.borrow().clone();
        if !last_folder.as_os_str().is_empty() && last_folder.is_dir() {
            self.scan_folder(last_folder);
        }
    }

    // ── Load folder helpers ───────────────────────────────────────────

    /// Reset player state when switching to a different folder.
    fn clear_for_new_folder(&self) {
        self.sink.stop();
        self.sink.pause();

        self.current_peaks.borrow_mut().clear();
        self.current_peaks_max.borrow_mut().clear();

        self.track_list.clear();

        let window = self.window();
        let np = window.global::<NowPlaying>();
        np.set_path(SharedString::default());
        np.set_artist(SharedString::default());
        np.set_title(SharedString::default());
        np.set_duration_secs(0.0);
        np.set_total_time_text(SharedString::from("0:00"));
        np.set_waveform_image(slint::Image::default());
        np.set_cover_art(slint::Image::default());
        np.set_progress(0.0);
        np.set_current_time_text(SharedString::from("0:00"));
    }

    /// Start a background scan of the given folder for new/changed tracks.
    fn scan_folder(self: &Rc<Self>, folder: PathBuf) {
        let existing = self.track_list.existing_paths();
        self.track_list.start_scan(folder, self.db.clone(), existing);
    }

    // ── Poll timer helpers ────────────────────────────────────────────

    /// Update playback progress bar and time display.
    fn poll_playback_progress(&self) {
        let dur = self.now_playing_duration();
        let pos = self.sink.get_pos();
        let progress = if dur > 0.0 {
            pos.as_secs_f64() / dur
        } else {
            0.0
        };
        let window = self.window();
        let np = window.global::<NowPlaying>();
        np.set_progress(progress.min(1.0) as f32);
        np.set_current_time_text(SharedString::from(format_duration_secs(pos.as_secs_f64())));
        window
            .global::<PlayState>()
            .set_is_playing(!self.sink.is_paused() && !self.sink.empty());
    }

    /// Advance to next track when current track finishes naturally.
    fn poll_auto_advance(&self) {
        let dur = self.now_playing_duration();
        if !self.sink.is_paused() && self.sink.empty() && dur > 0.0 {
            let path = self.now_playing_path();
            let idx = self.track_list.find_index_by_path(&path);
            let count = self.track_list.track_count() as i32;
            if idx >= 0 && idx < count - 1 {
                if let Some(entry) = self.track_list.get_track_at((idx + 1) as usize) {
                    self.play_track_entry(&entry, true);
                }
            } else {
                // No more tracks — clear duration so we don't keep firing
                self.window()
                    .global::<NowPlaying>()
                    .set_duration_secs(0.0);
            }
        }
    }

    /// Execute deferred scroll-to-track once layout is ready after startup.
    fn poll_deferred_scroll(&self) {
        let pending = *self.pending_scroll.borrow();
        if pending >= 0 {
            let window = self.window();
            if window.get_waveform_area_width() > 0.0 {
                window.invoke_scroll_to_track(pending);
                *self.pending_scroll.borrow_mut() = -1;
            }
        }
    }

    /// Re-render waveform if the container width changed, using cached peaks.
    fn poll_waveform_resize(&self) {
        let window = self.window();
        let current_w = window.get_waveform_area_width() as u32;
        let last_w = *self.last_waveform_width.borrow();
        if current_w > 0 && current_w != last_w {
            let peaks = self.current_peaks.borrow();
            if peaks.is_empty() {
                return;
            }
            let peaks_max = self.current_peaks_max.borrow();
            let scale = window.window().scale_factor();
            let img = waveform::render_waveform(
                &peaks,
                &peaks_max,
                current_w,
                waveform::WAVEFORM_HEIGHT,
                scale,
            );
            drop(peaks);
            drop(peaks_max);
            window.global::<NowPlaying>().set_waveform_image(img);
            *self.last_waveform_width.borrow_mut() = current_w;
        }
    }

    /// Load and play a track from a TrackEntry (used by track-selected, next, prev).
    /// When `scroll` is true, the list scrolls to the track.
    fn play_track_entry(&self, entry: &TrackEntry, scroll: bool) {
        let window = self.window();
        let np = window.global::<NowPlaying>();
        np.set_progress(0.0);
        np.set_current_time_text(SharedString::from("0:00"));

        if self.load_track(entry) {
            self.start_waveform_load(entry.path.to_string());
            self.sink.play();
        }

        if scroll {
            // Resync resets the filtered model and scrolls to the current track.
            // Without this, Slint's ListView can render stale column widths
            // until the window is resized.
            self.track_list.resync(false);
        }

        self.save_state();
    }

    pub fn register_all(self: &Rc<Self>) {
        self.register_drag_window();
        self.register_close_window();
        self.register_load_folder();
        self.register_track_selected();
        self.register_play_pause();
        self.register_volume_changed();
        self.register_seek();
        self.register_next();
        self.register_prev();
        self.register_poll_timer();
    }

    fn register_drag_window(self: &Rc<Self>) {
        let weak = self.weak.clone();
        self.window().on_drag_window(move || {
            if let Some(app) = weak.upgrade() {
                use slint::winit_030::WinitWindowAccessor;
                let _ = app.window().with_winit_window(|w| {
                    let _ = w.drag_window();
                });
                let weak_inner = app.as_weak();
                slint::Timer::single_shot(Duration::from_millis(0), move || {
                    if let Some(app) = weak_inner.upgrade() {
                        app.window().dispatch_event(
                            slint::platform::WindowEvent::PointerReleased {
                                position: slint::LogicalPosition::new(0.0, 0.0),
                                button: slint::platform::PointerEventButton::Left,
                            },
                        );
                        app.window()
                            .dispatch_event(slint::platform::WindowEvent::PointerExited);
                    }
                });
            }
        });
    }

    fn register_close_window(self: &Rc<Self>) {
        let weak = self.weak.clone();
        self.window().on_close_window(move || {
            if let Some(app) = weak.upgrade() {
                let _ = app.hide();
            }
        });
    }

    fn register_load_folder(self: &Rc<Self>) {
        let app = self.clone();
        self.window().on_load_folder(move || {
            let app = app.clone();
            slint::spawn_local(async move {
                let mut dialog =
                    rfd::AsyncFileDialog::new().set_title("Select music folder");

                let lf = app.last_folder.borrow().clone();
                if !lf.as_os_str().is_empty() && lf.is_dir() {
                    dialog = dialog.set_directory(&lf);
                }

                let Some(folder) = dialog.pick_folder().await else {
                    return;
                };

                let folder = folder.path().to_path_buf();
                let same_folder = folder == *app.last_folder.borrow();
                *app.last_folder.borrow_mut() = folder.clone();

                if !same_folder {
                    app.clear_for_new_folder();
                }

                app.save_state();
                app.scan_folder(folder);
            })
            .unwrap();
        });
    }

    fn register_track_selected(self: &Rc<Self>) {
        let app = self.clone();
        self.window()
            .global::<TrackListState>()
            .on_track_selected(move |entry| {
                app.play_track_entry(&entry, false);
            });
    }

    fn register_play_pause(self: &Rc<Self>) {
        let app = self.clone();
        self.window()
            .global::<PlayState>()
            .on_play_pause(move || {
                if app.sink.is_paused() {
                    app.sink.play();
                } else {
                    app.sink.pause();
                }
            });
    }

    fn register_volume_changed(self: &Rc<Self>) {
        let app = self.clone();
        self.window()
            .global::<PlayState>()
            .on_volume_changed(move |vol| {
                app.sink.set_volume(vol);
                app.save_state();
            });
    }

    fn register_seek(self: &Rc<Self>) {
        let app = self.clone();
        self.window().global::<PlayState>().on_seek(move |progress| {
            let dur = app.now_playing_duration();
            let target = Duration::from_secs_f64(progress as f64 * dur);
            if let Err(e) = app.sink.try_seek(target) {
                eprintln!("seek error: {e}");
            }
            let window = app.window();
            let np = window.global::<NowPlaying>();
            np.set_progress(progress);
            np.set_current_time_text(SharedString::from(format_duration_secs(
                progress as f64 * dur,
            )));
        });
    }

    fn register_next(self: &Rc<Self>) {
        let app = self.clone();
        self.window().global::<PlayState>().on_next(move || {
            let path = app.now_playing_path();
            let idx = app.track_list.find_index_by_path(&path);
            let count = app.track_list.track_count() as i32;
            if idx >= 0 && idx < count - 1 {
                if let Some(entry) = app.track_list.get_track_at((idx + 1) as usize) {
                    app.play_track_entry(&entry, true);
                }
            }
        });
    }

    fn register_prev(self: &Rc<Self>) {
        let app = self.clone();
        self.window().global::<PlayState>().on_prev(move || {
            let path = app.now_playing_path();
            let idx = app.track_list.find_index_by_path(&path);
            if idx > 0 {
                if let Some(entry) = app.track_list.get_track_at((idx - 1) as usize) {
                    app.play_track_entry(&entry, true);
                }
            }
        });
    }

    fn register_poll_timer(self: &Rc<Self>) {
        let timer = slint::Timer::default();
        let app = self.clone();
        let mut save_counter: u32 = 0;
        let mut last_mpris_state: Option<bool> = None;

        timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(POLL_INTERVAL_MS),
            move || {
                if app.weak.upgrade().is_none() {
                    return;
                }

                app.poll_playback_progress();
                app.poll_auto_advance();
                app.poll_deferred_scroll();
                app.poll_waveform_ready();
                app.poll_waveform_resize();

                // Only update MPRIS playback status when it changes.
                let is_paused = app.sink.is_paused();
                let is_empty = app.sink.empty();
                let mpris_state = if is_empty { None } else { Some(!is_paused) };
                if mpris_state != last_mpris_state {
                    last_mpris_state = mpris_state;
                    let progress_secs = app.sink.get_pos().as_secs_f64();
                    let playback = if is_empty {
                        MediaPlayback::Stopped
                    } else if !is_paused {
                        MediaPlayback::Playing {
                            progress: Some(MediaPosition(Duration::from_secs_f64(progress_secs))),
                        }
                    } else {
                        MediaPlayback::Paused {
                            progress: Some(MediaPosition(Duration::from_secs_f64(progress_secs))),
                        }
                    };
                    app.media_controls.borrow_mut().set_playback(playback);
                }

                save_counter += 1;
                if save_counter >= SAVE_INTERVAL_TICKS {
                    save_counter = 0;
                    if !app.now_playing_path().is_empty() {
                        app.save_state();
                    }
                }
            },
        );

        *self.main_timer.borrow_mut() = Some(timer);
    }
}
