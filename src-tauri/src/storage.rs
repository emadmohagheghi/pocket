use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{Read, Seek};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::error::{AppError, AppResult};
use crate::fsutil;
use crate::models::*;

const BACKUP_FORMAT_VERSION: u32 = 3;
const BACKUP_MANIFEST_NAME: &str = "backup.json";
const MAX_BACKUP_MANIFEST_BYTES: u64 = 50 * 1024 * 1024;
const MAX_RECORDING_BYTES: u64 = 100 * 1024 * 1024;
const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BackupDocument {
    format_version: u32,
    scope: String,
    voice_files: String,
    /// Present from backup format v3 (note images). Absent in v2 archives.
    #[serde(default)]
    image_files: String,
    exported_at: i64,
    app_version: String,
    settings: Settings,
    workspaces: Vec<BackupWorkspace>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BackupWorkspace {
    #[serde(flatten)]
    meta: WorkspaceMeta,
    items: Vec<BackupItem>,
    recordings: Vec<BackupRecording>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BackupItem {
    #[serde(flatten)]
    item: Item,
    /// Every attached image gets an entry here; `archive_path` is `None` when
    /// the image file was missing on disk at export time.
    images: Vec<BackupImage>,
    /// Archive path of the note's embedded voice recording, when one exists
    /// and its audio file was present at export time. The recording metadata
    /// itself rides inside the flattened item; this only carries the file.
    #[serde(default)]
    recording_file: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BackupImage {
    image: ItemImage,
    archive_path: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BackupRecording {
    #[serde(flatten)]
    recording: Recording,
    archive_path: Option<String>,
}

pub(crate) struct PreparedBackup {
    document: BackupDocument,
    audio_files: Vec<(PathBuf, String)>,
    image_files: Vec<(PathBuf, String)>,
}

#[derive(Clone)]
pub struct Store {
    pub data_dir: PathBuf,
    pub uses_fallback_location: bool,
    pub settings: Settings,
    pub workspaces: Vec<WorkspaceMeta>,
    pub data: HashMap<String, WorkspaceData>,
}

impl Store {
    // ---------------------------------------------------------------- layout

    pub fn settings_path(&self) -> PathBuf {
        self.data_dir.join("settings.json")
    }
    pub fn index_path(&self) -> PathBuf {
        self.data_dir.join("workspaces.json")
    }
    fn workspace_dir(&self, ws_id: &str) -> PathBuf {
        self.data_dir.join("workspaces").join(ws_id)
    }
    fn workspace_path(&self, ws_id: &str) -> PathBuf {
        self.workspace_dir(ws_id).join("workspace.json")
    }
    pub fn voices_dir(&self, ws_id: &str) -> PathBuf {
        self.data_dir.join("voices").join(ws_id)
    }
    pub fn recording_path(&self, ws_id: &str, file: &str) -> PathBuf {
        self.voices_dir(ws_id).join(file)
    }
    pub fn images_dir(&self, ws_id: &str) -> PathBuf {
        self.data_dir.join("images").join(ws_id)
    }
    pub fn image_path(&self, ws_id: &str, file: &str) -> PathBuf {
        self.images_dir(ws_id).join(file)
    }
    /// Deleted images are parked here (not erased) so Ctrl+Z can put them
    /// back. Cleared on startup, same as the voice trash.
    fn image_trash_dir(&self) -> PathBuf {
        self.data_dir.join("images").join(".trash")
    }
    /// Deleted recordings are parked here (not erased) so Ctrl+Z can put them
    /// back. Cleared on startup: undo history is in-memory only and never
    /// survives a restart.
    fn trash_dir(&self) -> PathBuf {
        self.data_dir.join("voices").join(".trash")
    }

    // ------------------------------------------------------------------ load

    pub fn load(data_dir: PathBuf, uses_fallback_location: bool) -> Store {
        fs::create_dir_all(&data_dir).ok();
        // Undo never survives a restart, so nothing parked for undo is needed.
        let _ = fs::remove_dir_all(data_dir.join("voices").join(".trash"));
        let _ = fs::remove_dir_all(data_dir.join("images").join(".trash"));

        // Settings: fall back to defaults on missing/corrupt file (keep the
        // corrupt file around for recovery).
        let mut settings = match fsutil::read_json::<Settings>(&data_dir.join("settings.json")) {
            Ok(s) => s,
            Err(_) => {
                backup_corrupt(&data_dir.join("settings.json"));
                Settings::default()
            }
        };
        // Gaming mode is temporarily feature-gated off. Normalise existing
        // installations as well as fresh ones so the persisted state and UI
        // cannot suggest that the detector is active.
        let gaming_was_enabled = settings.gaming_detection_enabled;
        settings.gaming_detection_enabled = false;

        let workspaces: Vec<WorkspaceMeta> =
            match fsutil::read_json(&data_dir.join("workspaces.json")) {
                Ok(w) => w,
                Err(_) => {
                    backup_corrupt(&data_dir.join("workspaces.json"));
                    Vec::new()
                }
            };

        let mut data = HashMap::new();
        for ws in &workspaces {
            if !fsutil::valid_id(&ws.id) {
                continue;
            }
            let path = self_path(&data_dir, &ws.id);
            let ws_data = match fsutil::read_json::<WorkspaceData>(&path) {
                Ok(d) => d,
                Err(_) => {
                    backup_corrupt(&path);
                    WorkspaceData::default()
                }
            };
            data.insert(ws.id.clone(), ws_data);
        }

        // Images have no feed of their own (unlike recordings), so files can
        // survive their metadata after crashes or staging that never became a
        // note. Sweep orphans so disk cannot grow silently.
        {
            let referenced: HashSet<String> = data
                .values()
                .flat_map(|d| d.items.iter().flat_map(|i| i.images.iter().map(|img| img.file.clone())))
                .collect();
            if let Ok(workspaces_root) = fs::read_dir(data_dir.join("images")) {
                for entry in workspaces_root.flatten() {
                    let entry_path = entry.path();
                    if !entry_path.is_dir()
                        || entry.file_name().to_str() == Some(".trash")
                    {
                        // Files at the root are stray temp writes; the undo
                        // trash (if present) must survive untouched.
                        if entry_path.is_file() {
                            let _ = fs::remove_file(&entry_path);
                        }
                        continue;
                    }
                    if let Ok(files) = fs::read_dir(&entry_path) {
                        for file in files.flatten() {
                            let fp = file.path();
                            if fp.is_file()
                                && !file
                                    .file_name()
                                    .to_str()
                                    .map(|name| referenced.contains(name))
                                    .unwrap_or(false)
                            {
                                let _ = fs::remove_file(&fp);
                            }
                        }
                    }
                }
            }
        }

        let mut store = Store {
            data_dir,
            uses_fallback_location,
            settings,
            workspaces,
            data,
        };
        if gaming_was_enabled {
            store.persist_settings();
        }
        if store.workspaces.is_empty() {
            let ws = WorkspaceMeta {
                id: Uuid::new_v4().to_string(),
                name: "Personal".into(),
                created_at: now_ms(),
                updated_at: now_ms(),
            };
            store.workspaces.push(ws.clone());
            store.data.insert(ws.id.clone(), WorkspaceData::default());
            store.settings.active_workspace_id = ws.id.clone();
            store.persist_index();
            store.persist_workspace(&ws.id);
            // Persist the complete first-run defaults as part of the same
            // initialization, including startMinimized=false.
            store.persist_settings();
        }
        if !store
            .workspaces
            .iter()
            .any(|w| w.id == store.settings.active_workspace_id)
        {
            store.settings.active_workspace_id = store.workspaces[0].id.clone();
            store.persist_settings();
        }
        store
    }

    // ---------------------------------------------------------------- persist

    pub fn persist_settings(&self) {
        if let Err(e) = fsutil::write_json_atomic(&self.settings_path(), &self.settings) {
            eprintln!("[pocket] failed to persist settings: {e}");
        }
    }

    pub fn persist_index(&self) {
        if let Err(e) = fsutil::write_json_atomic(&self.index_path(), &self.workspaces) {
            eprintln!("[pocket] failed to persist workspace index: {e}");
        }
    }

    pub fn persist_workspace(&self, ws_id: &str) {
        let data = self.data.get(ws_id).cloned().unwrap_or_default();
        if let Err(e) = fsutil::write_json_atomic(&self.workspace_path(ws_id), &data) {
            eprintln!("[pocket] failed to persist workspace {ws_id}: {e}");
        }
    }

    // ------------------------------------------------------------- workspaces

    pub fn workspace(&self, ws_id: &str) -> AppResult<&WorkspaceMeta> {
        self.workspaces
            .iter()
            .find(|w| w.id == ws_id)
            .ok_or(AppError::WorkspaceNotFound)
    }

    pub fn workspace_data(&self, ws_id: &str) -> AppResult<&WorkspaceData> {
        if !fsutil::valid_id(ws_id) {
            return Err(AppError::Invalid("invalid workspace id".into()));
        }
        self.data.get(ws_id).ok_or(AppError::WorkspaceNotFound)
    }

    pub fn workspace_data_mut(&mut self, ws_id: &str) -> AppResult<&mut WorkspaceData> {
        if !fsutil::valid_id(ws_id) {
            return Err(AppError::Invalid("invalid workspace id".into()));
        }
        self.touch_workspace(ws_id);
        self.data.get_mut(ws_id).ok_or(AppError::WorkspaceNotFound)
    }

    fn touch_workspace(&mut self, ws_id: &str) {
        if let Some(ws) = self.workspaces.iter_mut().find(|w| w.id == ws_id) {
            ws.updated_at = now_ms();
        }
    }

    pub fn counts(&self, ws_id: &str) -> AppResult<Counts> {
        let data = self.workspace_data(ws_id)?;
        Ok(Counts {
            texts: data.items.len(),
            recordings: data.recordings.len(),
        })
    }

    pub fn workspace_info(&self, ws_id: &str) -> AppResult<WorkspaceInfo> {
        Ok(WorkspaceInfo {
            meta: self.workspace(ws_id)?.clone(),
            counts: self.counts(ws_id)?,
        })
    }

    pub fn all_workspace_infos(&self) -> Vec<WorkspaceInfo> {
        self.workspaces
            .iter()
            .filter_map(|w| self.workspace_info(&w.id).ok())
            .collect()
    }

    pub(crate) fn prepare_backup(
        &self,
        destination: &std::path::Path,
    ) -> AppResult<(PreparedBackup, ExportSummary)> {
        let mut item_count = 0;
        let mut recording_count = 0;
        let mut missing_audio = 0;
        let mut missing_images = 0;
        let mut audio_files = Vec::new();
        let mut image_files = Vec::new();
        let mut workspaces = Vec::with_capacity(self.workspaces.len());

        for meta in &self.workspaces {
            let data = self.workspace_data(&meta.id)?;
            item_count += data.items.len();
            recording_count += data.recordings.len();

            let recordings = data
                .recordings
                .iter()
                .cloned()
                .map(|recording| {
                    let stored_path = self.recording_path(&meta.id, &recording.file);
                    let archive_path = if stored_path.is_file() {
                        let archive_path = backup_audio_path(&meta.id, &recording.file);
                        audio_files.push((stored_path, archive_path.clone()));
                        Some(archive_path)
                    } else {
                        missing_audio += 1;
                        None
                    };                    BackupRecording {
                        recording,
                        archive_path,
                    }
                })
                .collect();

            let items = data
                .items
                .iter()
                .cloned()
                .map(|item| {
                    let images = item
                        .images
                        .iter()
                        .map(|image| {
                            let stored_path = self.image_path(&meta.id, &image.file);
                            let archive_path = if stored_path.is_file() {
                                let archive_path = backup_image_path(&meta.id, &image.file);
                                image_files.push((stored_path, archive_path.clone()));
                                Some(archive_path)
                            } else {
                                missing_images += 1;
                                None
                            };
                            BackupImage {
                                image: image.clone(),
                                archive_path,
                            }
                        })
                        .collect();
                    // A note-embedded voice recording's audio is embedded like
                    // any standalone recording's.
                    let recording_file = match &item.recording {
                        Some(rec) => {
                            let stored_path = self.recording_path(&meta.id, &rec.file);
                            if stored_path.is_file() {
                                let archive_path = backup_audio_path(&meta.id, &rec.file);
                                audio_files.push((stored_path, archive_path.clone()));
                                Some(archive_path)
                            } else {
                                missing_audio += 1;
                                None
                            }
                        }
                        None => None,
                    };
                    BackupItem {
                        item,
                        images,
                        recording_file,
                    }
                })
                .collect();

            workspaces.push(BackupWorkspace {
                meta: meta.clone(),
                items,
                recordings,
            });
        }

        let summary = ExportSummary {
            path: destination.to_string_lossy().into_owned(),
            workspaces: workspaces.len(),
            items: item_count,
            recordings: recording_count,
            audio_files: audio_files.len(),
            missing_audio,
            images: image_files.len(),
            missing_images,
        };
        let document = BackupDocument {
            format_version: BACKUP_FORMAT_VERSION,
            scope: "allWorkspaces".into(),
            voice_files: "embeddedInArchive".into(),
            image_files: "embeddedInArchive".into(),
            exported_at: now_ms(),
            app_version: env!("CARGO_PKG_VERSION").into(),
            settings: self.settings.clone(),
            workspaces,
        };
        Ok(
            (
                PreparedBackup {
                    document,
                    audio_files,
                    image_files,
                },
                summary,
            )
        )
    }

    pub(crate) fn write_backup_archive(
        destination: &Path,
        prepared: PreparedBackup,
    ) -> AppResult<()> {
        let parent = destination.parent().ok_or_else(|| {
            AppError::Invalid("export path has no parent directory".into())
        })?;
        fs::create_dir_all(parent)?;
        let file_name = destination
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "pocket-backup.zip".into());
        let temporary = parent.join(format!(".{file_name}.tmp"));

        let result = (|| -> AppResult<()> {
            let output = File::create(&temporary)?;
            let mut archive = ZipWriter::new(output);
            let options = SimpleFileOptions::default()
                .compression_method(CompressionMethod::Stored);

            archive
                .start_file(BACKUP_MANIFEST_NAME, options)
                .map_err(|e| backup_archive_error("could not write manifest", e))?;
            serde_json::to_writer_pretty(&mut archive, &prepared.document)?;

            for (source, archive_path) in prepared.audio_files {
                archive
                    .start_file(&archive_path, options)
                    .map_err(|e| backup_archive_error("could not add audio file", e))?;
                let mut input = File::open(&source).map_err(|e| {
                    AppError::Storage(format!(
                        "could not read recording {}: {e}",
                        source.display()
                    ))
                })?;
                std::io::copy(&mut input, &mut archive)?;
            }

            for (source, archive_path) in prepared.image_files {
                archive
                    .start_file(&archive_path, options)
                    .map_err(|e| backup_archive_error("could not add image file", e))?;
                let mut input = File::open(&source).map_err(|e| {
                    AppError::Storage(format!(
                        "could not read image {}: {e}",
                        source.display()
                    ))
                })?;
                std::io::copy(&mut input, &mut archive)?;
            }

            let output = archive
                .finish()
                .map_err(|e| backup_archive_error("could not finish backup", e))?;
            output.sync_all()?;
            fs::rename(&temporary, destination)?;
            Ok(())
        })();

        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    pub(crate) fn import_backup_archive(&mut self, source: &Path) -> AppResult<ImportSummary> {
        let input = File::open(source)?;
        let mut archive = ZipArchive::new(input)
            .map_err(|e| backup_archive_error("invalid ZIP backup", e))?;
        let document = read_backup_manifest(&mut archive)?;
        validate_backup(&document, &mut archive)?;

        let mut summary = ImportSummary {
            workspaces_created: 0,
            workspaces_merged: 0,
            items_imported: 0,
            items_skipped: 0,
            recordings_imported: 0,
            recordings_skipped: 0,
            audio_files_restored: 0,
            missing_audio: 0,
            image_files_restored: 0,
            missing_images: 0,
        };
        let mut changed_workspaces = HashSet::new();

        for workspace in document.workspaces {
            let workspace_id = workspace.meta.id.clone();
            if self.workspaces.iter().any(|meta| meta.id == workspace_id) {
                summary.workspaces_merged += 1;
            } else {
                self.workspaces.push(workspace.meta.clone());
                self.data
                    .insert(workspace_id.clone(), WorkspaceData::default());
                summary.workspaces_created += 1;
                changed_workspaces.insert(workspace_id.clone());
            }            for item in workspace.items {
                let exists = self.data[&workspace_id]
                    .items
                    .iter()
                    .any(|existing| existing.id == item.item.id);
                if exists {
                    summary.items_skipped += 1;
                    continue;
                }

                for backup_image in &item.images {
                    let image = &backup_image.image;
                    let destination = self.image_path(&workspace_id, &image.file);
                    if !destination.is_file() {
                        if let Some(archive_path) = &backup_image.archive_path {
                            extract_backup_image(&mut archive, archive_path, &destination)?;
                            summary.image_files_restored += 1;
                        } else {
                            summary.missing_images += 1;
                        }
                    }
                }
                // The note's embedded voice file comes back the same way.
                if let Some(rec) = &item.item.recording {
                    let destination = self.recording_path(&workspace_id, &rec.file);
                    if !destination.is_file() {
                        if let Some(archive_path) = &item.recording_file {
                            extract_backup_audio(&mut archive, archive_path, &destination)?;
                            summary.audio_files_restored += 1;
                        } else {
                            summary.missing_audio += 1;
                        }
                    }
                }

                self.data
                    .get_mut(&workspace_id)
                    .unwrap()
                    .items
                    .push(item.item);
                summary.items_imported += 1;
                changed_workspaces.insert(workspace_id.clone());
            }

            for backup_recording in workspace.recordings {
                let existing = self.data[&workspace_id]
                    .recordings
                    .iter()
                    .find(|recording| recording.id == backup_recording.recording.id)
                    .cloned();
                let destination_file = existing
                    .as_ref()
                    .map(|recording| recording.file.clone())
                    .unwrap_or_else(|| backup_recording.recording.file.clone());
                let destination = self.recording_path(&workspace_id, &destination_file);

                if !destination.is_file() {
                    if let Some(archive_path) = &backup_recording.archive_path {
                        extract_backup_audio(&mut archive, archive_path, &destination)?;
                        summary.audio_files_restored += 1;
                    } else {
                        summary.missing_audio += 1;
                        if existing.is_none() {
                            continue;
                        }
                    }
                }

                if existing.is_some() {
                    summary.recordings_skipped += 1;
                } else {
                    self.data
                        .get_mut(&workspace_id)
                        .unwrap()
                        .recordings
                        .push(backup_recording.recording);
                    summary.recordings_imported += 1;
                    changed_workspaces.insert(workspace_id.clone());
                }
            }
        }

        self.persist_index();
        for workspace_id in changed_workspaces {
            self.persist_workspace(&workspace_id);
        }
        Ok(summary)
    }

    pub fn create_workspace(&mut self, name: &str) -> AppResult<WorkspaceInfo> {
        let name = name.trim();
        if name.is_empty() || name.len() > 64 {
            return Err(AppError::Invalid(
                "workspace name must be 1-64 characters".into(),
            ));
        }
        let ws = WorkspaceMeta {
            id: Uuid::new_v4().to_string(),
            name: name.to_string(),
            created_at: now_ms(),
            updated_at: now_ms(),
        };
        self.workspaces.push(ws.clone());
        self.data.insert(ws.id.clone(), WorkspaceData::default());
        self.persist_index();
        self.persist_workspace(&ws.id);
        self.workspace_info(&ws.id)
    }

    pub fn rename_workspace(&mut self, ws_id: &str, name: &str) -> AppResult<WorkspaceInfo> {
        let name = name.trim();
        if name.is_empty() || name.len() > 64 {
            return Err(AppError::Invalid(
                "workspace name must be 1-64 characters".into(),
            ));
        }
        let ws = self
            .workspaces
            .iter_mut()
            .find(|w| w.id == ws_id)
            .ok_or(AppError::WorkspaceNotFound)?;
        ws.name = name.to_string();
        ws.updated_at = now_ms();
        self.persist_index();
        self.workspace_info(ws_id)
    }

    pub fn delete_workspace(&mut self, ws_id: &str) -> AppResult<()> {
        if !fsutil::valid_id(ws_id) {
            return Err(AppError::Invalid("invalid workspace id".into()));
        }
        if self.workspaces.len() <= 1 {
            return Err(AppError::Invalid(
                "the last workspace cannot be deleted".into(),
            ));
        }
        let Some(pos) = self.workspaces.iter().position(|w| w.id == ws_id) else {
            return Err(AppError::WorkspaceNotFound);
        };

        // Remove on-disk data first; if this fails we abort so the index and
        // the files never disagree.
        crate::shortcuts::diag_log("delete: removing voices dir");
        fsutil::remove_tree(&self.voices_dir(ws_id))
            .map_err(|e| AppError::Storage(format!("could not delete voice files: {e}")))?;
        crate::shortcuts::diag_log("delete: removing images dir");
        fsutil::remove_tree(&self.images_dir(ws_id))
            .map_err(|e| AppError::Storage(format!("could not delete image files: {e}")))?;
        crate::shortcuts::diag_log("delete: removing workspace dir");
        fsutil::remove_tree(&self.workspace_dir(ws_id))
            .map_err(|e| AppError::Storage(format!("could not delete workspace file: {e}")))?;

        self.workspaces.remove(pos);
        self.data.remove(ws_id);
        crate::shortcuts::diag_log("delete: persisting index");
        self.persist_index();

        if self.settings.active_workspace_id == ws_id {
            self.settings.active_workspace_id = self.workspaces[0].id.clone();
            self.persist_settings();
        }
        Ok(())
    }

    pub fn set_active_workspace(&mut self, ws_id: &str) -> AppResult<()> {
        self.workspace(ws_id)?;
        self.settings.active_workspace_id = ws_id.to_string();
        self.persist_settings();
        Ok(())
    }

    // ------------------------------------------------------------------ items

    pub fn create_item(&mut self, ws_id: &str, new: NewItem) -> AppResult<Item> {
        // Image-only notes are valid: the content check only fires when no
        // images are attached.
        if new.content.trim().is_empty() && new.images.is_empty() {
            return Err(AppError::Invalid("content cannot be empty".into()));
        }
        let content = new.content.trim().to_string();
        let url = new.url.map(|u| u.trim().to_string()).filter(|u| !u.is_empty());
        // Only accept image files this app actually saved; client-supplied
        // arbitrary file names would be a path-traversal risk.
        let images: Vec<ItemImage> = new
            .images
            .into_iter()
            .filter(|image| self.image_path(ws_id, &image.file).is_file())
            .collect();
        // An embedded voice note must exist in the workspace feed: it is
        // moved out of the feed into the note (not copied), so undo via the
        // usual removeRecording step never duplicates it.
        let mut recording: Option<Recording> = None;
        if let Some(rec_id) = new.recording_id.as_deref() {
            let data = self.workspace_data_mut(ws_id)?;
            let pos = data
                .recordings
                .iter()
                .position(|r| r.id == rec_id)
                .ok_or(AppError::RecordingNotFound)?;
            recording = Some(data.recordings.remove(pos));
        }
        // Content rule: a note holds at most two of the three media kinds —
        // images + voice is the only forbidden pair with text present, and
        // any pair without text is fine.
        if !images.is_empty() && recording.is_some() && !new.content.trim().is_empty() {
            return Err(AppError::Invalid(
                "a note can hold text, images or a voice note — pick two".into(),
            ));
        }
        let item = Item {
            id: Uuid::new_v4().to_string(),
            item_type: ItemType::Text,
            content,
            title: new
                .title
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty()),
            url,
            images,
            recording,
            pinned: false,
            created_at: now_ms(),
            updated_at: now_ms(),
        };
        let data = self.workspace_data_mut(ws_id)?;
        data.items.push(item.clone());
        self.persist_workspace(ws_id);
        Ok(item)
    }

    pub fn update_item(&mut self, ws_id: &str, item_id: &str, patch: ItemPatch) -> AppResult<Item> {
        // Recordings that leave the note go back to the feed after the item
        // borrow ends; an attached recording was removed from the feed inside
        // the scope below.
        let mut returned_recordings: Vec<Recording> = Vec::new();
        let item = {
            let data = self.workspace_data_mut(ws_id)?;
            let item = data
                .items
                .iter_mut()
                .find(|i| i.id == item_id)
                .ok_or(AppError::ItemNotFound)?;
            if let Some(c) = patch.content {
                let c = c.trim().to_string();
                // Media-only notes (images and/or an embedded voice note) have
                // no text to protect: an empty edit removes the text, not the
                // note.
                if c.is_empty() && item.images.is_empty() && item.recording.is_none() {
                    return Err(AppError::Invalid("content cannot be empty".into()));
                }
                item.content = c;
            }
            if let Some(t) = patch.title {
                item.title = t.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
            }
            if let Some(u) = patch.url {
                item.url = u.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
            }
            if let Some(pinned) = patch.pinned {
                item.pinned = pinned;
            }
            if let Some(recording_patch) = patch.recording_id {
                match recording_patch {
                    // Detach: the recording returns to the workspace feed as
                    // a standalone voice note (no file moves; metadata only).
                    None => {
                        if let Some(rec) = item.recording.take() {
                            returned_recordings.push(rec);
                        }
                    }
                    Some(rec_id) => {
                        // Reject a forbidden triple early: attaching a voice
                        // note to an image note with text would create one.
                        if !item.images.is_empty() && !item.content.trim().is_empty() {
                            return Err(AppError::Invalid(
                                "a note can hold text, images or a voice note — pick two"
                                    .into(),
                            ));
                        }
                        // Same-id re-embed is a no-op; otherwise the current
                        // recording (if any) returns to the feed and the
                        // target one is moved in.
                        if item.recording.as_ref().map(|r| r.id.as_str()) != Some(rec_id.as_str())
                        {
                            if let Some(rec) = item.recording.take() {
                                returned_recordings.push(rec);
                            }
                            let pos = data
                                .recordings
                                .iter()
                                .position(|r| r.id == rec_id)
                                .ok_or(AppError::RecordingNotFound)?;
                            item.recording = Some(data.recordings.remove(pos));
                        }
                    }
                }
            }
            item.updated_at = now_ms();
            item.clone()
        };
        if !returned_recordings.is_empty() {
            let data = self.workspace_data_mut(ws_id)?;
            data.recordings.extend(returned_recordings);
        }
        self.persist_workspace(ws_id);
        Ok(item)
    }

    /// Park a deleted image file for Ctrl+Z. Missing files are fine: the
    /// metadata row goes away regardless.
    fn park_image(&self, ws_id: &str, file: &str) {
        let source = self.image_path(ws_id, file);
        let trash_path = self.image_trash_dir().join(file);
        if fs::create_dir_all(self.image_trash_dir()).is_ok() {
            let _ = fs::rename(&source, &trash_path);
        }
    }

    /// Move a note (with its images and embedded voice file) to another
    /// workspace. Files are copied to the target workspace's media
    /// directories; ids are preserved, so undo is restore-in-source +
    /// remove-in-target.
    pub fn move_item(&mut self, from_ws: &str, to_ws: &str, item_id: &str) -> AppResult<Item> {
        if !fsutil::valid_id(to_ws) {
            return Err(AppError::Invalid("invalid workspace id".into()));
        }
        self.workspace(to_ws)?;
        let item = {
            let data = self.workspace_data_mut(from_ws)?;
            let pos = data
                .items
                .iter()
                .position(|i| i.id == item_id)
                .ok_or(AppError::ItemNotFound)?;
            data.items.remove(pos)
        };
        // Copy attachments over before registering the note in the target:
        // metadata without files would render broken thumbnails/audio.
        for image in &item.images {
            let source = self.image_path(from_ws, &image.file);
            let destination = self.image_path(to_ws, &image.file);
            if source.is_file() && !destination.is_file() {
                fs::create_dir_all(self.images_dir(to_ws))
                    .map_err(|e| AppError::Storage(format!("cannot create images directory: {e}")))?;
                fs::copy(&source, &destination).map_err(|e| {
                    AppError::Storage(format!("could not copy image file: {e}"))
                })?;
            }
        }
        if let Some(rec) = &item.recording {
            let source = self.recording_path(from_ws, &rec.file);
            let destination = self.recording_path(to_ws, &rec.file);
            if source.is_file() && !destination.is_file() {
                fs::create_dir_all(self.voices_dir(to_ws))
                    .map_err(|e| AppError::Storage(format!("cannot create voices directory: {e}")))?;
                fs::copy(&source, &destination).map_err(|e| {
                    AppError::Storage(format!("could not copy voice file: {e}"))
                })?;
            }
        }
        let data = self.workspace_data_mut(to_ws)?;
        data.items.push(item.clone());
        self.persist_workspace(to_ws);
        self.persist_workspace(from_ws);
        Ok(item)
    }

    pub fn delete_item(&mut self, ws_id: &str, item_id: &str) -> AppResult<()> {
        let (parked_images, embedded_recording) = {
            let data = self.workspace_data_mut(ws_id)?;
            let Some(pos) = data.items.iter().position(|i| i.id == item_id) else {
                return Err(AppError::ItemNotFound);
            };
            let item = data.items.remove(pos);
            let parked_images: Vec<String> =
                item.images.iter().map(|i| i.file.clone()).collect();
            (parked_images, item.recording)
        };
        for file in parked_images {
            self.park_image(ws_id, &file);
        }
        if let Some(rec) = embedded_recording {
            // The note's voice file is parked too, so Ctrl+Z restores the
            // whole note with playable audio. Missing files stay fine: the
            // metadata row carries on either way.
            self.park_recording_file(ws_id, &rec.file);
        }
        self.persist_workspace(ws_id);
        Ok(())
    }

    /// Park a recording file (standalone or note-embedded) for Ctrl+Z.
    fn park_recording_file(&self, ws_id: &str, file: &str) {
        let source = self.recording_path(ws_id, file);
        let trash_path = self.trash_dir().join(file);
        if fs::create_dir_all(self.trash_dir()).is_ok() {
            let _ = fs::rename(&source, &trash_path);
        }
    }

    /// Delete many entries in one lock + one persist + one event broadcast.
    /// Missing ids are skipped (a selection may contain already-deleted rows).
    /// Recording audio is parked in the trash for undo, same as
    /// `delete_recording`.
    pub fn delete_entries_bulk(
        &mut self,
        ws_id: &str,
        item_ids: &[String],
        recording_ids: &[String],
    ) -> AppResult<usize> {
        let mut count = 0usize;
        let mut image_files: Vec<String> = Vec::new();
        let mut embedded_recording_files: Vec<String> = Vec::new();
        let removed_files: Vec<String> = {
            let data = self.workspace_data_mut(ws_id)?;
            let before = data.items.len();
            data.items.retain(|i| {
                if item_ids.contains(&i.id) {
                    image_files.extend(i.images.iter().map(|img| img.file.clone()));
                    if let Some(rec) = &i.recording {
                        embedded_recording_files.push(rec.file.clone());
                    }
                    false
                } else {
                    true
                }
            });
            count += before - data.items.len();

            let before = data.recordings.len();
            let mut files: Vec<String> = Vec::new();
            data.recordings.retain(|r| {
                if recording_ids.iter().any(|id| id == &r.id) {
                    files.push(r.file.clone());
                    false
                } else {
                    true
                }
            });
            count += before - data.recordings.len();
            files
        };
        // Park deleted images for undo (after the workspace-data borrow ends).
        for file in image_files.into_iter() {
            self.park_image(ws_id, &file);
        }
        for file in &embedded_recording_files {
            self.park_recording_file(ws_id, file);
        }
        for file in &removed_files {
            let source = self.recording_path(ws_id, file);
            let trash_path = self.trash_dir().join(file);
            if fs::create_dir_all(self.trash_dir()).is_ok() {
                let _ = fs::rename(&source, &trash_path);
            }
        }
        self.persist_workspace(ws_id);
        Ok(count)
    }

    // ------------------------------------------------------------- recordings

    pub fn save_recording(
        &mut self,
        ws_id: &str,
        name: &str,
        duration_ms: u64,
        data_bytes: &[u8],
    ) -> AppResult<Recording> {
        if data_bytes.is_empty() {
            return Err(AppError::Recording("empty recording".into()));
        }
        if data_bytes.len() > 100 * 1024 * 1024 {
            return Err(AppError::Recording("recording too large".into()));
        }
        let id = Uuid::new_v4().to_string();
        let file = format!("{id}.webm");

        let dir = self.voices_dir(ws_id);
        fs::create_dir_all(&dir)
            .map_err(|e| AppError::Storage(format!("cannot create voices directory: {e}")))?;
        // Write to a temp name then rename, so a crash never leaves a
        // half-written recording registered in metadata.
        let final_path = dir.join(&file);
        let tmp_path = dir.join(format!(".{file}.tmp"));
        {
            use std::io::Write;
            let mut f = fs::File::create(&tmp_path)
                .map_err(|e| AppError::Storage(format!("cannot write recording: {e}")))?;
            f.write_all(data_bytes)
                .map_err(|e| AppError::Storage(format!("cannot write recording: {e}")))?;
            f.sync_all()
                .map_err(|e| AppError::Storage(format!("cannot write recording: {e}")))?;
        }
        fs::rename(&tmp_path, &final_path)
            .map_err(|e| AppError::Storage(format!("cannot finalize recording: {e}")))?;

        let rec = Recording {
            id,
            name: clean_recording_name(name),
            file,
            duration_ms,
            size_bytes: data_bytes.len() as u64,
            pinned: false,
            created_at: now_ms(),
        };
        let data = self.workspace_data_mut(ws_id)?;
        data.recordings.push(rec.clone());
        self.persist_workspace(ws_id);
        Ok(rec)
    }

    pub fn rename_recording(
        &mut self,
        ws_id: &str,
        rec_id: &str,
        name: &str,
    ) -> AppResult<Recording> {
        let data = self.workspace_data_mut(ws_id)?;
        let rec = data
            .recordings
            .iter_mut()
            .find(|r| r.id == rec_id)
            .ok_or(AppError::RecordingNotFound)?;
        rec.name = clean_recording_name(name);
        let rec = rec.clone();
        self.persist_workspace(ws_id);
        Ok(rec)
    }

    pub fn set_pinned(
        &mut self,
        ws_id: &str,
        kind: &str,
        entry_id: &str,
        pinned: bool,
    ) -> AppResult<()> {
        let data = self.workspace_data_mut(ws_id)?;
        match kind {
            "text" => {
                let item = data
                    .items
                    .iter_mut()
                    .find(|item| item.id == entry_id)
                    .ok_or(AppError::ItemNotFound)?;
                item.pinned = pinned;
                item.updated_at = now_ms();
            }
            "voice" => {
                let recording = data
                    .recordings
                    .iter_mut()
                    .find(|recording| recording.id == entry_id)
                    .ok_or(AppError::RecordingNotFound)?;
                recording.pinned = pinned;
            }
            _ => return Err(AppError::Invalid("entry kind must be text or voice".into())),
        }
        self.persist_workspace(ws_id);
        Ok(())
    }

    pub fn delete_recording(&mut self, ws_id: &str, rec_id: &str) -> AppResult<()> {
        let (file, remaining) = {
            let data = self.workspace_data_mut(ws_id)?;
            let Some(pos) = data.recordings.iter().position(|r| r.id == rec_id) else {
                return Err(AppError::RecordingNotFound);
            };
            let rec = data.recordings.remove(pos);
            (rec.file, data.recordings.len())
        };
        // Park the audio in the trash instead of erasing it so an undo can
        // restore the recording (the trash is wiped on next startup).
        let source = self.recording_path(ws_id, &file);
        let trash_path = self.trash_dir().join(&file);
        let parked = match fs::create_dir_all(&trash_path.parent().unwrap_or(&self.trash_dir()))
        {
            Ok(()) => fs::rename(&source, &trash_path),
            Err(e) => Err(e),
        };
        match parked {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                self.persist_workspace(ws_id);
                return Err(AppError::Storage(format!(
                    "could not delete voice file: {e}"
                )));
            }
        }
        self.persist_workspace(ws_id);
        let _ = remaining;
        Ok(())
    }

    // ------------------------------------------------------------------ images

    /// Persist an uploaded image into the workspace images directory and
    /// return its metadata row. The caller attaches the row to an item when
    /// the note is created (or updated).
    pub fn save_image(&mut self, ws_id: &str, ext: &str, data_bytes: &[u8]) -> AppResult<ItemImage> {
        if data_bytes.is_empty() {
            return Err(AppError::Invalid("empty image".into()));
        }
        if data_bytes.len() as u64 > MAX_IMAGE_BYTES {
            return Err(AppError::Invalid("image too large".into()));
        }
        let ext = ext.to_ascii_lowercase();
        if !matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "avif") {
            return Err(AppError::Invalid("unsupported image format".into()));
        }
        let id = Uuid::new_v4().to_string();
        let file = format!("{id}.{ext}");

        let dir = self.images_dir(ws_id);
        fs::create_dir_all(&dir)
            .map_err(|e| AppError::Storage(format!("cannot create images directory: {e}")))?;
        let final_path = dir.join(&file);
        let tmp_path = dir.join(format!(".{file}.tmp"));
        {
            use std::io::Write;
            let mut f = fs::File::create(&tmp_path)
                .map_err(|e| AppError::Storage(format!("cannot write image: {e}")))?;
            f.write_all(data_bytes)
                .map_err(|e| AppError::Storage(format!("cannot write image: {e}")))?;
            f.sync_all()
                .map_err(|e| AppError::Storage(format!("cannot write image: {e}")))?;
        }
        fs::rename(&tmp_path, &final_path)
            .map_err(|e| AppError::Storage(format!("cannot finalize image: {e}")))?;

        Ok(ItemImage {
            id,
            file,
            size_bytes: data_bytes.len() as u64,
        })
    }

    /// Upsert a text item back with its original id and timestamps (a Ctrl+Z
    /// restore, or the pre-edit snapshot of an edited item). Images parked in
    /// the trash by the matching delete are moved back first.
    pub fn restore_item(&mut self, ws_id: &str, item: Item) -> AppResult<()> {
        for image in &item.images {
            let parked = self.image_trash_dir().join(&image.file);
            let final_path = self.image_path(ws_id, &image.file);
            if parked.is_file() && !final_path.is_file() {
                fs::create_dir_all(self.images_dir(ws_id))?;
                fs::rename(&parked, &final_path).map_err(|e| {
                    AppError::Storage(format!("could not restore image file: {e}"))
                })?;
            }
        }
        // A deleted image+voice note parks its embedded audio too.
        if let Some(rec) = &item.recording {
            let parked = self.trash_dir().join(&rec.file);
            let final_path = self.recording_path(ws_id, &rec.file);
            if parked.is_file() && !final_path.is_file() {
                fs::create_dir_all(self.voices_dir(ws_id))?;
                fs::rename(&parked, &final_path).map_err(|e| {
                    AppError::Storage(format!("could not restore voice file: {e}"))
                })?;
            }
        }
        let data = self.workspace_data_mut(ws_id)?;
        match data.items.iter_mut().find(|i| i.id == item.id) {
            Some(slot) => *slot = item,
            None => data.items.push(item),
        }
        self.persist_workspace(ws_id);
        Ok(())
    }

    /// Restore a deleted recording: move its audio out of the trash and
    /// upsert the metadata row.
    pub fn restore_recording(&mut self, ws_id: &str, recording: Recording) -> AppResult<()> {
        let parked = self.trash_dir().join(&recording.file);
        let final_path = self.recording_path(ws_id, &recording.file);
        if parked.is_file() {
            fs::create_dir_all(self.voices_dir(ws_id))?;
            fs::rename(&parked, &final_path).map_err(|e| {
                AppError::Storage(format!("could not restore voice file: {e}"))
            })?;
        } else if !final_path.is_file() {
            return Err(AppError::Storage(
                "voice audio is no longer available".into(),
            ));
        }
        let data = self.workspace_data_mut(ws_id)?;
        match data.recordings.iter_mut().find(|r| r.id == recording.id) {
            Some(slot) => *slot = recording,
            None => data.recordings.push(recording),
        }
        self.persist_workspace(ws_id);
        Ok(())
    }

    // ----------------------------------------------------------------- search

    pub fn search(&self, ws_id: &str, query: &str) -> AppResult<Vec<SearchHit>> {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return Ok(Vec::new());
        }
        let data = self.workspace_data(ws_id)?;
        let mut hits = Vec::new();
        for item in &data.items {
            let title = item
                .title
                .clone()
                .unwrap_or_else(|| default_item_title(item));
            let haystacks = [
                title.to_lowercase(),
                item.content.to_lowercase(),
                item.url.clone().unwrap_or_default().to_lowercase(),
            ];
            if haystacks.iter().any(|h| h.contains(&q)) {
                hits.push(SearchHit {
                    kind: "text".into(),
                    id: item.id.clone(),
                    title,
                    snippet: snippet(&item.content, &q),
                    created_at: item.created_at,
                });
            }
        }
        for rec in &data.recordings {
            if rec.name.to_lowercase().contains(&q) {
                hits.push(SearchHit {
                    kind: "voice".into(),
                    id: rec.id.clone(),
                    title: rec.name.clone(),
                    snippet: format!("voice note · {}", rec.file),
                    created_at: rec.created_at,
                });
            }
        }
        hits.sort_by_key(|h| std::cmp::Reverse(h.created_at));
        Ok(hits)
    }
}

