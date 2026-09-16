use std::sync::atomic::Ordering;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_opener::OpenerExt;

use crate::error::{AppError, AppResult};
use crate::fsutil;
use crate::models::*;
use crate::shortcuts::AppFlags;
use crate::storage::Store;

/// Snapshot emitted to the frontend whenever settings change.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StateChangedPayload {
    pub settings: Settings,
    pub workspaces: Vec<WorkspaceInfo>,
}

fn state_changed(app: &AppHandle) {
    let payload = {
        let store = app.state::<Mutex<Store>>();
        let store = store.lock().unwrap();
        StateChangedPayload {
            settings: store.settings.clone(),
            workspaces: store.all_workspace_infos(),
        }
    };
    let _ = app.emit("state-changed", payload);
}

fn items_changed(app: &AppHandle, ws_id: &str) {
    let payload = {
        let store = app.state::<Mutex<Store>>();
        let store = store.lock().unwrap();
        store.workspace_data(ws_id).cloned()
    };
    if let Ok(data) = payload {
        let _ = app.emit(
            "items-changed",
            serde_json::json!({ "workspaceId": ws_id, "data": data }),
        );
        // Sidebar counts live on the workspace infos; refresh them too.
        state_changed(app);
    }
}

fn show_main_window_now(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.set_focus();
    }
}

/// Explicit user requests (tray click, tray menu, or launching Pocket again)
/// must never depend on the frontend-ready handshake. Remember the request so
/// a startup-minimized window stays visible after initialization, then reveal
/// it immediately.
pub fn show_main_window(app: &AppHandle) {
    let shared = app.state::<AppFlags>();
    shared.show_requested.store(true, Ordering::Release);
    show_main_window_now(app);
}

/// The frontend normally reveals a standard launch as soon as its persisted
/// state is ready. Fail open if that IPC handshake never arrives so a broken or
/// unusually slow webview cannot leave Pocket permanently inaccessible in the
/// system tray.
pub fn schedule_startup_reveal(app: AppHandle, start_minimized: bool) {
    if start_minimized {
        return;
    }

    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(3));
        if app
            .state::<AppFlags>()
            .frontend_ready
            .load(Ordering::Acquire)
        {
            return;
        }

        let reveal_app = app.clone();
        let _ = app.run_on_main_thread(move || show_main_window_now(&reveal_app));
    });
}

/// Called once the main React view has loaded its persisted state. A normal
/// launch is revealed exactly once here; startup-minimized launches remain in
/// the tray unless the user explicitly requested the window while it loaded.
#[tauri::command]
pub fn frontend_ready(app: AppHandle) -> AppResult<()> {
    let shared = app.state::<AppFlags>();
    if shared.frontend_ready.swap(true, Ordering::AcqRel) {
        return Ok(());
    }
    let show_was_requested = shared.show_requested.swap(false, Ordering::AcqRel);
    let start_minimized = {
        let store = app.state::<Mutex<Store>>();
        let start_minimized = store.lock().unwrap().settings.start_minimized;
        start_minimized
    };

    if show_was_requested || !start_minimized {
        show_main_window_now(&app);
    }
    Ok(())
}

#[tauri::command]
pub fn get_state(app: AppHandle) -> AppResult<InitialState> {
    let store = app.state::<Mutex<Store>>();
    let store = store.lock().unwrap();
    Ok(InitialState {
        settings: store.settings.clone(),
        workspaces: store.all_workspace_infos(),
        storage: storage_info(&store),
    })
}

fn storage_info(store: &Store) -> StorageInfo {
    StorageInfo {
        data_dir: store.data_dir.display().to_string(),
        size_bytes: fsutil::dir_size(&store.data_dir),
        uses_fallback_location: store.uses_fallback_location,
    }
}

#[tauri::command]
pub fn get_storage_info(app: AppHandle) -> AppResult<StorageInfo> {
    let store = app.state::<Mutex<Store>>();
    let store = store.lock().unwrap();
    Ok(storage_info(&store))
}

