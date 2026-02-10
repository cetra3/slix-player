use crate::db::TrackDatabase;
use crate::scan::ScanMsg;
use crate::state::{SortState, COL_ARTIST, COL_DURATION, COL_MODIFIED, COL_TITLE};
use crate::{AppWindow, NowPlaying, TrackEntry, TrackListState};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;

pub struct TrackListController {
    weak: slint::Weak<AppWindow>,
    track_model: Rc<VecModel<TrackEntry>>,
    filtered_model: ModelRc<TrackEntry>,
    sorted_model_reset: Rc<dyn Fn()>,
    filtered_model_reset: Rc<dyn Fn()>,
    sort: Rc<RefCell<SortState>>,
    filter_text: Rc<RefCell<String>>,
    scan_cancel: RefCell<Option<Rc<Cell<bool>>>>,
}

impl TrackListController {
    pub fn new(window: &AppWindow) -> Rc<Self> {
        let track_model: Rc<VecModel<TrackEntry>> = Rc::new(VecModel::default());
        let sort = Rc::new(RefCell::new(SortState::new()));
        let filter_text = Rc::new(RefCell::new(String::new()));

        // Model chain: VecModel -> SortModel -> FilterModel -> UI
        let sort_for_closure = sort.clone();
        let sorted_model = Rc::new(slint::SortModel::new(
            track_model.clone(),
            move |a: &TrackEntry, b: &TrackEntry| {
                let sort = sort_for_closure.borrow();
                if sort.shuffle {
                    let key_a = sort
                        .shuffle_keys
                        .get(a.path.as_str())
                        .copied()
                        .unwrap_or(u64::MAX);
                    let key_b = sort
                        .shuffle_keys
                        .get(b.path.as_str())
                        .copied()
                        .unwrap_or(u64::MAX);
                    return key_a.cmp(&key_b);
                }
                let ord = match sort.column {
                    COL_ARTIST => a.artist.to_lowercase().cmp(&b.artist.to_lowercase()),
                    COL_TITLE => a.title.to_lowercase().cmp(&b.title.to_lowercase()),
                    COL_DURATION => a.duration_text.cmp(&b.duration_text),
                    COL_MODIFIED => a.mtime_secs.cmp(&b.mtime_secs),
                    _ => std::cmp::Ordering::Equal,
                };
                if sort.ascending {
                    ord
                } else {
                    ord.reverse()
                }
            },
        ));

        let filter_for_closure = filter_text.clone();
        let filtered_model = Rc::new(slint::FilterModel::new(
            sorted_model.clone(),
            move |entry: &TrackEntry| {
                let ft = filter_for_closure.borrow();
                if ft.is_empty() {
                    return true;
                }
                entry
                    .artist
                    .to_lowercase()
                    .contains(ft.as_str())
                    || entry
                        .title
                        .to_lowercase()
                        .contains(ft.as_str())
            },
        ));

        window
            .global::<TrackListState>()
            .set_tracks(ModelRc::from(filtered_model.clone()));

        let sorted_for_reset = sorted_model.clone();
        let sorted_model_reset: Rc<dyn Fn()> = Rc::new(move || sorted_for_reset.reset());
        let filtered_for_reset = filtered_model.clone();
        let filtered_model_reset: Rc<dyn Fn()> = Rc::new(move || filtered_for_reset.reset());

        Rc::new(Self {
            weak: window.as_weak(),
            track_model,
            filtered_model: ModelRc::from(filtered_model),
            sorted_model_reset,
            filtered_model_reset,
            sort,
            filter_text,
            scan_cancel: RefCell::new(None),
        })
    }

    fn window(&self) -> AppWindow {
        self.weak.upgrade().expect("window already closed")
    }

    pub fn register_callbacks(self: &Rc<Self>) {
        self.register_sort_changed();
        self.register_shuffle_toggled();
        self.register_filter_changed();
    }