// ------------------------------------------------------------------- helpers

fn backup_audio_path(workspace_id: &str, file: &str) -> String {
    format!("audio/{workspace_id}/{file}")
}

fn backup_image_path(workspace_id: &str, file: &str) -> String {
    format!("images/{workspace_id}/{file}")
}

fn has_image_extension(file: &str) -> bool {
    let lower = file.to_ascii_lowercase();
    [".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp", ".avif"]
        .iter()
        .any(|ext| lower.ends_with(ext))
}

fn backup_archive_error(context: &str, error: impl std::fmt::Display) -> AppError {
    AppError::Storage(format!("{context}: {error}"))
}

fn read_backup_manifest<R: Read + Seek>(archive: &mut ZipArchive<R>) -> AppResult<BackupDocument> {
    let mut manifest = archive
        .by_name(BACKUP_MANIFEST_NAME)
        .map_err(|e| backup_archive_error("backup.json is missing", e))?;
    if manifest.size() > MAX_BACKUP_MANIFEST_BYTES {
        return Err(AppError::Invalid("backup manifest is too large".into()));
    }
    let mut json = String::with_capacity(manifest.size() as usize);
    manifest.read_to_string(&mut json)?;
    serde_json::from_str(&json).map_err(AppError::from)
}

fn validate_backup<R: Read + Seek>(
    document: &BackupDocument,
    archive: &mut ZipArchive<R>,
) -> AppResult<()> {
    // v2 = pre-images archives, still importable (items simply carry no
    // images). v3 adds embedded note images.
    if ![2, BACKUP_FORMAT_VERSION].contains(&document.format_version) {
        return Err(AppError::Invalid(format!(
            "unsupported backup version {}",
            document.format_version
        )));
    }
    if document.scope != "allWorkspaces" || document.voice_files != "embeddedInArchive" {
        return Err(AppError::Invalid("not a portable Pocket backup".into()));
    }
    if document.workspaces.is_empty() {
        return Err(AppError::Invalid("backup contains no workspaces".into()));
    }

    let mut workspace_ids = HashSet::new();
    for workspace in &document.workspaces {
        if !fsutil::valid_id(&workspace.meta.id) || !workspace_ids.insert(&workspace.meta.id) {
            return Err(AppError::Invalid(
                "backup contains an invalid or duplicate workspace id".into(),
            ));
        }
        let workspace_name = workspace.meta.name.trim();
        if workspace_name.is_empty() || workspace_name.len() > 64 {
            return Err(AppError::Invalid(
                "backup contains an invalid workspace name".into(),
            ));
        }

        let mut item_ids = HashSet::new();
        for backup_item in &workspace.items {
            let item = &backup_item.item;
            if !fsutil::valid_id(&item.id) || !item_ids.insert(&item.id) {
                return Err(AppError::Invalid(
                    "backup contains an invalid or duplicate item id".into(),
                ));
            }
            if item.content.trim().is_empty() && item.images.is_empty() {
                return Err(AppError::Invalid(
                    "backup contains an empty text item".into(),
                ));
            }
            let mut image_ids = HashSet::new();
            for backup_image in &backup_item.images {
                let image = &backup_image.image;
                if !fsutil::valid_id(&image.id) || !image_ids.insert(&image.id) {
                    return Err(AppError::Invalid(
                        "backup contains an invalid or duplicate image id".into(),
                    ));
                }
                if !fsutil::valid_file_name(&image.file)
                    || !has_image_extension(&image.file)
                {
                    return Err(AppError::Invalid(
                        "backup contains an invalid image file name".into(),
                    ));
                }
                if image.size_bytes > MAX_IMAGE_BYTES {
                    return Err(AppError::Invalid(
                        "backup contains an oversized image".into(),
                    ));
                }
                if let Some(archive_path) = &backup_image.archive_path {
                    let expected = backup_image_path(&workspace.meta.id, &image.file);
                    if archive_path != &expected {
                        return Err(AppError::Invalid(
                            "backup contains an unsafe image path".into(),
                        ));
                    }
                    let entry = archive
                        .by_name(archive_path)
                        .map_err(|e| backup_archive_error("backup image file is missing", e))?;
                    if !entry.is_file() || entry.size() != image.size_bytes {
                        return Err(AppError::Invalid(
                            "backup image metadata does not match its file".into(),
                        ));
                    }
                }
            }

            // The note's embedded voice file (format v3 only — v2 items carry
            // no recordings at all).
            if let Some(rec) = &backup_item.item.recording {
                if !fsutil::valid_file_name(&rec.file) || !rec.file.ends_with(".webm") {
                    return Err(AppError::Invalid(
                        "backup contains an invalid note recording file name".into(),
                    ));
                }
                if rec.size_bytes > MAX_RECORDING_BYTES {
                    return Err(AppError::Invalid(
                        "backup contains an oversized note recording".into(),
                    ));
                }
                if let Some(archive_path) = &backup_item.recording_file {
                    let expected = backup_audio_path(&workspace.meta.id, &rec.file);
                    if archive_path != &expected {
                        return Err(AppError::Invalid(
                            "backup contains an unsafe audio path".into(),
                        ));
                    }
                    let entry = archive
                        .by_name(archive_path)
                        .map_err(|e| backup_archive_error("backup audio file is missing", e))?;
                    if !entry.is_file() || entry.size() != rec.size_bytes {
                        return Err(AppError::Invalid(
                            "backup recording metadata does not match its file".into(),
                        ));
                    }
                }
            }
        }

        let mut recording_ids = HashSet::new();
        for backup_recording in &workspace.recordings {
            let recording = &backup_recording.recording;
            if !fsutil::valid_id(&recording.id) || !recording_ids.insert(&recording.id) {
                return Err(AppError::Invalid(
                    "backup contains an invalid or duplicate recording id".into(),
                ));
            }
            if !fsutil::valid_file_name(&recording.file) || !recording.file.ends_with(".webm") {
                return Err(AppError::Invalid(
                    "backup contains an invalid recording file name".into(),
                ));
            }
            if recording.size_bytes > MAX_RECORDING_BYTES {
                return Err(AppError::Invalid(
                    "backup contains an oversized recording".into(),
                ));
            }
            if let Some(archive_path) = &backup_recording.archive_path {
                let expected = backup_audio_path(&workspace.meta.id, &recording.file);
                if archive_path != &expected {
                    return Err(AppError::Invalid(
                        "backup contains an unsafe audio path".into(),
                    ));
                }
                let entry = archive
                    .by_name(archive_path)
                    .map_err(|e| backup_archive_error("backup audio file is missing", e))?;
                if !entry.is_file() || entry.size() != recording.size_bytes {
                    return Err(AppError::Invalid(
                        "backup audio metadata does not match its file".into(),
                    ));
                }
            }
        }
    }
    Ok(())
}

