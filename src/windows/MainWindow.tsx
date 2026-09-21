import { lazy, useCallback, useEffect, useRef, useState } from "react";
import { AnimatePresence } from "motion/react";
import {
  ArrowBigUp,
  FolderOpen,
  Layers,
  MoreHorizontal,
  Download,
  Pin,
  Settings as SettingsIcon,
  X,
} from "lucide-react";

import { usePocket } from "@/store";
import { AddBar, ItemList } from "@/components/ItemList";
import { VoicePlayerEngine } from "@/components/VoiceList";

// Dialogs are chunk-split and only mounted while open: their code (a few
// hundred kB of settings + workspace UI) never loads in the common path of
// opening the app, capturing, and closing.
const SettingsDialog = lazy(() =>
  import("@/components/SettingsView").then((m) => ({ default: m.SettingsDialog }))
);
const WorkspacesDialog = lazy(() =>
  import("@/components/WorkspaceSwitcher").then((m) => ({
    default: m.WorkspacesDialog,
  }))
);
import { SearchBar } from "@/components/SearchBar";
import { ImageDropOverlay } from "@/components/ImageDropOverlay";
import { WhatsNewModal } from "@/components/WhatsNewModal";
import { useImageStaging } from "@/hooks/useImageStaging";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuShortcut,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { api } from "@/lib/api";
import { Toaster, toast } from "@/components/ui/toast";
import { applyTheme } from "@/lib/theme";

const LATEST_RELEASE_API = "https://api.github.com/repos/emadmohagheghi/Pocket/releases/latest";
/** Asset name suffixes produced by the release workflow, per platform. */
const UPDATE_ASSET_SUFFIX =
  navigator.userAgent.includes("Windows")
    ? ("x64-setup.exe" as const)
    : ("amd64.deb" as const);

/** True when `latest` (e.g. "0.2.2") is newer than `current` ("0.2.1"). */
function isNewerVersion(latest: string, current: string): boolean {
  const parse = (v: string) =>
    v.replace(/^v/, "").split(".").map((n) => parseInt(n, 10) || 0);
  const a = parse(latest);
  const b = parse(current);
  for (let i = 0; i < Math.max(a.length, b.length); i++) {
    if ((a[i] ?? 0) > (b[i] ?? 0)) return true;
    if ((a[i] ?? 0) < (b[i] ?? 0)) return false;
  }
  return false;
}

