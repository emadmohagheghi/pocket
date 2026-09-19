mod commands;
mod error;
mod fsutil;
mod gaming;
mod hud;
mod models;
mod shortcuts;
mod storage;
mod tray;
mod updater;

use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::Mutex;

use tauri::http::header::{
    ACCESS_CONTROL_ALLOW_ORIGIN, CACHE_CONTROL, CONTENT_LENGTH, CONTENT_TYPE,
};
use tauri::{Manager, Runtime, UriSchemeContext};
use tauri_plugin_autostart::MacosLauncher;

use shortcuts::AppFlags;
use storage::Store;

pub fn run() {
    install_panic_logger();
    // A relaunch carrying the post-update marker comes from the self-update
    // installer/watcher: its window must ALWAYS end up visible (the user just
    // clicked "update"), even with start-minimized enabled.
    let post_update_launch = std::env::args()
        .any(|arg| arg == updater::POST_UPDATE_LAUNCH_MARKER);
    // Load the store BEFORE the builder starts creating windows: config
    // windows are built before setup() runs, and a fast webview (warm dev
    // server or cached assets) can invoke commands before setup finishes —
    // which used to panic with "state() called before manage()".
    let (data_dir, fallback) = resolve_data_dir();
    eprintln!(
        "[pocket] data directory: {} (post_update_launch={post_update_launch})",
        data_dir.display()
    );
    let store = Store::load(data_dir, fallback);
    let start_minimized = store.settings.start_minimized;
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            commands::show_main_window(app);
        }))
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            None,
        ))
        .register_uri_scheme_protocol("voice", voice_protocol)
        .register_uri_scheme_protocol("image", image_protocol)
        .manage(AppFlags {
            gaming: std::sync::atomic::AtomicBool::new(false),
            frontend_ready: std::sync::atomic::AtomicBool::new(false),
            show_requested: std::sync::atomic::AtomicBool::new(false),
        })
        .manage(Mutex::new(store))
        .setup(move |app| {
            let handle = app.handle().clone();

            // Restore the persisted always-on-top preference. Applied from
            // setup so the window is pinned before it is ever revealed.
            commands::apply_always_on_top(&handle);

            // The frontend normally reveals the initialized window. Keep a
            // backend fail-safe so a missed ready event cannot strand a normal
            // launch in the tray forever. A post-update relaunch always gets
            // the fail-safe so the freshly updated Pocket can never open as a
            // tray-only ghost.
            commands::schedule_startup_reveal(handle.clone(), start_minimized, post_update_launch);

            // Keep OS autostart in sync with the persisted preference.
            commands::apply_autostart(&handle);

            // Tray.
            tray::build_tray(&handle)?;

            // Fixed gestures: double-shift hook (text on tap, voice on hold).
            shortcuts::double_shift::spawn(handle.clone());
            // The HUD window must never intercept mouse input while it is
            // transparent-idle on screen.
            hud::init_hud(&handle);
            // Visual self-test hook for the capture HUD (POCKET_HUD_TEST=1).
            hud::spawn_self_test_if_requested(handle.clone());
            // Gaming mode is temporarily unavailable while its detector is
            // being stabilised. Keep the module and IPC surface intact so it
            // can be re-enabled without a data or API migration.
            if gaming::ENABLED {
                gaming::spawn(handle.clone());
            }

            // Voice notes use getUserMedia; WebView2 denies media permission
            // requests by default, so grant microphone access for our own
            // (offline-only) webviews.
            for label in ["main", "quick-capture"] {
                if let Some(win) = handle.get_webview_window(label) {
                    grant_microphone_permission(&win);
                    disable_browser_accelerators(&win);
                    disable_browser_autofill(&win);
                }
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    // Closing the main window always keeps Pocket running in
                    // the tray. Explicit Quit actions terminate the app.
                    api.prevent_close();
                    let _ = window.hide();
                } else if window.label() == "quick-capture"
                    || window.label() == "hud"
                    || window.label() == "image-viewer"
                {
                    // The capture window, the HUD and the image viewer always hide
                    // instead of quitting.
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_state,
            commands::frontend_ready,
            commands::get_items,
            commands::get_storage_info,
            commands::open_data_folder,
            commands::close_main_window,
            commands::export_backup,
            commands::import_backup,
            commands::create_workspace,
            commands::rename_workspace,
            commands::delete_workspace,
            commands::get_workspace_counts,
            commands::set_active_workspace,
            commands::create_item,
            commands::update_item,
            commands::delete_item,
            commands::move_item,
            commands::delete_entries_bulk,
            commands::set_pinned,
            commands::search,
            commands::save_recording,
            commands::save_image,
            commands::rename_recording,
            commands::delete_recording,
            commands::copy_to_clipboard,
            commands::update_settings,
            commands::mark_whats_new_seen,
            commands::get_gaming_state,
            commands::open_voice_capture,
            commands::open_url,
            commands::apply_undo,
            commands::frontend_log,
            updater::install_and_launch_update,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|_app_handle, _event| {
        // Graceful shutdown point; nothing to flush — all writes are
        // atomic and synchronous.
    });
}

/// The data directory lives next to the executable ("installed folder")
/// whenever that location is writable; otherwise we fall back to the
/// per-user app-config directory so nothing is ever lost. Resolved without
/// an app handle so the store can be managed before any window exists.
fn resolve_data_dir() -> (PathBuf, bool) {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()));
    if let Some(dir) = exe_dir {
        let candidate = dir.join("PocketData");
        if fsutil::is_writable(&candidate) {
            return (candidate, false);
        }
    }
    // Matches app.path().app_config_dir().join("data") for our identifier
    // (app.pocket.companion): %APPDATA%\app.pocket.companion\data.
    let fallback = std::env::var("APPDATA")
        .map(|d| PathBuf::from(d).join("app.pocket.companion").join("data"))
        .unwrap_or_else(|_| PathBuf::from("PocketData"));
    (fallback, true)
}