#[tauri::command]
pub fn open_data_folder(app: AppHandle) -> AppResult<()> {
    let dir = {
        let store = app.state::<Mutex<Store>>();
        let store = store.lock().unwrap();
        store.data_dir.clone()
    };
    app.opener()
        .open_path(dir.to_string_lossy().to_string(), None::<&str>)
        .map_err(|e| AppError::Storage(format!("could not open data folder: {e}")))?;
    Ok(())
}

#[tauri::command]
pub fn close_main_window(app: AppHandle) -> AppResult<()> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| AppError::Invalid("main window is not available".into()))?;
    window
        .hide()
        .map_err(|e| AppError::Storage(format!("could not hide main window: {e}")))?;
    Ok(())
}

#[tauri::command]
pub async fn export_backup(app: AppHandle, path: String) -> AppResult<ExportSummary> {
    if path.trim().is_empty() {
        return Err(AppError::Invalid("export path cannot be empty".into()));
    }
    let destination = PathBuf::from(path);
    if destination.file_name().is_none() {
        return Err(AppError::Invalid(
            "export path must include a file name".into(),
        ));
    }

    let (prepared, summary) = {
        let store = app.state::<Mutex<Store>>();
        let store = store.lock().unwrap();
        store.prepare_backup(&destination)?
    };
    tauri::async_runtime::spawn_blocking(move || {
        Store::write_backup_archive(&destination, prepared)
    })
    .await
    .map_err(|e| AppError::Storage(format!("could not finish export: {e}")))??;
    Ok(summary)
}

#[tauri::command]
pub async fn import_backup(app: AppHandle, path: String) -> AppResult<ImportSummary> {
    if path.trim().is_empty() {
        return Err(AppError::Invalid("import path cannot be empty".into()));
    }
    let source = PathBuf::from(path);
    if source.file_name().is_none() {
        return Err(AppError::Invalid(
            "import path must include a file name".into(),
        ));
    }

    let import_app = app.clone();
    let summary = tauri::async_runtime::spawn_blocking(move || {
        let state = import_app.state::<Mutex<Store>>();
        let mut store = state.lock().unwrap();
        let mut merged = store.clone();
        let summary = merged.import_backup_archive(&source)?;
        *store = merged;
        Ok::<ImportSummary, AppError>(summary)
    })
    .await
    .map_err(|e| AppError::Storage(format!("could not finish import: {e}")))??;

    let active_id = {
        app.state::<Mutex<Store>>()
            .lock()
            .unwrap()
            .settings
            .active_workspace_id
            .clone()
    };
    items_changed(&app, &active_id);
    crate::tray::refresh_tray(&app);
    Ok(summary)
}

// ---------------------------------------------------------------- workspaces

#[tauri::command]
pub fn create_workspace(app: AppHandle, name: String) -> AppResult<WorkspaceInfo> {
    let info = {
        let store = app.state::<Mutex<Store>>();
        let mut store = store.lock().unwrap();
        store.create_workspace(&name)?
    };
    state_changed(&app);
    crate::tray::refresh_tray(&app);
    Ok(info)
}

#[tauri::command]
pub fn rename_workspace(
    app: AppHandle,
    workspace_id: String,
    name: String,
) -> AppResult<WorkspaceInfo> {
    let info = {
        let store = app.state::<Mutex<Store>>();
        let mut store = store.lock().unwrap();
        store.rename_workspace(&workspace_id, &name)?
    };
    state_changed(&app);
    crate::tray::refresh_tray(&app);
    Ok(info)
}

#[tauri::command]
pub fn get_workspace_counts(app: AppHandle, workspace_id: String) -> AppResult<Counts> {
    let store = app.state::<Mutex<Store>>();
    let store = store.lock().unwrap();
    store.counts(&workspace_id)
}