fn extract_backup_audio<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    archive_path: &str,
    destination: &Path,
) -> AppResult<()> {
    let parent = destination
        .parent()
        .ok_or_else(|| AppError::Invalid("recording path has no parent".into()))?;
    fs::create_dir_all(parent)?;
    let file_name = destination
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "recording.webm".into());
    let temporary = parent.join(format!(".{file_name}.importing"));

    let result = (|| -> AppResult<()> {
        let mut entry = archive
            .by_name(archive_path)
            .map_err(|e| backup_archive_error("backup audio file is missing", e))?;
        let mut output = File::create(&temporary)?;
        std::io::copy(&mut entry, &mut output)?;
        output.sync_all()?;
        fs::rename(&temporary, destination)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn extract_backup_image<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    archive_path: &str,
    destination: &Path,
) -> AppResult<()> {
    let parent = destination
        .parent()
        .ok_or_else(|| AppError::Invalid("image path has no parent".into()))?;
    fs::create_dir_all(parent)?;
    let file_name = destination
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "image.bin".into());
    let temporary = parent.join(format!(".{file_name}.importing"));

    let result = (|| -> AppResult<()> {
        let mut entry = archive
            .by_name(archive_path)
            .map_err(|e| backup_archive_error("backup image file is missing", e))?;
        let mut output = File::create(&temporary)?;
        std::io::copy(&mut entry, &mut output)?;
        output.sync_all()?;
        fs::rename(&temporary, destination)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// Canonical per-workspace data file path (shared by load and save).
fn self_path(data_dir: &std::path::Path, ws_id: &str) -> PathBuf {
    data_dir
        .join("workspaces")
        .join(ws_id)
        .join("workspace.json")
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn clean_recording_name(name: &str) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        "Voice note".into()
    } else {
        trimmed.chars().take(120).collect()
    }
}

pub fn default_item_title(item: &Item) -> String {
    let first_line = item.content.lines().next().unwrap_or("").trim();
    let mut t: String = first_line.chars().take(80).collect();
    if first_line.chars().count() > 80 {
        t.push('…');
    }
    if t.is_empty() {
        "Untitled".into()
    } else {
        t
    }
}

fn snippet(content: &str, query: &str) -> String {
    let lower = content.to_lowercase();
    match lower.find(query) {
        Some(idx) => {
            let start = idx.saturating_sub(40);
            let s: String = content
                .chars()
                .skip(
                    content
                        .char_indices()
                        .position(|(bi, _)| bi == start)
                        .unwrap_or(0),
                )
                .take(120)
                .collect();
            let prefix = if start > 0 { "…" } else { "" };
            format!("{prefix}{s}")
        }
        None => content.chars().take(120).collect(),
    }
}

fn backup_corrupt(path: &PathBuf) {
    if path.exists() {
        let backup = path.with_extension(format!(
            "corrupt-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
        ));
        eprintln!("[pocket] corrupt data file, backing up: {}", path.display());
        let _ = fs::rename(path, backup);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_store() -> (Store, PathBuf) {
        let dir = std::env::temp_dir().join(format!("pocket-test-{}", uuid::Uuid::new_v4()));
        let store = Store::load(dir.clone(), false);
        (store, dir)
    }

    fn cleanup(dir: &PathBuf) {
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn workspace_lifecycle_and_file_cleanup() {
        let (mut store, dir) = test_store();
        let ws = store.create_workspace("Test WS").unwrap();

        store
            .create_item(
                &ws.meta.id,
                NewItem {
                    item_type: ItemType::Text,
                    content: "hello world".into(),
                    title: None,
                    url: None,
                    images: Vec::new(),
                    recording_id: None,
                },
            )
            .unwrap();
        store
            .save_recording(&ws.meta.id, "note", 1500, b"fakeaudio")
            .unwrap();

        assert!(store.workspace_path(&ws.meta.id).exists());
        assert!(store
            .recording_path(&ws.meta.id, &store.data[&ws.meta.id].recordings[0].file)
            .exists());

        let counts = store.counts(&ws.meta.id).unwrap();
        assert_eq!(counts.texts, 1);
        assert_eq!(counts.recordings, 1);

        store.delete_workspace(&ws.meta.id).unwrap();
        assert!(!store.workspace_path(&ws.meta.id).exists());
        assert!(!store.voices_dir(&ws.meta.id).exists());
        assert_eq!(store.workspaces.len(), 1);
        cleanup(&dir);
    }

    #[test]
    fn search_finds_items_and_recordings() {
        let (mut store, dir) = test_store();
        let ws = store.create_workspace("WS").unwrap();
        store
            .create_item(
                &ws.meta.id,
                NewItem {
                    item_type: ItemType::Text,
                    content: "Rust ownership rules".into(),
                    title: None,
                    url: None,
                    images: Vec::new(),
                    recording_id: None,
                },
            )
            .unwrap();
        store
            .save_recording(&ws.meta.id, "Standup notes", 500, b"x")
            .unwrap();

        let hits = store.search(&ws.meta.id, "rust").unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].kind, "text");

        let hits = store.search(&ws.meta.id, "standup").unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].kind, "voice");

        assert!(store.search(&ws.meta.id, "  ").unwrap().is_empty());
        cleanup(&dir);
    }

    #[test]
    fn link_items_carry_url() {
        let (mut store, dir) = test_store();
        let ws = store.create_workspace("WS").unwrap();
        let item = store
            .create_item(
                &ws.meta.id,
                NewItem {
                    item_type: ItemType::Text,
                    content: "https://example.com".into(),
                    title: None,
                    url: None,
                    images: Vec::new(),
                    recording_id: None,
                },
            )
            .unwrap();
        assert_eq!(item.item_type, ItemType::Text);
        cleanup(&dir);
    }

    #[test]
    fn persist_and_reload_roundtrip() {
        let dir = std::env::temp_dir().join(format!("pocket-test-{}", uuid::Uuid::new_v4()));
        {
            let mut store = Store::load(dir.clone(), false);
            let ws = store.create_workspace("Persisted").unwrap();
            store.set_active_workspace(&ws.meta.id).unwrap();
            store
                .create_item(
                    &ws.meta.id,
                    NewItem {
                        item_type: ItemType::Text,
                        content: "survives restart".into(),
                        title: None,
                        url: None,
                        images: Vec::new(),
                        recording_id: None,
                    },
                )
                .unwrap();
        }
        {
            let store = Store::load(dir.clone(), false);
            assert_eq!(store.workspaces.len(), 2);
            assert_eq!(store.settings.active_workspace_id, store.workspaces[1].id);
            let data = store.workspace_data(&store.workspaces[1].id).unwrap();
            assert_eq!(data.items.len(), 1);
            assert_eq!(data.items[0].content, "survives restart");
        }
        cleanup(&dir);
    }

    #[test]
    fn recording_delete_parks_audio_for_undo() {
        let (mut store, dir) = test_store();
        let ws = store.create_workspace("WS").unwrap();
        let rec = store
            .save_recording(&ws.meta.id, "temp", 100, b"abc")
            .unwrap();
        let path = store.recording_path(&ws.meta.id, &rec.file);
        assert!(path.exists());
        store.delete_recording(&ws.meta.id, &rec.id).unwrap();
        assert!(!path.exists());
        // The audio is parked in the trash so an undo can restore it, and a
        // restore puts it back at the original path.
        let restored = store.restore_recording(&ws.meta.id, rec).unwrap();
        assert!(path.exists());
        let _ = restored;
        cleanup(&dir);
    }

    #[test]
    fn portable_backup_roundtrip_includes_audio_and_deduplicates_imports() {
        let (mut store, dir) = test_store();
        let first_id = store.workspaces[0].id.clone();
        store
            .create_item(
                &first_id,
                NewItem {
                    item_type: ItemType::Text,
                    content: "first workspace item".into(),
                    title: None,
                    url: None,
                    images: Vec::new(),
                    recording_id: None,
                },
            )
            .unwrap();
        let second = store.create_workspace("Exported workspace").unwrap();
        let recording = store
            .save_recording(&second.meta.id, "Exported voice", 750, b"audio")
            .unwrap();
        let destination = dir.join("pocket-backup.zip");
        let (prepared, summary) = store.prepare_backup(&destination).unwrap();
        Store::write_backup_archive(&destination, prepared).unwrap();

        assert_eq!(summary.workspaces, 2);
        assert_eq!(summary.items, 1);
        assert_eq!(summary.recordings, 1);
        assert_eq!(summary.audio_files, 1);
        assert_eq!(summary.missing_audio, 0);

        let import_dir = std::env::temp_dir().join(format!("pocket-import-test-{}", Uuid::new_v4()));
        let mut imported_store = Store::load(import_dir.clone(), false);
        imported_store.settings.theme = "dark".into();
        let imported = imported_store.import_backup_archive(&destination).unwrap();
        assert_eq!(imported.workspaces_created, 2);
        assert_eq!(imported.items_imported, 1);
        assert_eq!(imported.recordings_imported, 1);
        assert_eq!(imported.audio_files_restored, 1);
        assert_eq!(imported.missing_audio, 0);
        assert_eq!(imported_store.settings.theme, "dark");

        let imported_data = imported_store.workspace_data(&second.meta.id).unwrap();
        assert_eq!(imported_data.recordings[0].id, recording.id);
        assert_eq!(
            fs::read(imported_store.recording_path(&second.meta.id, &recording.file)).unwrap(),
            b"audio"
        );

        let repeated = imported_store.import_backup_archive(&destination).unwrap();
        assert_eq!(repeated.workspaces_created, 0);
        assert_eq!(repeated.items_imported, 0);
        assert_eq!(repeated.recordings_imported, 0);
        assert_eq!(repeated.audio_files_restored, 0);
        assert_eq!(repeated.items_skipped, 1);
        assert_eq!(repeated.recordings_skipped, 1);

        cleanup(&import_dir);
        cleanup(&dir);
    }

    #[test]
    fn note_with_recording_rolls_recording_into_the_item() {
        let (mut store, dir) = test_store();
        let ws = store.create_workspace("WS").unwrap();
        let rec = store
            .save_recording(&ws.meta.id, "temp", 800, b"voicebytes")
            .unwrap();
        let item = store
            .create_item(
                &ws.meta.id,
                NewItem {
                    item_type: ItemType::Text,
                    content: "note with voice".into(),
                    title: None,
                    url: None,
                    images: Vec::new(),
                    recording_id: Some(rec.id.clone()),
                },
            )
            .unwrap();
        // The recording moved out of the feed into the note (not copied).
        assert_eq!(item.recording.as_ref().unwrap().id, rec.id);
        assert!(store.workspace_data(&ws.meta.id).unwrap().recordings.is_empty());
        assert!(store.recording_path(&ws.meta.id, &rec.file).exists());

        // Detaching puts it back into the feed.
        let updated = store
            .update_item(
                &ws.meta.id,
                &item.id,
                ItemPatch {
                    recording_id: Some(None),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(updated.recording.is_none());
        assert_eq!(store.workspace_data(&ws.meta.id).unwrap().recordings.len(), 1);
        assert!(store.recording_path(&ws.meta.id, &rec.file).exists());
        cleanup(&dir);
    }

    #[test]
    fn deleting_note_parks_embedded_audio_and_undo_restores_it() {
        let (mut store, dir) = test_store();
        let ws = store.create_workspace("WS").unwrap();
        let rec = store
            .save_recording(&ws.meta.id, "temp", 800, b"voicebytes")
            .unwrap();
        let item = store
            .create_item(
                &ws.meta.id,
                NewItem {
                    item_type: ItemType::Text,
                    content: "voice note".into(),
                    title: None,
                    url: None,
                    images: Vec::new(),
                    recording_id: Some(rec.id.clone()),
                },
            )
            .unwrap();
        let audio_path = store.recording_path(&ws.meta.id, &rec.file);
        store.delete_item(&ws.meta.id, &item.id).unwrap();
        assert!(!audio_path.exists()); // parked for undo
        store.restore_item(&ws.meta.id, item).unwrap();
        assert!(audio_path.exists());
        cleanup(&dir);
    }

    #[test]
    fn move_item_copies_media_to_the_target_workspace() {
        let (mut store, dir) = test_store();
        let from = store.create_workspace("From").unwrap();
        let to = store.create_workspace("To").unwrap();
        let image = store
            .save_image(&from.meta.id, "png", b"fakepng")
            .unwrap();
        let rec = store
            .save_recording(&from.meta.id, "temp", 800, b"voicebytes")
            .unwrap();
        let item = store
            .create_item(
                &from.meta.id,
                NewItem {
                    item_type: ItemType::Text,
                    content: String::new(),
                    title: None,
                    url: None,
                    images: vec![image],
                    recording_id: Some(rec.id.clone()),
                },
            )
            .unwrap();

        let moved = store
            .move_item(&from.meta.id, &to.meta.id, &item.id)
            .unwrap();
        assert_eq!(moved.id, item.id);
        // Files exist in the target; source files remain for undo.
        assert!(store
            .image_path(&to.meta.id, &moved.images[0].file)
            .exists());
        assert!(store.recording_path(&to.meta.id, &moved.recording.unwrap().file).exists());
        assert!(store.workspace_data(&from.meta.id).unwrap().items.is_empty());
        cleanup(&dir);
    }
}
