mod auth;
mod commands;
mod db;
mod mount;
mod s3client;
mod sync;

use commands::AppState;
use std::sync::Mutex;
use tauri::Manager;
use tauri_plugin_deep_link::DeepLinkExt;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
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
                mount_state: mount::new_shared(),
                sync_progress: sync::new_progress(),
                quest_base,
                active_filespace: Mutex::new(None),
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
            commands::pin_file,
            commands::unpin_file,
            commands::list_pins,
            commands::start_sync,
            commands::get_sync_progress,
            commands::get_cache_config,
            commands::set_cache_config,
            commands::reveal_cache_dir,
            commands::open_in_finder,
            commands::reveal_mount_point,
            // ARMRA Quest auth + filespaces
            auth::begin_login,
            auth::current_session,
            auth::submit_pairing_code,
            auth::logout,
            commands::list_filespaces,
            commands::open_filespace,
            commands::get_active_filespace,
        ])
        .run(tauri::generate_context!())
        .expect("error while running ARMRA Space");
}