#[tauri::command]
pub fn delete_workspace(app: AppHandle, workspace_id: String) -> AppResult<()> {
    crate::shortcuts::diag_log(&format!("delete: command entered ws={workspace_id}"));
    {
        let store = app.state::<Mutex<Store>>();
        let mut store = store.lock().unwrap();
        crate::shortcuts::diag_log("delete: store lock acquired");
        store.delete_workspace(&workspace_id)?;
        crate::shortcuts::diag_log("delete: store.delete_workspace returned Ok");
    }
    crate::shortcuts::diag_log("delete: emitting state-changed");
    state_changed(&app);
    crate::shortcuts::diag_log("delete: refreshing tray");
    crate::tray::refresh_tray(&app);
    crate::shortcuts::diag_log("delete: emitting items-changed");
    // NOTE: bind the id to an owned String FIRST so the store guard is
    // dropped before items_changed() takes the lock again. Passing
    // `&...lock().unwrap().settings...` inline here used to hold the guard
    // across the call and deadlock the store mutex forever (the UI then sat
    // on "Deleting…" and every later command hung too).
    let active_id = {
        app.state::<Mutex<Store>>()
            .lock()
            .unwrap()
            .settings
            .active_workspace_id
            .clone()
    };
    items_changed(&app, &active_id);
    crate::shortcuts::diag_log("delete: command returning Ok");
    Ok(())
}

/// Internal helper used by the tray menu (not exposed as a command).
pub fn set_active_workspace_internal(app: &AppHandle, workspace_id: &str) -> AppResult<()> {
    {
        let store = app.state::<Mutex<Store>>();
        let mut store = store.lock().unwrap();
        store.set_active_workspace(workspace_id)?;
    }
    state_changed(app);
    Ok(())
}

#[tauri::command]
pub fn set_active_workspace(app: AppHandle, workspace_id: String) -> AppResult<()> {
    set_active_workspace_internal(&app, &workspace_id)?;
    crate::tray::refresh_tray(&app);
    Ok(())
}

// --------------------------------------------------------------------- items

#[tauri::command]
pub fn get_items(app: AppHandle, workspace_id: String) -> AppResult<WorkspaceData> {
    let store = app.state::<Mutex<Store>>();
    let store = store.lock().unwrap();
    Ok(store.workspace_data(&workspace_id)?.clone())
}

#[tauri::command]
pub fn create_item(app: AppHandle, workspace_id: String, item: NewItem) -> AppResult<Item> {
    crate::shortcuts::debug_log(&format!(
        "create_item ws={workspace_id} type={:?} content_len={}",
        item.item_type,
        item.content.len()
    ));
    let created = {
        let store = app.state::<Mutex<Store>>();
        let mut store = store.lock().unwrap();
        store.create_item(&workspace_id, item)
    };
    match &created {
        Ok(i) => crate::shortcuts::debug_log(&format!("create_item ok id={}", i.id)),
        Err(e) => crate::shortcuts::debug_log(&format!("create_item FAILED: {e}")),
    }
    items_changed(&app, &workspace_id);
    created
}

/// Double-Shift tap path: save the grabbed selection straight into the active
/// workspace without opening any window. Refreshes main-window listeners the
/// same way `create_item` does. Returns `Ok(None)` when the capture was
/// skipped because it duplicates the newest item (rapid double captures of
/// the same selection should not stack copies).
pub fn save_hotkey_text_capture(app: &AppHandle, text: String) -> AppResult<Option<Item>> {
    let trimmed = text.trim().to_string();
    let (ws_id, duplicate_of_latest) = {
        let store = app.state::<Mutex<Store>>();
        let store = store.lock().unwrap();
        let ws_id = store.settings.active_workspace_id.clone();
        let duplicate_of_latest = store
            .workspace_data(&ws_id)
            .ok()
            .and_then(|data| data.items.iter().max_by_key(|i| i.created_at))
            .map(|latest| latest.content.trim() == trimmed)
            .unwrap_or(false);
        (ws_id, duplicate_of_latest)
    };
    if duplicate_of_latest {
        crate::shortcuts::debug_log("hotkey text capture skipped: duplicate of latest item");
        return Ok(None);
    }
    let created = {
        let store = app.state::<Mutex<Store>>();
        let mut store = store.lock().unwrap();
        store.create_item(
            &ws_id,
            NewItem {
                item_type: ItemType::Text,
                content: trimmed,
                title: None,
                url: None,
            },
        )?
    };
    crate::shortcuts::debug_log(&format!(
        "hotkey text capture saved id={} ws={ws_id}",
        created.id
    ));
    items_changed(app, &ws_id);
    Ok(Some(created))
}

