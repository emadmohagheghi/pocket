export type ItemType = "text";
export type EntryKind = "text" | "voice";

/** One image attached to a note. The bytes live in the backend images dir. */
export interface ItemImage {
  id: string;
  /** Generated file name inside the workspace images dir. */
  file: string;
  sizeBytes: number;
}

export interface Item {
  id: string;
  itemType: ItemType;
  content: string;
  title: string | null;
  url: string | null;
  images: ItemImage[];
  /** Voice note embedded in this note (image+voice notes); null otherwise. */
  recording: Recording | null;
  pinned: boolean;
  createdAt: number;
  updatedAt: number;
}

export interface Recording {
  id: string;
  name: string;
  file: string;
  durationMs: number;
  sizeBytes: number;
  pinned: boolean;
  createdAt: number;
}

export interface WorkspaceData {
  items: Item[];
  recordings: Recording[];
}

export interface Counts {
  texts: number;
  recordings: number;
}

export interface WorkspaceInfo {
  id: string;
  name: string;
  createdAt: number;
  updatedAt: number;
  counts: Counts;
}

export interface Settings {
  launchOnStartup: boolean;
  startMinimized: boolean;
  activeWorkspaceId: string;
  gamingDetectionEnabled: boolean;
  /** Keep the window above all other applications. */
  alwaysOnTop: boolean;
  theme: "system" | "light" | "dark";
  /** 0 shows the full note; 1–6 sets the collapsed preview height. */
  notePreviewLines: number;
  /** App version running the previous launch; null on first-recorded launch. */
  lastLaunchVersion: string | null;
  /** Version whose "what's new" modal was dismissed; null until then. */
  whatsNewSeenVersion: string | null;
}

export interface StorageInfo {
  dataDir: string;
  sizeBytes: number;
  usesFallbackLocation: boolean;
}

export interface ExportSummary {
  path: string;
  workspaces: number;
  items: number;
  recordings: number;
  audioFiles: number;
  missingAudio: number;
  images: number;
  missingImages: number;
}

export interface ImportSummary {
  workspacesCreated: number;
  workspacesMerged: number;
  itemsImported: number;
  itemsSkipped: number;
  recordingsImported: number;
  recordingsSkipped: number;
  audioFilesRestored: number;
  missingAudio: number;
  imageFilesRestored: number;
  missingImages: number;
}

export interface InitialState {
  settings: Settings;
  workspaces: WorkspaceInfo[];
  storage: StorageInfo;
  /** Version whose "what's new" modal should show once, if any. */
  showWhatsNewFor: string | null;
}

export interface SearchHit {
  kind: string;
  id: string;
  title: string;
  snippet: string;
  createdAt: number;
}

export interface StateChangedPayload {
  settings: Settings;
  workspaces: WorkspaceInfo[];
}