/// Panics vanish silently when the app runs detached from a console
/// (shortcut, tray, autostart). Keep the last ones on disk so crashes
/// like event-loop panics can actually be diagnosed afterwards.
fn install_panic_logger() {
    let path = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .map(|d| d.join("PocketData").join("panic.log"))
        .unwrap_or_else(|| std::env::temp_dir().join("pocket-panic.log"));
    std::panic::set_hook(Box::new(move |info| {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let payload = info.payload();
        let msg = payload
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown panic payload".into());
        let loc = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_default();
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            use std::io::Write;
            let _ = writeln!(f, "[{ts}] panic: {msg}\n  at {loc}");
        }
        eprintln!("[pocket] panic: {msg} at {loc}");
    }));
}

/// Disables WebView2's browser accelerator keys (Ctrl+N, Ctrl+F, Ctrl+P, …)
/// so in-app shortcuts like Ctrl+K reach the page instead of being swallowed
/// by the embedded browser layer.
#[cfg(windows)]
fn disable_browser_accelerators(window: &tauri::WebviewWindow) {
    use webview2_com::Microsoft::Web::WebView2::Win32::*;
    let _ = window.with_webview(move |webview| {
        let controller = webview.controller();
        let Ok(core) = (unsafe { controller.CoreWebView2() }) else {
            return;
        };
        let Ok(settings) = (unsafe { core.Settings() }) else {
            return;
        };
        let Ok(settings3) = windows::core::Interface::cast::<ICoreWebView2Settings3>(&settings)
        else {
            eprintln!("[pocket] ICoreWebView2Settings3 unavailable; accelerators untouched");
            return;
        };
        unsafe {
            let _ = settings3.SetAreBrowserAcceleratorKeysEnabled(false);
        }
        eprintln!("[pocket] browser accelerator keys disabled");
    });
}

#[cfg(not(windows))]
fn disable_browser_accelerators(_window: &tauri::WebviewWindow) {}

/// WebView2's general autofill pops a "Saved info" suggestions dropdown over
/// the capture/search inputs (fed by previously typed values). Pocket never
/// wants form autofill, anywhere.
#[cfg(windows)]
fn disable_browser_autofill(window: &tauri::WebviewWindow) {
    use webview2_com::Microsoft::Web::WebView2::Win32::*;
    let _ = window.with_webview(move |webview| {
        let controller = webview.controller();
        let Ok(core) = (unsafe { controller.CoreWebView2() }) else {
            return;
        };
        let Ok(settings) = (unsafe { core.Settings() }) else {
            return;
        };
        let Ok(settings9) = windows::core::Interface::cast::<ICoreWebView2Settings9>(&settings)
        else {
            eprintln!("[pocket] ICoreWebView2Settings9 unavailable; autofill untouched");
            return;
        };
        unsafe {
            let _ = settings9.SetIsGeneralAutofillEnabled(false);
        }
        eprintln!("[pocket] browser autofill disabled");
    });
}
#[cfg(not(windows))]
fn disable_browser_autofill(_window: &tauri::WebviewWindow) {}

/// Grants microphone access on the WebView2 layer so voice recording works
/// without a per-session permission prompt (WebView2 does not persist these).
#[cfg(windows)]
fn grant_microphone_permission(window: &tauri::WebviewWindow) {
    let _ = window.with_webview(move |webview| {
        use webview2_com::Microsoft::Web::WebView2::Win32::*;
        let controller = webview.controller();
        let Ok(core) = (unsafe { controller.CoreWebView2() }) else {
            return;
        };
        let handler =
            webview2_com::PermissionRequestedEventHandler::create(Box::new(|_core, args| {
                if let Some(args) = args {
                    unsafe {
                        let mut kind = COREWEBVIEW2_PERMISSION_KIND::default();
                        if args.PermissionKind(&mut kind).is_ok()
                            && kind == COREWEBVIEW2_PERMISSION_KIND_MICROPHONE
                        {
                            let _ = args.SetState(COREWEBVIEW2_PERMISSION_STATE_ALLOW);
                        }
                    }
                }
                Ok(())
            }));
        unsafe {
            let mut _token = 0i64;
            let _ = core.add_PermissionRequested(&handler, &mut _token);
        }
    });
}

