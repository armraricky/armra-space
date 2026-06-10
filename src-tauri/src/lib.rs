mod auth;
mod commands;
mod db;
mod mount;
mod s3client;
mod sync;

use commands::AppState;
use mount::MountStatus;
use std::sync::Mutex;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, TrayIconBuilder, TrayIconEvent};
use tauri::{Manager, WindowEvent};
use tauri_plugin_deep_link::DeepLinkExt;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // Closing the window hides it instead of quitting — the menu-bar (tray)
        // item keeps ARMRA Space running so mounts stay live. Quit from the tray.
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(
            tauri_plugin_log::Builder::default()
                .level(log::LevelFilter::Info)
                .build(),
        )
        .setup(|app| {
            let data_dir = app
                .path()
                .app_data_dir()
                .expect("failed to get app data dir");

            std::fs::create_dir_all(&data_dir).expect("failed to create data dir");

            let db_path = data_dir.join("s3vault.db");
            let config_dir = data_dir.join("config");

            std::fs::create_dir_all(&config_dir).ok();

            // Load persisted cache settings + Quest base URL, falling back to defaults
            let (cache_dir, cache_max_mb, quest_base) = {
                if let Ok(conn) = db::open(&db_path) {
                    let path = db::get_config(&conn, "cache_path")
                        .ok()
                        .flatten()
                        .map(std::path::PathBuf::from)
                        .unwrap_or_else(|| data_dir.join("cache"));
                    let max_mb = db::get_config(&conn, "cache_max_mb")
                        .ok()
                        .flatten()
                        .and_then(|v| v.parse::<u64>().ok())
                        .unwrap_or(0);
                    let quest_base = db::get_config(&conn, "quest_base")
                        .ok()
                        .flatten()
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| "https://armra.quest".to_string());
                    (path, max_mb, quest_base)
                } else {
                    (data_dir.join("cache"), 0, "https://armra.quest".to_string())
                }
            };

            std::fs::create_dir_all(cache_dir.join("pins")).ok();

            app.manage(AppState {
                db_path,
                cache_dir: Mutex::new(cache_dir),
                cache_max_mb: Mutex::new(cache_max_mb),
                config_dir,
                s3_config: Mutex::new(None),
                active_filespace: Mutex::new(None),
                mounts: mount::new_mounts(),
                fs_configs: Mutex::new(std::collections::HashMap::new()),
                sync_progress: sync::new_progress(),
                quest_base,
                pending_login: Mutex::new(None),
            });

            // Handle armra-space:// deep-link callbacks (PKCE login completion).
            let handle = app.handle().clone();
            app.deep_link().on_open_url(move |event| {
                for url in event.urls() {
                    auth::handle_callback(&handle, url.as_str());
                }
            });
            // Best-effort runtime scheme registration (needed for dev / Linux / Windows;
            // macOS registers via the bundle Info.plist from tauri.conf.json).
            let _ = app.deep_link().register_all();

            // ── Menu-bar (tray) item ──────────────────────────────────────────
            // Keeps ARMRA Space accessible from the menu bar with live status
            // (mounted filespace + cache usage) and quick actions.
            let status_i = MenuItem::with_id(app, "status", "ARMRA Space", false, None::<&str>)?;
            let fs_i = MenuItem::with_id(app, "fs", "No filespace open", false, None::<&str>)?;
            let cache_i = MenuItem::with_id(app, "cache", "Cache: —", false, None::<&str>)?;
            let open_i = MenuItem::with_id(app, "open", "Open ARMRA Space", true, None::<&str>)?;
            let folder_i = MenuItem::with_id(app, "folder", "Open files folder", true, None::<&str>)?;
            let quit_i = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[
                &status_i,
                &PredefinedMenuItem::separator(app)?,
                &fs_i, &cache_i,
                &PredefinedMenuItem::separator(app)?,
                &open_i, &folder_i,
                &PredefinedMenuItem::separator(app)?,
                &quit_i,
            ])?;

            let _tray = TrayIconBuilder::with_id("armra-space-tray")
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("ARMRA Space")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "open" => {
                        if let Some(w) = app.get_webview_window("main") { let _ = w.show(); let _ = w.set_focus(); }
                    }
                    "folder" => {
                        let dir = mount::brand_base_dir();
                        let _ = std::fs::create_dir_all(&dir);
                        #[cfg(target_os = "macos")]
                        let _ = std::process::Command::new("open").arg(&dir).spawn();
                        #[cfg(target_os = "windows")]
                        let _ = std::process::Command::new("explorer").arg(&dir).spawn();
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click { button: MouseButton::Left, .. } = event {
                        if let Some(w) = tray.app_handle().get_webview_window("main") { let _ = w.show(); let _ = w.set_focus(); }
                    }
                })
                .build(app)?;

            // Refresh the tray status lines every 15s.
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    let st = handle.state::<AppState>();
                    let af = st.active_filespace.lock().unwrap().clone();
                    let cache_pins = st.cache_dir.lock().unwrap().join("pins");
                    let mounted_names: Vec<String> = {
                        let mounts = st.mounts.lock().await;
                        mounts.values().filter(|m| m.status == MountStatus::Mounted).map(|m| m.name.clone()).collect()
                    };
                    let fs_text = match mounted_names.len() {
                        0 => match af {
                            Some(a) => format!("○ {} — not mounted", a.name),
                            None => "No filespace open".to_string(),
                        },
                        1 => format!("● {} — mounted", mounted_names[0]),
                        n => format!("● {} filespaces mounted", n),
                    };
                    let used_mb = sync::disk_usage_bytes(cache_pins) as f64 / 1_048_576.0;
                    let _ = fs_i.set_text(fs_text);
                    let _ = cache_i.set_text(format!("Cache: {:.0} MB", used_mb));
                    tokio::time::sleep(std::time::Duration::from_secs(15)).await;
                }
            });

            // Create + brand the ~/ARMRA Space folder (green-Q icon). Filespaces
            // mount INSIDE it — macOS won't icon an NFS volume root, so this
            // branded local folder is the entry point users see in Finder.
            let base = mount::brand_base_dir();
            let _ = std::fs::create_dir_all(&base);
            if let Ok(icns) = app.path().resolve("icons/icon.icns", tauri::path::BaseDirectory::Resource) {
                mount::set_folder_icon(&base, &icns);
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::save_s3_config,
            commands::load_s3_config,
            commands::mount_bucket,
            commands::unmount_bucket,
            commands::get_mount_status,
            commands::list_files,
            commands::cached_listing,
            commands::pin_file,
            commands::unpin_file,
            commands::pin_folder,
            commands::unpin_folder,
            commands::list_pins,
            commands::start_sync,
            commands::get_sync_progress,
            commands::get_cache_config,
            commands::set_cache_config,
            commands::reveal_cache_dir,
            commands::open_in_finder,
            commands::reveal_mount_point,
            commands::refresh_files,
            commands::mount_transfer_stats,
            commands::macfuse_available,
            commands::open_url,
            commands::pick_folder,
            // ARMRA Quest auth + filespaces
            auth::begin_login,
            auth::current_session,
            auth::submit_pairing_code,
            auth::logout,
            commands::list_filespaces,
            commands::open_filespace,
            commands::get_active_filespace,
            commands::get_mounts,
            commands::unmount_filespace,
            commands::refresh_filespace,
            commands::reveal_logs,
        ])
        .build(tauri::generate_context!())
        .expect("error while building ARMRA Space")
        .run(|app_handle, event| {
            // Auto-eject: when the app is actually quitting (tray Quit, ⌘Q,
            // logout → exit), unmount any live filespace so we don't leave a
            // dangling NFS mount the OS has to reap. Window-close just hides
            // (handled above), so this only fires on a real exit.
            if let tauri::RunEvent::ExitRequested { .. } = event {
                let state = app_handle.state::<AppState>();
                tauri::async_runtime::block_on(async {
                    // Eject every mounted filespace.
                    let mut mounts = state.mounts.lock().await;
                    for (_id, ms) in mounts.iter_mut() {
                        if matches!(ms.status, MountStatus::Mounted) {
                            let _ = mount::kill_mount(ms).await;
                        }
                    }
                });
            }
        });
}
