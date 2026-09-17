import { create } from "zustand";
import { listen } from "@tauri-apps/api/event";

import { api } from "@/lib/api";
import type {
  Item,
  ItemImage,
  Recording,
  Settings,
  StateChangedPayload,
  WorkspaceData,
  WorkspaceInfo,
  EntryKind,
} from "@/types";

function errMessage(e: unknown): string {
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  return String(e);
}

/**
 * One reversible primitive. `restore*` steps upsert the full entity back
 * (original id and timestamps preserved); `remove*` steps delete it again.
 * A user-visible action (delete, merge, move, …) is a list of these.
 */
export type UndoStep =
  | { type: "restoreItem"; workspaceId: string; item: Item }
  | { type: "restoreRecording"; workspaceId: string; recording: Recording }
  | { type: "removeItem"; workspaceId: string; itemId: string }
  | { type: "removeRecording"; workspaceId: string; recordingId: string };

const UNDO_HISTORY_LIMIT = 10;

interface PlayerTrack {
  recordingId: string;
  name: string;
  wsId: string;
  file: string;
  /** Known length in seconds, so the time display is right from frame one. */
  duration: number;
}

interface PocketStore {
  ready: boolean;
  settings: Settings | null;
  workspaces: WorkspaceInfo[];
  data: WorkspaceData | null;
  /** Item to scroll to + highlight (from search). */
  focusItemId: string | null;
  editRequest: { id: string; kind: EntryKind; nonce: number } | null;
  gaming: boolean;
  /** Recent actions for Ctrl+Z, newest last. */
  undoHistory: UndoStep[][];

  /** Shared voice player: one track at a time, driven by PlayerBar. */
  player: PlayerTrack | null;
  playerPlaying: boolean;
  playerTime: number;
  playerDuration: number;
  playerSeekRequest: number | null;
  /** True while the user is actively dragging a waveform (progress reports
      from the audio element are suppressed so the scrub stays in charge). */
  playerScrubbing: boolean;

  playRecording: (rec: Recording) => void;
  togglePlayer: () => void;
  stopPlayer: () => void;
  requestPlayerSeek: (seconds: number) => void;
  skipPlayer: (deltaSeconds: number) => void;
  reportPlayerProgress: (time: number, duration: number, playing: boolean) => void;
  setPlayerScrubbing: (scrubbing: boolean) => void;

  init: () => Promise<void>;
  setFocusItem: (id: string | null) => void;
  requestEdit: (id: string, kind: EntryKind) => void;
  clearEditRequest: () => void;
  refreshItems: () => Promise<void>;

  createWorkspace: (name: string) => Promise<WorkspaceInfo | null>;
  renameWorkspace: (id: string, name: string) => Promise<void>;
  deleteWorkspace: (id: string) => Promise<void>;
  setActiveWorkspace: (id: string) => Promise<void>;

  createItem: (
    content: string,
    images?: ItemImage[],
    recordingId?: string | null
  ) => Promise<Item | null>;
  updateItem: (itemId: string, patch: Partial<Item>) => Promise<void>;
  deleteItem: (itemId: string) => Promise<void>;
  /** Toggle the todo-style done state (backed by the legacy `pinned`
      flag, which is no longer used for pinning). Works for both text
      items and voice recordings. */
  setEntryDone: (kind: "text" | "voice", entryId: string, done: boolean) => Promise<void>;

  renameRecording: (recordingId: string, name: string) => Promise<void>;
  deleteRecording: (recordingId: string) => Promise<void>;

  /** Record one user action for Ctrl+Z (oldest actions dropped past 10). */
  recordUndo: (steps: UndoStep[]) => void;
  /** Undo the most recent action. Returns true when something was undone. */
  undo: () => Promise<boolean>;

  setSettings: (patch: Partial<Settings>) => Promise<void>;
}

// React StrictMode intentionally remounts effects in development. Keep app
// initialization process-wide so listeners and initial reads only run once.
let initPromise: Promise<void> | null = null;