export default function MainWindow() {
  // Field selectors: keeps this window (and everything subscribed below it)
  // from re-rendering on unrelated store changes like player progress.
  const init = usePocket((s) => s.init);
  const settings = usePocket((s) => s.settings);
  const gaming = usePocket((s) => s.gaming);
  // One-shot "what's new" after an in-place update; null hides the modal.
  const [whatsNewVersion, setWhatsNewVersion] = useState<string | null>(null);
  const setSettings = usePocket((s) => s.setSettings);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [workspacesOpen, setWorkspacesOpen] = useState(false);
  const [menuOpen, setMenuOpen] = useState(false);
  const [updateAvailable, setUpdateAvailable] = useState(false);
  const [updateState, setUpdateState] = useState<"idle" | "downloading" | "done" | "failed">(
    "idle"
  );
  const searchInputRef = useRef<HTMLInputElement>(null);
  const alwaysOnTop = settings?.alwaysOnTop ?? false;

  // Window-wide image staging: dragging files anywhere over the window and
  // pasting image files (Ctrl+V) stage them into the capture bar.
  const staging = useImageStaging();
  const [dragOver, setDragOver] = useState(false);
  const dragDepth = useRef(0);
  const stageFiles = useCallback(
    (files: readonly File[]) => {
      // No success toast: the staged thumbnails appearing in the capture bar
      // are the feedback. Only failures surface as a toast.
      void staging.addFiles(files).catch((error) =>
        toast.add({
          title: error instanceof Error ? error.message : "Could not attach image",
          type: "error",
        })
      );
    },
    [staging]
  );

  // Ctrl+V paste: clipboard image files (e.g. screenshots) are staged.
  useEffect(() => {
    const onPaste = (event: ClipboardEvent) => {
      const files = Array.from(event.clipboardData?.files ?? []);
      if (files.length === 0) return;
      event.preventDefault();
      stageFiles(files);
    };
    window.addEventListener("paste", onPaste);
    return () => window.removeEventListener("paste", onPaste);
  }, [stageFiles]);

  // Overflow-menu actions, shared between the menu items and their
  // app-focused keyboard shortcuts (Ctrl+Shift+T/W/O/Q, Ctrl+, and
  // Ctrl+Shift+S for settings).
  const toggleStayOnTop = useCallback(() => {
    void setSettings({ alwaysOnTop: !alwaysOnTop }).catch(() => {});
  }, [alwaysOnTop, setSettings]);
  const openWorkspaces = useCallback(() => {
    setWorkspacesOpen(true);
  }, []);
  const openSettings = useCallback(() => {
    setSettingsOpen(true);
  }, []);
  const openDataFolder = useCallback(() => {
    void api.openDataFolder().catch(() => {});
  }, []);
  // One-click update: download the latest installer in the background, run
  // it, and let the backend exit the app. The download is small and fast
  // enough that no progress UI is needed — the menu item just flips to a
  // busy state and a toast reports the outcome.
  const installUpdate = useCallback(() => {
    if (updateState === "downloading") return;
    setUpdateState("downloading");
    void api
      .installAndLaunchUpdate()
      .then(() => setUpdateState("done"))
      .catch(() => {
        setUpdateState("failed");
        toast.add({ title: "Update failed — try again from Releases", type: "error" });
      });
  }, [updateState]);
  const closeWindow = useCallback(() => {
    void api.closeWindow().catch(() => {});
  }, []);

  // "Ctrl+Shift+" prefix rendered with the ⇧ keycap glyph.
  const modPrefix = (
    <>
      Ctrl
      <ArrowBigUp className="mx-0.5 inline size-3.5 text-muted-foreground" />
    </>
  );

  useEffect(() => {
    void init()
      .then(() => {
        api.frontendReady().catch(() => {});
        // The store seeds its state inside init(); read the pending "what's
        // new" flag straight from the source of truth afterwards.
        return api.getState();
      })
      .then((initial) => {
        if (initial.showWhatsNewFor) setWhatsNewVersion(initial.showWhatsNewFor);
      })
      .catch(() => {});
  }, [init]);

  const dismissWhatsNew = useCallback((version: string) => {
    setWhatsNewVersion(null);
    // Record the dismissal durably, best-effort: even if the IPC fails the
    // modal is closed for this session.
    api.markWhatsNewSeen(version).catch(() => {});
  }, []);

  useEffect(() => {
    if (settings) applyTheme(settings.theme);
  }, [settings?.theme]);

  // Frameless transparent window: the card below provides the background.
  useEffect(() => {
    document.body.style.background = "transparent";
  }, []);

  // One-shot update check against the GitHub releases API; a failure is
  // silently ignored (offline, rate limit, …).
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const controller = new AbortController();
        const timer = window.setTimeout(() => controller.abort(), 8000);
        const res = await fetch(LATEST_RELEASE_API, {
          signal: controller.signal,
          headers: { Accept: "application/vnd.github+json" },
        });
        window.clearTimeout(timer);
        if (!res.ok) return;
        const data = (await res.json()) as { tag_name?: string; assets?: { name: string }[] };
        const latest = data.tag_name ?? "";
        const current = __APP_VERSION__;
        // Only offer the in-app update when the latest release actually has
        // an installer for this platform; otherwise stay quiet.
        const hasInstaller = (data.assets ?? []).some((a) =>
          a.name.endsWith(UPDATE_ASSET_SUFFIX)
        );
        if (!cancelled && latest && hasInstaller && isNewerVersion(latest, current)) {
          setUpdateAvailable(true);
          toast.add({ title: "New update available", type: "success" });
        }
      } catch {
        // No update check result — keep quiet.
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement;
      const typing =
        target.tagName === "INPUT" || target.tagName === "TEXTAREA" || target.isContentEditable;
      // Physical-key matching (e.code) keeps shortcuts working on every
      // keyboard layout — on Persian/Arabic/Russian layouts e.key holds the
      // layout character (e.g. "ک"), never the Latin letter.
      if (e.ctrlKey && !e.altKey && !e.shiftKey && e.code === "KeyK") {
        // Ctrl+K — focus the persistent search field.
        e.preventDefault();
        searchInputRef.current?.focus();
        searchInputRef.current?.select();
      } else if (e.ctrlKey && !e.shiftKey && !e.altKey && e.code === "Comma") {
        // Ctrl+, — Settings (VS Code convention).
        e.preventDefault();
        openSettings();
      } else if (e.ctrlKey && e.shiftKey && !e.altKey) {
        // Overflow-menu shortcuts — same actions as the three-dot items,
        // available whenever the app window itself has keyboard focus.
        if (e.code === "KeyT") {
          e.preventDefault();
          toggleStayOnTop();
        } else if (e.code === "KeyW") {
          e.preventDefault();
          openWorkspaces();
        } else if (e.code === "KeyS") {
          e.preventDefault();
          openSettings();
        } else if (e.code === "KeyO") {
          e.preventDefault();
          openDataFolder();
        } else if (e.code === "KeyQ") {
          e.preventDefault();
          closeWindow();
        }
      } else if (e.code === "Escape" && !typing) {
        setSettingsOpen(false);
        setWorkspacesOpen(false);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [toggleStayOnTop, openWorkspaces, openSettings, openDataFolder, closeWindow]);

  return (
    <div
      className="relative h-screen bg-transparent p-3"
      onDragOver={(event) => {
        if (event.dataTransfer.types.includes("Files")) event.preventDefault();
      }}
      onDragEnter={(event) => {
        if (!event.dataTransfer.types.includes("Files")) return;
        event.preventDefault();
        dragDepth.current += 1;
        setDragOver(true);
      }}
      onDragLeave={() => {
        dragDepth.current = Math.max(0, dragDepth.current - 1);
        if (dragDepth.current === 0) setDragOver(false);
      }}
      onDrop={(event) => {
        const files = Array.from(event.dataTransfer.files ?? []);
        event.preventDefault();
        dragDepth.current = 0;
        setDragOver(false);
        if (files.length > 0) stageFiles(files);
      }}
    >
      {/* The whole window is draggable by default (see lib/drag-region.ts):
            any press outside an interactive element moves the window, so the
            app background, feed gaps and bar padding all drag. Buttons,
            fields and floating menus opt out on their own — no markers
            needed here. */}
      <div className="flex h-full flex-col overflow-hidden rounded-[36px] border border-border/60 bg-background">
        {/* Top bar: search + overflow menu. */}
        <div className="flex items-center gap-2 px-3 pt-3">
          <SearchBar inputRef={searchInputRef} />
          <DropdownMenu open={menuOpen} onOpenChange={setMenuOpen}>
            <DropdownMenuTrigger asChild>
              <Button
                size="icon"
                variant="ghost"
                className="relative size-10 shrink-0 rounded-[24px] border border-border/60 bg-card text-muted-foreground hover:bg-card! hover:text-muted-foreground active:bg-card! aria-expanded:bg-card! aria-expanded:text-muted-foreground!"
                aria-label="More options"
              >
                <MoreHorizontal />
                {updateAvailable && (
                  <span
                    className="absolute right-2 top-2 size-2 rounded-full bg-orange-500"
                    aria-hidden
                  />
                )}
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end" className="w-56">
              <DropdownMenuGroup>
                <DropdownMenuCheckboxItem
                  checked={alwaysOnTop}
                  onCheckedChange={(checked) =>
                    void setSettings({ alwaysOnTop: checked === true }).catch(() => {})
                  }
                >
                  <Pin /> Stay on top
                  <DropdownMenuShortcut>{modPrefix}+T</DropdownMenuShortcut>
                </DropdownMenuCheckboxItem>
              </DropdownMenuGroup>
              <DropdownMenuSeparator />
              <DropdownMenuGroup>
                <DropdownMenuItem
                  onClick={openWorkspaces}
                >
                  <Layers /> Workspaces
                  <DropdownMenuShortcut>{modPrefix}+W</DropdownMenuShortcut>
                </DropdownMenuItem>
                <DropdownMenuItem
                  onClick={openSettings}
                >
                  <SettingsIcon /> Settings
                  <DropdownMenuShortcut>Ctrl+,</DropdownMenuShortcut>
                </DropdownMenuItem>
              </DropdownMenuGroup>
              <DropdownMenuSeparator />
              <DropdownMenuGroup>
                {updateAvailable && (
                  <DropdownMenuItem
                    className="whitespace-nowrap text-orange-600 dark:text-orange-400"
                    disabled={updateState === "downloading"}
                    // Stay open while downloading: the item itself shows the
                    // progress state ("Downloading…"), so closing the menu
                    // would hide the only feedback the user gets.
                    onSelect={(e) => {
                      e.preventDefault();
                      installUpdate();
                    }}
                  >
                    <Download /> {updateState === "downloading" ? "Downloading…" : "New update available"}
                  </DropdownMenuItem>
                )}
                <DropdownMenuItem
                  className="whitespace-nowrap"
                  onClick={openDataFolder}
                >
                  <FolderOpen /> Open data folder
                  <DropdownMenuShortcut>{modPrefix}+O</DropdownMenuShortcut>
                </DropdownMenuItem>
              </DropdownMenuGroup>
              <DropdownMenuSeparator />
              <DropdownMenuGroup>
                <DropdownMenuItem
                  className="whitespace-nowrap"
                  onClick={closeWindow}
                >
                  <X /> Close Window
                  <DropdownMenuShortcut>{modPrefix}+Q</DropdownMenuShortcut>
                </DropdownMenuItem>
              </DropdownMenuGroup>
            </DropdownMenuContent>
          </DropdownMenu>
        </div>

        {/* Gaming-mode banner. */}
        {gaming && (
          <div className="px-3 pt-2">
            <div className="rounded-xl bg-orange-500/10 px-3 py-2 text-[11px] leading-snug text-orange-600 dark:text-orange-400">
              <span className="font-medium">Gaming mode active</span> — quick capture is
              disabled.
            </div>
          </div>
        )}

        {/* Unified feed (MessageScroller handles scrolling/auto-scroll). */}
        <div className="min-h-0 flex-1">
          <ItemList />
        </div>

        {/* Invisible audio engine; playback UI lives in the voice rows. */}
        <VoicePlayerEngine />
        <div className="shrink-0 px-3 pb-3 pt-3">
          <AddBar staging={staging} />
        </div>
      </div>

      {settingsOpen && <SettingsDialog open onClose={() => setSettingsOpen(false)} />}
      {workspacesOpen && (
        <WorkspacesDialog open onClose={() => setWorkspacesOpen(false)} />
      )}
      {whatsNewVersion && (
        <WhatsNewModal version={whatsNewVersion} onDismiss={dismissWhatsNew} />
      )}
      <AnimatePresence>
        {dragOver && <ImageDropOverlay />}
      </AnimatePresence>
      <Toaster />
    </div>
  );
}