// ----------------------------------------------------------------------- undo

/// One reversible primitive sent by the frontend's Ctrl+Z. `restore*` upserts
/// the entity back with its original id/timestamps; `remove*` deletes it.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum UndoStep {
    RestoreItem {
        workspace_id: String,
        item: Item,
    },
    RestoreRecording {
        workspace_id: String,
        recording: Recording,
    },
    RemoveItem {
        workspace_id: String,
        item_id: String,
    },
    RemoveRecording {
        workspace_id: String,
        recording_id: String,
    },
}

/// Apply a whole recorded action (possibly several primitives across
/// workspaces) and notify listeners for every touched workspace.
#[tauri::command]
pub fn apply_undo(app: AppHandle, steps: Vec<UndoStep>) -> AppResult<()> {
    let mut changed: Vec<String> = Vec::new();
    {
        let store = app.state::<Mutex<Store>>();
        let mut store = store.lock().unwrap();
        for step in steps {
            let ws_id = match &step {
                UndoStep::RestoreItem { workspace_id, .. } => workspace_id,
                UndoStep::RestoreRecording { workspace_id, .. } => workspace_id,
                UndoStep::RemoveItem { workspace_id, .. } => workspace_id,
                UndoStep::RemoveRecording { workspace_id, .. } => workspace_id,
            };
            let ws_id = ws_id.clone();
            match step {
                UndoStep::RestoreItem { workspace_id, item } => {
                    store.restore_item(&workspace_id, item)?
                }
                UndoStep::RestoreRecording {
                    workspace_id,
                    recording,
                } => store.restore_recording(&workspace_id, recording)?,
                UndoStep::RemoveItem {
                    workspace_id,
                    item_id,
                } => store.delete_item(&workspace_id, &item_id)?,
                UndoStep::RemoveRecording {
                    workspace_id,
                    recording_id,
                } => store.delete_recording(&workspace_id, &recording_id)?,
            }
            if !changed.contains(&ws_id) {
                changed.push(ws_id);
            }
        }
    }
    for ws_id in changed {
        items_changed(&app, &ws_id);
    }
    Ok(())
}

#[tauri::command]
pub fn update_item(
    app: AppHandle,
    workspace_id: String,
    item_id: String,
    patch: ItemPatch,
) -> AppResult<Item> {
    let updated = {
        let store = app.state::<Mutex<Store>>();
        let mut store = store.lock().unwrap();
        store.update_item(&workspace_id, &item_id, patch)?
    };
    items_changed(&app, &workspace_id);
    Ok(updated)
}

#[tauri::command]
pub fn delete_item(app: AppHandle, workspace_id: String, item_id: String) -> AppResult<()> {
    {
        let store = app.state::<Mutex<Store>>();
        let mut store = store.lock().unwrap();
        store.delete_item(&workspace_id, &item_id)?;
    }
    items_changed(&app, &workspace_id);
    Ok(())
}

/// Bulk delete (Ctrl+A + Delete): one IPC call, one persist, one
/// items-changed broadcast — deleting hundreds of entries must feel instant.
#[tauri::command]
pub fn delete_entries_bulk(
    app: AppHandle,
    workspace_id: String,
    item_ids: Vec<String>,
    recording_ids: Vec<String>,
) -> AppResult<usize> {
    let count = {
        let store = app.state::<Mutex<Store>>();
        let mut store = store.lock().unwrap();
        store.delete_entries_bulk(&workspace_id, &item_ids, &recording_ids)?
    };
    items_changed(&app, &workspace_id);
    Ok(count)
}

#[tauri::command]
pub fn set_pinned(
    app: AppHandle,
    workspace_id: String,
    kind: String,
    entry_id: String,
    pinned: bool,
) -> AppResult<()> {
    {
        let store = app.state::<Mutex<Store>>();
        let mut store = store.lock().unwrap();
        store.set_pinned(&workspace_id, &kind, &entry_id, pinned)?;
    }
    items_changed(&app, &workspace_id);
    Ok(())
}