export const usePocket = create<PocketStore>((set, get) => ({
  ready: false,
  settings: null,
  workspaces: [],
  data: null,
  focusItemId: null,
  editRequest: null,
  gaming: false,
  undoHistory: [],
  player: null,
  playerPlaying: false,
  playerTime: 0,
  playerDuration: 0,
  playerSeekRequest: null,
  playerScrubbing: false,

  init: () => {
    if (initPromise) return initPromise;

    initPromise = (async () => {
      try {
        // Backend → frontend event wiring. Register all independent listeners
        // together and only once, including under React StrictMode.
        await Promise.all([
          listen<StateChangedPayload>("state-changed", (e) => {
            const prevActive = get().settings?.activeWorkspaceId;
            const { settings, workspaces } = e.payload;
            set({ settings, workspaces });
            if (settings.activeWorkspaceId !== prevActive) {
              void get().refreshItems();
            }
          }),
          listen<{ workspaceId: string; data: WorkspaceData }>(
            "items-changed",
            (e) => {
              if (e.payload.workspaceId === get().settings?.activeWorkspaceId) {
                set({ data: e.payload.data });
              }
            }
          ),
          listen<boolean>("gaming-mode-changed", (e) => set({ gaming: e.payload })),
        ]);

        const initial = await api.getState();
        // A state-changed event may land while getState is still in flight
        // (listeners are registered first on purpose). Seeding unconditionally
        // would clobber that newer payload with the older snapshot, so only
        // seed when no event got here first.
        if (!get().settings) {
          set({
            settings: initial.settings,
            workspaces: initial.workspaces,
          });
        }
        await get().refreshItems();
      } catch (e) {
        void api.log(`init FAILED: ${errMessage(e)}`);
      } finally {
        set({ ready: true });
      }

      void api
        .getGamingState()
        .then((g) => set({ gaming: g }))
        .catch(() => {});
    })();

    return initPromise;
  },

  setFocusItem: (focusItemId) => set({ focusItemId }),
  requestEdit: (id, kind) =>
    set({ editRequest: { id, kind, nonce: Date.now() } }),
  clearEditRequest: () => set({ editRequest: null }),

  refreshItems: async () => {
    const wsId = get().settings?.activeWorkspaceId;
    if (!wsId) return;
    try {
      const data = await api.getItems(wsId);
      set({ data });
    } catch (e) {
      void api.log(`refreshItems FAILED: ${errMessage(e)}`);
    }
  },

  createWorkspace: async (name) => {
    try {
      const ws = await api.createWorkspace(name);
      set((s) => ({ workspaces: [...s.workspaces, ws] }));
      return ws;
    } catch (e) {
      void api.log(`createWorkspace FAILED: ${errMessage(e)}`);
      return null;
    }
  },

  renameWorkspace: async (id, name) => {
    try {
      const ws = await api.renameWorkspace(id, name);
      set((s) => ({
        workspaces: s.workspaces.map((w) => (w.id === id ? { ...w, name: ws.name } : w)),
      }));
    } catch (e) {
      void api.log(`renameWorkspace FAILED: ${errMessage(e)}`);
    }
  },

  deleteWorkspace: async (id) => {
    try {
      await api.log(`deleteWorkspace: invoking backend for ${id}`);
      await api.deleteWorkspace(id);
      await api.log("deleteWorkspace: backend resolved");
    } catch (e) {
      await api.log(`deleteWorkspace: backend FAILED: ${errMessage(e)}`);
    }
  },

  setActiveWorkspace: async (id) => {
    try {
      await api.setActiveWorkspace(id);
      // The state-changed event refreshes everything else.
    } catch (e) {
      void api.log(`setActiveWorkspace FAILED: ${errMessage(e)}`);
    }
  },

  createItem: async (content, images, recordingId) => {
    const wsId = get().settings?.activeWorkspaceId;
    if (!wsId) return null;
    // Media-only notes are valid (empty text + attachments / voice).
    if (!content.trim() && (images?.length ?? 0) === 0 && !recordingId) return null;
    try {
      const item = await api.createItem(wsId, {
        itemType: "text",
        content,
        images,
        recordingId: recordingId ?? null,
      });
      get().recordUndo([
        { type: "removeItem", workspaceId: wsId, itemId: item.id },
      ]);
      return item;
    } catch (e) {
      void api.log(`createItem FAILED: ${errMessage(e)}`);
      return null;
    }
  },

  updateItem: async (itemId, patch) => {
    const wsId = get().settings?.activeWorkspaceId;
    if (!wsId) return;
    try {
      const prev = get().data?.items.find((i) => i.id === itemId);
      await api.updateItem(wsId, itemId, patch as Record<string, unknown>);
      if (prev) {
        get().recordUndo([
          { type: "restoreItem", workspaceId: wsId, item: prev },
        ]);
      }
    } catch (e) {
      void api.log(`updateItem FAILED: ${errMessage(e)}`);
    }
  },

  deleteItem: async (itemId) => {
    const wsId = get().settings?.activeWorkspaceId;
    if (!wsId) return;
    try {
      const prev = get().data?.items.find((i) => i.id === itemId);
      await api.deleteItem(wsId, itemId);
      if (prev) {
        get().recordUndo([
          { type: "restoreItem", workspaceId: wsId, item: prev },
        ]);
      }
    } catch (e) {
      void api.log(`deleteItem FAILED: ${errMessage(e)}`);
    }
  },

  setEntryDone: async (kind, entryId, done) => {
    const wsId = get().settings?.activeWorkspaceId;
    if (!wsId) return;
    try {
      const prev =
        kind === "text"
          ? get().data?.items.find((i) => i.id === entryId)
          : undefined;
      const prevRec =
        kind === "voice"
          ? get().data?.recordings.find((r) => r.id === entryId)
          : undefined;
      await api.setPinned(wsId, kind, entryId, done);
      if (prev) {
        get().recordUndo([
          { type: "restoreItem", workspaceId: wsId, item: prev },
        ]);
      } else if (prevRec) {
        get().recordUndo([
          { type: "restoreRecording", workspaceId: wsId, recording: prevRec },
        ]);
      }
    } catch (e) {
      void api.log(`setEntryDone FAILED: ${errMessage(e)}`);
    }
  },

  renameRecording: async (recordingId, name) => {
    const wsId = get().settings?.activeWorkspaceId;
    if (!wsId) return;
    try {
      await api.renameRecording(wsId, recordingId, name);
    } catch (e) {
      void api.log(`renameRecording FAILED: ${errMessage(e)}`);
    }
  },

  deleteRecording: async (recordingId) => {
    const wsId = get().settings?.activeWorkspaceId;
    if (!wsId) return;
    try {
      const prev = get().data?.recordings.find((r) => r.id === recordingId);
      await api.deleteRecording(wsId, recordingId);
      if (prev) {
        get().recordUndo([
          { type: "restoreRecording", workspaceId: wsId, recording: prev },
        ]);
      }
    } catch (e) {
      void api.log(`deleteRecording FAILED: ${errMessage(e)}`);
    }
  },

  recordUndo: (steps) => {
    if (steps.length === 0) return;
    set((s) => ({
      undoHistory: [...s.undoHistory, steps].slice(-UNDO_HISTORY_LIMIT),
    }));
  },

  undo: async () => {
    const history = get().undoHistory;
    if (history.length === 0) return false;
    const steps = history[history.length - 1];
    set({ undoHistory: history.slice(0, -1) });
    try {
      await api.applyUndo(steps);
      return true;
    } catch (e) {
      void api.log(`undo FAILED: ${errMessage(e)}`);
      return false;
    }
  },

  setSettings: async (patch) => {
    try {
      const settings = await api.updateSettings(patch);
      set((s) => ({ settings: s.settings ? { ...s.settings, ...settings } : settings }));
    } catch (e) {
      void api.log(`setSettings FAILED: ${errMessage(e)}`);
    }
  },

  playRecording: (rec) => {
    const wsId = get().settings?.activeWorkspaceId ?? "";
    set({
      player: {
        recordingId: rec.id,
        name: rec.name,
        wsId,
        file: rec.file,
        duration: rec.durationMs / 1000,
      },
      playerPlaying: true,
      playerTime: 0,
      playerDuration: rec.durationMs / 1000,
      playerSeekRequest: null,
    });
  },

  togglePlayer: () => {
    const { player, playerPlaying, playerTime, playerDuration } = get();
    if (!player) return;
    if (!playerPlaying && playerDuration > 0 && playerTime >= playerDuration - 0.5) {
      // Ended track: restart from the beginning instead of stalling at the end.
      set({ playerPlaying: true, playerSeekRequest: 0 });
    } else {
      set({ playerPlaying: !playerPlaying });
    }
  },

  stopPlayer: () =>
    set({
      player: null,
      playerPlaying: false,
      playerTime: 0,
      playerDuration: 0,
      playerSeekRequest: null,
    }),

  requestPlayerSeek: (seconds) => {
    if (!get().player) return;
    // Optimistic: the row's waveform follows the drag immediately; the
    // audio element converges when it applies the request.
    set({ playerSeekRequest: Math.max(0, seconds), playerTime: Math.max(0, seconds) });
  },

  skipPlayer: (deltaSeconds) => {
    if (!get().player) return;
    set({ playerSeekRequest: Math.max(0, get().playerTime + deltaSeconds) });
  },

  setPlayerScrubbing: (scrubbing: boolean) => set({ playerScrubbing: scrubbing }),

  reportPlayerProgress: (time, duration, playing) => {
    // During a waveform drag the optimistic scrub position is the truth;
    // reports from the audio element (still at the pre-seek position) lose.
    if (get().playerScrubbing) return;
    set({ playerTime: time, playerDuration: duration, playerPlaying: playing });
  },
}));