#[cfg(not(windows))]
fn grant_microphone_permission(_window: &tauri::WebviewWindow) {}

/// Serves note images from `data_dir/images/<workspace>/<file>` over the
/// `image://` scheme (the voice:// equivalent for attached pictures). Only
/// strict, internally-generated paths with known image extensions are
/// accepted; any file inside the images directory is served, because freshly
/// staged images are legitimately requested *before* they are attached to a
/// note and appear in metadata. Path traversal is impossible by construction
/// (`valid_file_name` rejects separators, `valid_id` rejects odd workspaces).
fn image_protocol<R: Runtime>(
    ctx: UriSchemeContext<'_, R>,
    request: tauri::http::Request<Vec<u8>>,
) -> tauri::http::Response<Cow<'static, [u8]>> {
    let not_found = |msg: &'static str| {
        tauri::http::Response::builder()
            .status(404)
            .header(CONTENT_TYPE, "text/plain")
            .body(Cow::Borrowed(msg.as_bytes()))
            .unwrap()
    };

    let path = request.uri().path().trim_start_matches('/');
    let mut parts = path.split('/');
    let (Some(ws_id), Some(file), None) = (parts.next(), parts.next(), parts.next()) else {
        return not_found("not found");
    };
    if !fsutil::valid_id(ws_id) || !fsutil::valid_file_name(file) || !has_image_ext(file) {
        return not_found("not found");
    }

    let store = match ctx.app_handle().try_state::<Mutex<Store>>() {
        Some(s) => s,
        None => return not_found("not ready"),
    };
    let store = store.lock().unwrap();
    let full_path = store.image_path(ws_id, file);
    drop(store);

    let content_type = match file.rsplit('.').next().unwrap_or("").to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "avif" => "image/avif",
        _ => "application/octet-stream",
    };

    match std::fs::read(&full_path) {
        Ok(bytes) => {
            let len = bytes.len();
            tauri::http::Response::builder()
                .status(200)
                .header(CONTENT_TYPE, content_type)
                .header(CONTENT_LENGTH, len.to_string())
                .header(ACCESS_CONTROL_ALLOW_ORIGIN, "*")
                .header(CACHE_CONTROL, "no-store")
                .body(Cow::Owned(bytes))
                .unwrap()
        }
        Err(_) => not_found("not found"),
    }
}

fn has_image_ext(file: &str) -> bool {
    let lower = file.to_ascii_lowercase();
    [".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp", ".avif"]
        .iter()
        .any(|ext| lower.ends_with(ext))
}

/// Serves voice recordings from `data_dir/voices/<workspace>/<file>.webm`
/// over the `voice://` scheme. Only strict, internally-generated paths are
/// accepted, so nothing outside the voices directory can be read.
fn voice_protocol<R: Runtime>(
    ctx: UriSchemeContext<'_, R>,
    request: tauri::http::Request<Vec<u8>>,
) -> tauri::http::Response<Cow<'static, [u8]>> {
    let not_found = |msg: &'static str| {
        tauri::http::Response::builder()
            .status(404)
            .header(CONTENT_TYPE, "text/plain")
            .body(Cow::Borrowed(msg.as_bytes()))
            .unwrap()
    };

    let path = request.uri().path().trim_start_matches('/');
    let mut parts = path.split('/');
    let (Some(ws_id), Some(file), None) = (parts.next(), parts.next(), parts.next()) else {
        return not_found("not found");
    };
    if !fsutil::valid_id(ws_id) || !fsutil::valid_file_name(file) || !file.ends_with(".webm") {
        return not_found("not found");
    }

    let store = match ctx.app_handle().try_state::<Mutex<Store>>() {
        Some(s) => s,
        None => return not_found("not ready"),
    };
    let store = store.lock().unwrap();
    // Defense in depth: the recording must exist in workspace metadata —
    // either standalone in the feed or embedded in a note (image+voice
    // notes carry their recording on the item, not in the feed).
    let known = store.workspace_data(ws_id).map_or(false, |d| {
        d.recordings.iter().any(|r| r.file == file)
            || d.items
                .iter()
                .any(|i| i.recording.as_ref().is_some_and(|r| r.file == file))
    });
    if !known {
        return not_found("not found");
    }
    let full_path = store.recording_path(ws_id, file);
    drop(store);

    match std::fs::read(&full_path) {
        Ok(bytes) => {
            let len = bytes.len();
            tauri::http::Response::builder()
                .status(200)
                .header(CONTENT_TYPE, "audio/webm")
                .header(CONTENT_LENGTH, len.to_string())
                .header(ACCESS_CONTROL_ALLOW_ORIGIN, "*")
                .header(CACHE_CONTROL, "no-store")
                .body(Cow::Owned(bytes))
                .unwrap()
        }
        Err(_) => not_found("not found"),
    }
}