#[tauri::command]
pub fn search(app: AppHandle, workspace_id: String, query: String) -> AppResult<Vec<SearchHit>> {
    let store = app.state::<Mutex<Store>>();
    let store = store.lock().unwrap();
    store.search(&workspace_id, &query)
}

// ---------------------------------------------------------------- recordings

#[tauri::command]
pub fn save_recording(app: AppHandle, request: tauri::ipc::Request) -> AppResult<Recording> {
    // Audio bytes travel as the raw IPC body; metadata rides in headers.
    let headers = request.headers();
    let header_str = |key: &str| -> Option<String> {
        headers
            .get(key)
            .and_then(|v| v.to_str().ok())
            .map(|v| percent_decode(v.to_string()))
    };
    let workspace_id = header_str("x-pocket-workspace")
        .ok_or_else(|| AppError::Invalid("missing workspace".into()))?;
    let name = header_str("x-pocket-name").unwrap_or_else(|| "Voice note".into());
    let duration_ms: u64 = header_str("x-pocket-duration")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);

    let bytes: Vec<u8> = match request.body() {
        tauri::ipc::InvokeBody::Raw(b) => b.clone(),
        // When the webview falls back to postMessage IPC, binary payloads
        // arrive JSON-serialized as a number array. Accept both.
        tauri::ipc::InvokeBody::Json(v) => {
            let arr = v
                .as_array()
                .ok_or_else(|| AppError::Recording("expected raw audio body".into()))?;
            if arr.len() > 100 * 1024 * 1024 {
                return Err(AppError::Recording("recording too large".into()));
            }
            let mut bytes = Vec::with_capacity(arr.len());
            for n in arr {
                let b = u8::try_from(n.as_u64().unwrap_or(256))
                    .map_err(|_| AppError::Recording("invalid audio byte".into()))?;
                bytes.push(b);
            }
            crate::shortcuts::debug_log(&format!(
                "save_recording: body arrived as JSON array ({} bytes) — postMessage IPC fallback active",
                bytes.len()
            ));
            bytes
        }
    };
    crate::shortcuts::debug_log(&format!(
        "save_recording ws={workspace_id} name='{name}' duration={duration_ms}ms bytes={}",
        bytes.len()
    ));
    let result = {
        let store = app.state::<Mutex<Store>>();
        let mut store = store.lock().unwrap();
        store.save_recording(&workspace_id, &name, duration_ms, &bytes)
    };
    match &result {
        Ok(rec) => {
            let path = {
                let store = app.state::<Mutex<Store>>();
                let store = store.lock().unwrap();
                store.recording_path(&workspace_id, &rec.file)
            };
            crate::shortcuts::debug_log(&format!(
                "save_recording ok -> {} ({} bytes on disk: {})",
                path.display(),
                std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0),
                path.exists()
            ));
        }
        Err(e) => crate::shortcuts::debug_log(&format!("save_recording FAILED: {e}")),
    }
    let rec = result?;
    items_changed(&app, &workspace_id);
    Ok(rec)
}

/// Frontend diagnostics channel — routed into the timestamped diag file so
/// release builds keep frontend evidence without POCKET_DEBUG.
#[tauri::command]
pub fn frontend_log(message: String) {
    crate::shortcuts::diag_log(&format!("[web] {message}"));
}

