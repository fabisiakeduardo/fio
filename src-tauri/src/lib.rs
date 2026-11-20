pub mod audio;
pub mod commands;
pub mod model;
pub mod reconcile;
pub mod tray;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_autostart::init(
            // Required by the signature; ignored on Linux (XDG autostart).
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .manage(commands::Manager::new())
        .setup(tray::setup)
        // Fio is a background app: closing the window hides it to the
        // tray — only the tray menu's Quit actually exits.
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
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
