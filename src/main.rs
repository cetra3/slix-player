mod app;
mod audio;
mod db;
mod mpris;
mod scan;
mod state;
mod track_list;
mod utils;
mod waveform;

use anyhow::Result;
use app::App;
use db::{PlayerState, TrackDatabase};
use track_list::TrackListController;
use std::rc::Rc;

slint::include_modules!();

fn main() -> Result<()> {
    let backend = i_slint_backend_winit::Backend::builder()
        .with_window_attributes_hook(|attrs| {
            let attrs =
                attrs.with_theme(Some(i_slint_backend_winit::winit::window::Theme::Dark));
            #[cfg(target_os = "linux")]
            let attrs = {
                use i_slint_backend_winit::winit::platform::wayland::WindowAttributesExtWayland;
                attrs.with_name("slix-player", "")
            };
            attrs
        })
        .build()?;
    slint::platform::set_platform(Box::new(backend))?;

    let db_dir = dirs::state_dir()
        .or_else(|| dirs::data_local_dir())
        .expect("could not determine data directory")
        .join("slix-player");

    std::fs::create_dir_all(&db_dir)?;

    eprintln!("reading db from {}", db_dir.display());

    let db = std::sync::Arc::new(TrackDatabase::open(db_dir.join("slix-player.db"))?);

    let saved_state = db
        .get_player_state()
        .ok()
        .flatten()
        .unwrap_or(PlayerState::default());

    let window = AppWindow::new()?;

    let track_list = TrackListController::new(&window);
    track_list.register_callbacks();

    let stream = rodio::DeviceSinkBuilder::open_default_sink()
        .map_err(|e| anyhow::anyhow!("Failed to open audio output: {e}"))?;
    let sink = Rc::new(rodio::Player::connect_new(stream.mixer()));
    sink.pause();
    sink.set_volume(saved_state.volume);
    window
        .global::<PlayState>()
        .set_volume(saved_state.volume);

    let last_folder = saved_state.last_folder.clone().unwrap_or_default();
    let app = App::build(&window, track_list, sink, db, last_folder)?;
    app.register_all();
    app.restore_state(&saved_state);

    window.run()?;

    app.save_state();
    let _ = app.db.persist();

    Ok(())
}
