pub mod audio;
pub mod commands;
pub mod model;
pub mod reconcile;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(commands::Manager::new())
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
