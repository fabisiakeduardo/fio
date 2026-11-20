//! System tray icon + menu — Fio's primary surface once the window is
//! hidden. The menu is built once at startup: the device list is
//! informational (disabled items, names current at launch) — devices
//! are managed from the window, and a live-rebuilt menu isn't worth it
//! for v1.

use tauri::{
    App, AppHandle, Manager,
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
};

/// `.setup()` hook: builds the tray menu and registers the tray icon.
pub fn setup(app: &mut App) -> Result<(), Box<dyn std::error::Error>> {
    let menu = Menu::new(app)?;
    menu.append(&MenuItem::with_id(
        app,
        "open",
        "Open Fio",
        true,
        None::<&str>,
    )?)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;

    let devices = app.state::<crate::commands::Manager>().desired().devices;
    if devices.is_empty() {
        menu.append(&MenuItem::with_id(
            app,
            "no-devices",
            "No virtual devices",
            false,
            None::<&str>,
        )?)?;
    } else {
        for device in devices {
            menu.append(&MenuItem::with_id(
                app,
                format!("device-{}", device.id),
                device.name,
                false,
                None::<&str>,
            )?)?;
        }
    }

    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?)?;

    TrayIconBuilder::with_id("fio")
        .icon(
            app.default_window_icon()
                .cloned()
                .expect("bundle icons are configured"),
        )
        .tooltip("Fio — virtual audio patchbay")
        .menu(&menu)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => show_main_window(app),
            "quit" => {
                // Quitting leaves the loaded PipeWire modules in place
                // on purpose: they die with the session anyway, so the
                // user's devices keep working — Fio just stops
                // enforcing routes. Nothing is unloaded here.
                app.exit(0);
            }
            _ => {}
        })
        .build(app)?;

    Ok(())
}

/// "Open Fio": un-hide the main window and focus it.
fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}
