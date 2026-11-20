pub mod audio;
pub mod commands;
pub mod model;
pub mod reconcile;
pub mod tray;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // `Manager` provides `app.state`, `window.app_handle`, `tray_by_id`.
    use tauri::Manager as _;
    tauri::Builder::default()
        .plugin(tauri_plugin_autostart::init(
            // Required by the signature; ignored on Linux (XDG autostart).
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .manage(commands::Manager::new())
        .setup(|app| {
            // The driver notifies the UI on engine connect/disconnect —
            // without this, connectivity changes never reach the frontend.
            app.state::<commands::Manager>()
                .set_app_handle(app.handle());
            // Tray is a nicety, not a load-bearing part of the app: any
            // failure (missing icon, no libappindicator, setup error)
            // must not abort startup — run windowed instead.
            if let Err(e) = tray::setup(app) {
                eprintln!("fio: system tray unavailable ({e}); running windowed");
            }
            Ok(())
        })
        // Fio is a background app: closing the window hides it to the
        // tray — only the tray menu's Quit actually exits. Without a
        // tray there is nothing to un-hide from, so let close be close.
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.app_handle().tray_by_id("fio").is_some() {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::ui_state,
            commands::create_device,
            commands::delete_device,
            commands::create_route,
            commands::delete_route,
            commands::set_default_mic,
            commands::restore_drift,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
