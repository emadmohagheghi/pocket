/**
 * Image viewer window coordination (main → image-viewer).
 *
 * The viewer runs in its own Tauri window (`image-viewer`, declared in
 * tauri.conf.json and created at startup, hidden) so it can be genuinely
 * fullscreen-sized — the main window is a fixed 400px card. State flows one
 * way over the Tauri event bus: the main window emits `image-viewer://state`,
 * then shows the already-existing viewer window.
 */

import { invoke } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";

import { api } from "@/lib/api";

export const VIEWER_STATE_EVENT = "image-viewer://state";

/** Tauri label of the pre-created, hidden viewer window. */
const VIEWER_LABEL = "image-viewer";

/** One image the viewer can page through. */
export interface ViewerImage {
  wsId: string;
  file: string;
}

/** Full viewer state — the images plus which one is selected. */
export interface ViewerState {
  images: ViewerImage[];
  index: number;
}

/** True in the image-viewer webview itself. */
export function isViewerWindow(): boolean {
  return getCurrentWebviewWindow().label === "image-viewer";
}

/** Open the viewer window showing `images`, starting at `index`. */
export async function openImageViewer(
  images: ViewerImage[],
  index: number
): Promise<void> {
  if (images.length === 0) return;
  const clamped = Math.min(Math.max(index, 0), images.length - 1);
  try {
    await emit(VIEWER_STATE_EVENT, {
      images,
      index: clamped,
    } satisfies ViewerState);
    // The viewer window exists from app startup (hidden); reveal it in
    // native fullscreen so it covers everything — the taskbar too (maximize
    // would leave the taskbar visible). Raw plugin commands with an
    // explicit label: `Window.getByLabel` is broken here —
    // `get_all_windows` returns plain label strings in this Tauri version,
    // so `getByLabel` never matches and returns null.
    await invoke("plugin:window|show", { label: VIEWER_LABEL });
    await invoke("plugin:window|unminimize", { label: VIEWER_LABEL });
    await invoke("plugin:window|set_fullscreen", {
      label: VIEWER_LABEL,
      value: true,
    });
    await invoke("plugin:window|set_focus", { label: VIEWER_LABEL });
  } catch (error) {
    void api.log(`openImageViewer FAILED: ${error}`);
  }
}

/** Subscribe to viewer state pushes (used by the viewer window itself). */
export function listenViewerState(
  handler: (state: ViewerState) => void
): Promise<() => void> {
  return listen<ViewerState>(VIEWER_STATE_EVENT, (event) =>
    handler(event.payload)
  );
}