    fn register_sort_changed(self: &Rc<Self>) {
        let ctrl = self.clone();
        self.window()
            .global::<TrackListState>()
            .on_sort_changed(move |col| {
                {
                    let mut sort = ctrl.sort.borrow_mut();
                    if sort.column == col {
                        sort.ascending = !sort.ascending;
                    } else {
                        sort.column = col;
                        sort.ascending = true;
                    }
                    sort.shuffle = false;
                    sort.shuffle_keys.clear();
                }
                let window = ctrl.window();
                let sort = ctrl.sort.borrow();
                window
                    .global::<TrackListState>()
                    .set_sort_column(sort.column);
                window
                    .global::<TrackListState>()
                    .set_sort_ascending(sort.ascending);
                window
                    .global::<TrackListState>()
                    .set_shuffle_enabled(false);
                drop(sort);
                ctrl.resync(true);
            });
    }

    fn register_shuffle_toggled(self: &Rc<Self>) {
        let ctrl = self.clone();
        self.window()
            .global::<TrackListState>()
            .on_shuffle_toggled(move || {
                {
                    let mut sort = ctrl.sort.borrow_mut();
                    sort.shuffle = !sort.shuffle;
                    if sort.shuffle {
                        let current_path = ctrl
                            .window()
                            .global::<NowPlaying>()
                            .get_path()
                            .to_string();
                        let paths: Vec<_> = (0..ctrl.track_model.row_count())
                            .filter_map(|i| ctrl.track_model.row_data(i))
                            .map(|e| e.path.to_string())
                            .collect();
                        sort.generate_shuffle_keys(
                            paths.iter().map(|s| s.as_str()),
                            &current_path,
                        );
                    } else {
                        sort.shuffle_keys.clear();
                    }
                }
                ctrl.window()
                    .global::<TrackListState>()
                    .set_shuffle_enabled(ctrl.sort.borrow().shuffle);
                ctrl.resync(true);
            });
    }

    fn register_filter_changed(self: &Rc<Self>) {
        let ctrl = self.clone();
        self.window()
            .global::<TrackListState>()
            .on_filter_changed(move |text| {
                *ctrl.filter_text.borrow_mut() = text.to_lowercase().to_string();
                ctrl.resync(false);
            });
    }

    /// Reset sort/filter models and scroll to the current track.
    /// The model reset forces Slint's ListView to rebuild all rows, which
    /// corrects column widths that can otherwise go stale after incremental
    /// model updates (pushes during scan, track changes, etc.).
    pub fn resync(&self, reset_sort: bool) {
        if reset_sort {
            (self.sorted_model_reset)();
        }
        (self.filtered_model_reset)();
        let idx = self.find_index_by_path(&self.window().global::<NowPlaying>().get_path());
        self.window().invoke_scroll_to_track(idx);
    }

    /// Find the index of a track path in the filtered model.
    pub fn find_index_by_path(&self, path: &str) -> i32 {
        if path.is_empty() {
            return -1;
        }
        for i in 0..self.filtered_model.row_count() {
            if let Some(entry) = self.filtered_model.row_data(i) {
                if entry.path.as_str() == path {
                    return i as i32;
                }
            }
        }
        -1
    }

    pub fn get_track_at(&self, index: usize) -> Option<TrackEntry> {
        self.filtered_model.row_data(index)
    }

    pub fn track_count(&self) -> usize {
        self.filtered_model.row_count()
    }

    pub fn push_track(&self, entry: TrackEntry) {
        self.track_model.push(entry);
    }

    pub fn clear(&self) {
        // Cancel any running scan
        if let Some(cancel) = self.scan_cancel.borrow().as_ref() {
            cancel.set(true);
        }
        while self.track_model.row_count() > 0 {
            self.track_model.remove(self.track_model.row_count() - 1);
        }
    }

    pub fn existing_paths(&self) -> HashSet<String> {
        (0..self.track_model.row_count())
            .filter_map(|i| self.track_model.row_data(i))
            .map(|e| e.path.to_string())
            .collect()
    }

    pub fn is_shuffle(&self) -> bool {
        self.sort.borrow().shuffle
    }

