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
/// Every failure returns `Err` — the caller decides whether the app can
/// live without a tray (it can: it runs windowed and window-close then
/// quits instead of hiding).
pub fn setup(app: &mut App) -> Result<(), Box<dyn std::error::Error>> {
    // On Linux the tray backend `dlopen`s libayatana-appindicator while
    // building the icon — and *panics* inside the sys crate when it is
    // absent, which `panic = "abort"` makes unrecoverable. Check the
    // linker cache first so a minimal-DE host gets a windowed app
    // instead of a dead one. Residual risk: a library present in the
    // cache but undlopenable at runtime can't be detected this way and
    // would still abort — that case needs a session bus / env probe we
    // don't have, and no distro hits it in practice.
    #[cfg(target_os = "linux")]
    if !appindicator_available() {
        return Err("no libayatana-appindicator on this host; tray skipped".into());
    }

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

    let icon = app
        .default_window_icon()
        .cloned()
        .ok_or("bundle icons are not configured")?;
    TrayIconBuilder::with_id("fio")
        .icon(icon)
        .tooltip("Fio: virtual audio patchbay")
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

/// Is an appindicator library findable by the dynamic linker? Reads the
/// `ldconfig` cache — same lookup `dlopen` performs. When the cache
/// can't be read at all (non-glibc, no ldconfig), err on the side of
/// trying: a normal distro is the common case, and the check exists to
/// protect the *detectably* unconfigured host.
#[cfg(target_os = "linux")]
fn appindicator_available() -> bool {
    match std::process::Command::new("ldconfig").arg("-p").output() {
        Ok(out) => {
            let cache = String::from_utf8_lossy(&out.stdout);
            cache.contains("libayatana-appindicator3.so")
                || cache.contains("libappindicator3.so")
        }
        Err(_) => true,
    }
}

/// "Open Fio": un-hide the main window and focus it.
fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}
