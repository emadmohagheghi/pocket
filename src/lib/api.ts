import { invoke } from "@tauri-apps/api/core";
import type { UndoStep } from "@/store";
import type {
  Counts,
  ExportSummary,
  InitialState,
  ImportSummary,
  Item,
  ItemType,
  Recording,
  SearchHit,
  Settings,
  StorageInfo,
  WorkspaceData,
  WorkspaceInfo,
} from "@/types";

export const api = {
  getState: () => invoke<InitialState>("get_state"),
  frontendReady: () => invoke<void>("frontend_ready"),
  getItems: (workspaceId: string) =>
    invoke<WorkspaceData>("get_items", { workspaceId }),

  createWorkspace: (name: string) =>
    invoke<WorkspaceInfo>("create_workspace", { name }),
  renameWorkspace: (workspaceId: string, name: string) =>
    invoke<WorkspaceInfo>("rename_workspace", { workspaceId, name }),
  deleteWorkspace: (workspaceId: string) =>
    invoke<void>("delete_workspace", { workspaceId }),
  getWorkspaceCounts: (workspaceId: string) =>
    invoke<Counts>("get_workspace_counts", { workspaceId }),
  setActiveWorkspace: (workspaceId: string) =>
    invoke<void>("set_active_workspace", { workspaceId }),

  createItem: (
    workspaceId: string,
    item: { itemType: ItemType; content: string; title?: string | null; url?: string | null }
  ) => invoke<Item>("create_item", { workspaceId, item }),
  updateItem: (
    workspaceId: string,
    itemId: string,
    patch: {
      content?: string;
      title?: string | null;
      url?: string | null;
      pinned?: boolean;
    }
  ) => invoke<Item>("update_item", { workspaceId, itemId, patch }),
  deleteItem: (workspaceId: string, itemId: string) =>
    invoke<void>("delete_item", { workspaceId, itemId }),
  deleteEntriesBulk: (
    workspaceId: string,
    itemIds: string[],
    recordingIds: string[]
  ) => invoke<number>("delete_entries_bulk", { workspaceId, itemIds, recordingIds }),

  setPinned: (
    workspaceId: string,
    kind: "text" | "voice",
    entryId: string,
    pinned: boolean
  ) => invoke<void>("set_pinned", { workspaceId, kind, entryId, pinned }),

  search: (workspaceId: string, query: string) =>
    invoke<SearchHit[]>("search", { workspaceId, query }),

  saveRecording: (workspaceId: string, name: string, durationMs: number, audio: ArrayBuffer) =>
    invoke<Recording>("save_recording", audio, {
      headers: {
        "x-pocket-workspace": workspaceId,
        "x-pocket-name": encodeURIComponent(name),
        "x-pocket-duration": String(Math.round(durationMs)),
      },
    }),
  renameRecording: (workspaceId: string, recordingId: string, name: string) =>
    invoke<Recording>("rename_recording", { workspaceId, recordingId, name }),
  deleteRecording: (workspaceId: string, recordingId: string) =>
    invoke<void>("delete_recording", { workspaceId, recordingId }),

  copyToClipboard: (text: string) =>
    invoke<void>("copy_to_clipboard", { text }),
  openUrl: (url: string) => invoke<void>("open_url", { url }),
  applyUndo: (steps: UndoStep[]) =>
    invoke<void>("apply_undo", { steps }),

  updateSettings: (patch: Partial<Settings>) =>
    invoke<Settings>("update_settings", { patch }),
  getGamingState: () => invoke<boolean>("get_gaming_state"),

  getStorageInfo: () => invoke<StorageInfo>("get_storage_info"),
  openDataFolder: () => invoke<void>("open_data_folder"),
  closeWindow: () => invoke<void>("close_main_window"),
  exportBackup: (path: string) => invoke<ExportSummary>("export_backup", { path }),
  importBackup: (path: string) => invoke<ImportSummary>("import_backup", { path }),

  openVoiceCapture: () => invoke<void>("open_voice_capture"),
  /** Download the latest release's installer in the background and launch it.
   *  The backend exits the app once the installer is running. */
  installAndLaunchUpdate: () => invoke<void>("install_and_launch_update"),
  log: (message: string) => invoke<void>("frontend_log", { message }).catch(() => {}),
};

export function voiceUrl(workspaceId: string, file: string): string {
  const isWindows = navigator.userAgent.includes("Windows");
  return isWindows
    ? `http://voice.localhost/${workspaceId}/${file}`
    : `voice://localhost/${workspaceId}/${file}`;
}

export function recordingExtension(): string {
  const canOpus =
    typeof MediaRecorder !== "undefined" &&
    MediaRecorder.isTypeSupported("audio/webm;codecs=opus");
  return canOpus ? "audio/webm;codecs=opus" : "audio/webm";
}