    /// Restore shuffle state from saved state.
    pub fn restore_shuffle(&self, current_path: &str) {
        let mut sort = self.sort.borrow_mut();
        sort.shuffle = true;
        let paths: Vec<_> = (0..self.track_model.row_count())
            .filter_map(|i| self.track_model.row_data(i))
            .map(|e| e.path.to_string())
            .collect();
        sort.generate_shuffle_keys(paths.iter().map(|s| s.as_str()), current_path);
        drop(sort);
        self.resync(true);
        self.window()
            .global::<TrackListState>()
            .set_shuffle_enabled(true);
    }

    /// Start a background scan, draining results into the model via `spawn_local`.
    pub fn start_scan(
        self: &Rc<Self>,
        folder: PathBuf,
        db: std::sync::Arc<TrackDatabase>,
        existing: HashSet<String>,
    ) {
        // Cancel any previous scan
        if let Some(prev) = self.scan_cancel.borrow().as_ref() {
            prev.set(true);
        }

        self.window()
            .global::<TrackListState>()
            .set_loading(true);

        let (tx, rx) = async_channel::unbounded();
        crate::scan::start_scan(folder, db, existing, tx);

        let cancel = Rc::new(Cell::new(false));
        *self.scan_cancel.borrow_mut() = Some(cancel.clone());

        let track_model = self.track_model.clone();
        let weak = self.weak.clone();
        let sorted_reset = self.sorted_model_reset.clone();
        let filtered_reset = self.filtered_model_reset.clone();

        // Seed with paths already in the model so we never push duplicates.
        let mut seen: HashSet<String> = (0..track_model.row_count())
            .filter_map(|i| track_model.row_data(i))
            .map(|e| e.path.to_string())
            .collect();

        slint::spawn_local(async move {
            let mut scan_total = 0usize;
            let mut scan_done = 0usize;

            while let Ok(msg) = rx.recv().await {
                if cancel.get() {
                    break;
                }

                // Process the awaited message plus any already-buffered messages
                // in one batch before yielding back to the event loop.
                let finished = process_scan_msg(
                    msg,
                    &track_model,
                    &mut seen,
                    &mut scan_total,
                    &mut scan_done,
                );
                if finished {
                    // Reset models once at the end to fix column widths.
                    // We intentionally avoid resetting per-batch because it
                    // rebuilds every ListView row, swallowing click events.
                    (sorted_reset)();
                    (filtered_reset)();
                    set_loading_done(&weak);
                    break;
                }

                // Drain buffered messages without yielding
                while let Ok(msg) = rx.try_recv() {
                    if cancel.get() {
                        break;
                    }
                    let finished = process_scan_msg(
                        msg,
                        &track_model,
                        &mut seen,
                        &mut scan_total,
                        &mut scan_done,
                    );
                    if finished {
                        (sorted_reset)();
                        (filtered_reset)();
                        set_loading_done(&weak);
                        return;
                    }
                }

                if cancel.get() {
                    break;
                }

                // Update progress text once per batch
                if scan_total > 0 {
                    if let Some(window) = weak.upgrade() {
                        window
                            .global::<TrackListState>()
                            .set_loading_text(SharedString::from(format!(
                                "Analysing... {}/{}",
                                scan_done, scan_total
                            )));
                    }
                }
            }
        })
        .unwrap();
    }
}

/// Process a single scan message. Returns `true` if the scan is finished.
fn process_scan_msg(
    msg: ScanMsg,
    track_model: &Rc<VecModel<TrackEntry>>,
    seen: &mut HashSet<String>,
    scan_total: &mut usize,
    scan_done: &mut usize,
) -> bool {
    match msg {
        ScanMsg::Total(n) => {
            *scan_total = n;
            false
        }
        ScanMsg::Track(entry) => {
            if seen.insert(entry.path.to_string()) {
                track_model.push(entry);
            }
            *scan_done += 1;
            false
        }
        ScanMsg::Done => true,
    }
}

fn set_loading_done(weak: &slint::Weak<AppWindow>) {
    if let Some(window) = weak.upgrade() {
        let tls = window.global::<TrackListState>();
        tls.set_loading(false);
        tls.set_loading_text(SharedString::default());
    }
}