fn percent_decode(input: String) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(&input[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        if bytes[i] == b'+' {
            out.push(b' ');
            i += 1;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

#[tauri::command]
pub fn rename_recording(
    app: AppHandle,
    workspace_id: String,
    recording_id: String,
    name: String,
) -> AppResult<Recording> {
    let rec = {
        let store = app.state::<Mutex<Store>>();
        let mut store = store.lock().unwrap();
        store.rename_recording(&workspace_id, &recording_id, &name)?
    };
    items_changed(&app, &workspace_id);
    Ok(rec)
}

#[tauri::command]
pub fn delete_recording(
    app: AppHandle,
    workspace_id: String,
    recording_id: String,
) -> AppResult<()> {
    {
        let store = app.state::<Mutex<Store>>();
        let mut store = store.lock().unwrap();
        store.delete_recording(&workspace_id, &recording_id)?;
    }
    items_changed(&app, &workspace_id);
    Ok(())
}

// ----------------------------------------------------------------- clipboard

#[tauri::command]
pub fn copy_to_clipboard(app: AppHandle, text: String) -> AppResult<()> {
    app.clipboard()
        .write_text(text)
        .map_err(|e| AppError::Invalid(format!("could not copy: {e}")))?;
    Ok(())
}

// ------------------------------------------------------------------ settings

#[tauri::command]
pub fn update_settings(app: AppHandle, patch: SettingsPatch) -> AppResult<Settings> {
    let settings = {
        let store = app.state::<Mutex<Store>>();
        let mut guard = store.lock().unwrap();
        let s = &mut guard.settings;
        if let Some(v) = patch.launch_on_startup {
            if s.launch_on_startup != v {
                s.launch_on_startup = v;
            }
        }
        if let Some(v) = patch.start_minimized {
            s.start_minimized = v;
        }
        if let Some(v) = patch.gaming_detection_enabled {
            // Keep accepting the field for API compatibility, but do not let
            // clients enable the feature while its runtime gate is disabled.
            s.gaming_detection_enabled = v && crate::gaming::ENABLED;
        }
        if let Some(v) = patch.always_on_top {
            s.always_on_top = v;
        }
        if let Some(v) = patch.theme {
            if matches!(v.as_str(), "system" | "light" | "dark") {
                s.theme = v;
            } else {
                return Err(AppError::Invalid(
                    "theme must be system, light or dark".into(),
                ));
            }
        }
        if let Some(v) = patch.note_preview_lines {
            if v <= 6 {
                s.note_preview_lines = v;
            } else {
                return Err(AppError::Invalid(
                    "note preview lines must be between 0 and 6".into(),
                ));
            }
        }
        let settings = s.clone();
        guard.persist_settings();
        settings
    };
    if patch.launch_on_startup.is_some() {
        apply_autostart(&app);
    }
    if patch.always_on_top.is_some() {
        apply_always_on_top(&app);
    }
    state_changed(&app);
    Ok(settings)
}

/// Sync the main window's always-on-top state with the persisted setting.
pub fn apply_always_on_top(app: &AppHandle) {
    let enabled = {
        let store = app.state::<Mutex<Store>>();
        let guard = store.lock().unwrap();
        guard.settings.always_on_top
    };
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.set_always_on_top(enabled);
    }
}

/// Sync the OS autostart registration with the persisted setting.
pub fn apply_autostart(app: &AppHandle) {
    use tauri_plugin_autostart::ManagerExt;
    let enabled = {
        let store = app.state::<Mutex<Store>>();
        let guard = store.lock().unwrap();
        guard.settings.launch_on_startup
    };
    let autolaunch = app.autolaunch();
    let is_enabled = autolaunch.is_enabled().unwrap_or(false);
    let result = if enabled && !is_enabled {
        autolaunch.enable()
    } else if !enabled && is_enabled {
        autolaunch.disable()
    } else {
        Ok(())
    };
    if let Err(e) = result {
        eprintln!("[pocket] failed to update autostart: {e}");
    }
}

#[tauri::command]
pub fn get_gaming_state(app: AppHandle) -> AppResult<bool> {
    let shared = app.state::<AppFlags>();
    Ok(shared.gaming.load(Ordering::Relaxed))
}

/// Opens the voice-capture panel from the main window.
#[tauri::command]
pub fn open_voice_capture(app: AppHandle) -> AppResult<()> {
    crate::shortcuts::show_voice_capture(&app);
    Ok(())
}

/// Opens a captured link in the default browser.
#[tauri::command]
pub fn open_url(app: AppHandle, url: String) -> AppResult<()> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(AppError::Invalid("only http(s) links can be opened".into()));
    }
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| AppError::Invalid(format!("could not open link: {e}")))?;
    Ok(())
}
