import { useCallback, useEffect, useId, useMemo, useRef, useState } from "react";
import { AnimatePresence, motion } from "motion/react";
import { Loader2, Mic, Pause, Play, Square, X } from "lucide-react";

import { usePocket } from "@/store";
import { api } from "@/lib/api";
import { openImageViewer } from "@/lib/imageViewer";
import { cn, formatDuration } from "@/lib/utils";
import { toast } from "@/components/ui/toast";
import { useRecorder } from "@/hooks/useRecorder";
import type { StagingApi } from "@/hooks/useImageStaging";
import { imageUrl } from "@/lib/api";
import type { Item, ItemImage, Recording } from "@/types";
import type { UndoStep } from "@/store";
import { ItemRow } from "@/components/ItemRow";
import { VoiceRow } from "@/components/VoiceList";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
import {
  MessageScroller,
  MessageScrollerButton,
  MessageScrollerContent,
  MessageScrollerItem,
  MessageScrollerProvider,
  MessageScrollerViewport,
} from "@/components/ui/message-scroller";

type FeedEntry =
  | { key: string; kind: "text"; item: Item }
  | { key: string; kind: "voice"; recording: Recording };

const CAPTURE_BAR_CLASS =
  "flex min-h-20 items-start gap-3 overflow-hidden rounded-[24px] border border-border/60 bg-card px-4 py-3 transition-colors focus-within:border-border";

/**
 * Live waveform while recording: consumes the recorder's per-frame mic
 * level (levelRef, 0..1) and scrolls a bar history right-to-left, like
 * Telegram's capture bar. Paused takes freeze the history.
 */
function LiveWaveform({
  levelRef,
  paused,
}: {
  levelRef: React.RefObject<number>;
  paused: boolean;
}) {
  const BARS = 48;
  const barsRef = useRef<number[]>(Array(BARS).fill(0.06));
  const [, tickState] = useState(0);

  useEffect(() => {
    let raf = 0;
    let last = 0;
    const tick = (t: number) => {
      raf = requestAnimationFrame(tick);
      if (t - last < 50) return; // 20 fps history is smooth enough
      last = t;
      const bars = barsRef.current;
      bars.push(paused ? 0.06 : (levelRef.current ?? 0));
      bars.shift();
      tickState((n) => n + 1);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [levelRef, paused]);

  return (
    <div className="flex h-5 min-w-0 flex-1 items-center gap-[2px]" aria-hidden>
      {barsRef.current.map((level, i) => (
        <span
          key={i}
          className={
            "min-w-[2px] flex-1 rounded-full " +
            (paused ? "bg-muted-foreground/30" : "bg-red-500/60")
          }
          style={{ height: `${Math.round(Math.max(0.1, level) * 100)}%` }}
        />
      ))}
    </div>
  );
}

/**
 * Single unified feed: text items and voice recordings together, oldest at
 * the top and newest at the bottom like a chat. Auto-scrolls while the
 * reader is at the live edge; a jump-to-latest button appears otherwise.
 */
export interface FeedActions {
  selectedIds: Set<string>;
  toggleSelect: (id: string) => void;
  clearSelection: () => void;
  copySelected: (asList: boolean) => void;
  toggleDoneSelected: () => void;
  requestEdit: (id: string) => void;
  requestExpand: (id: string) => void;
  mergeSelected: () => void;
  moveSelectedTo: (workspaceId: string, workspaceName: string) => void;
  deleteSelected: () => void;
  selectedTextIds: string[];
}

export function ItemList() {
  const data = usePocket((s) => s.data);
  const focusItemId = usePocket((s) => s.focusItemId);
  const setEntryDone = usePocket((s) => s.setEntryDone);
  const mergeItems = usePocket((s) => s.mergeItems);
  const recordUndo = usePocket((s) => s.recordUndo);
  const undo = usePocket((s) => s.undo);
  const wsId = usePocket((s) => s.settings?.activeWorkspaceId ?? "");

  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set());
  const [expandRequest, setExpandRequest] = useState<{ id: string; nonce: number } | null>(null);
  const [editRequest, setEditRequest] = useState<{ id: string; nonce: number } | null>(null);
  const nonce = useRef(0);

  const entries = useMemo<FeedEntry[]>(() => {
    return [
      ...(data?.items ?? []).map(
        (item): FeedEntry => ({ key: item.id, kind: "text", item })
      ),
      ...(data?.recordings ?? []).map(
        (recording): FeedEntry => ({
          key: recording.id,
          kind: "voice",
          recording,
        })
      ),
    ].sort((a, b) => {
      const aCreated = a.kind === "text" ? a.item.createdAt : a.recording.createdAt;
      const bCreated = b.kind === "text" ? b.item.createdAt : b.recording.createdAt;
      return aCreated - bCreated;
    });
  }, [data]);

  // Selection helpers ------------------------------------------------------------

  const selectedTextIds = useMemo(
    () =>
      entries
        .filter(
          (e): e is { key: string; kind: "text"; item: Item } =>
            e.kind === "text" && selectedIds.has(e.key)
        )
        .map((e) => e.item.id),
    [entries, selectedIds]
  );

  const toggleSelect = useCallback((id: string) => {
    setSelectedIds((current) => {
      const next = new Set(current);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }, []);

  const clearSelection = useCallback(() => setSelectedIds(new Set()), []);

  const copySelected = useCallback(
    (asList: boolean) => {
      const contents = entries
        .filter((e) => selectedIds.has(e.key))
        .map((e) =>
          e.kind === "text" ? e.item.content : e.recording.name + " (voice note)"
        );
      if (contents.length === 0) return;
      const text = asList
        ? contents.map((c, i) => `${i + 1}. ${c}`).join("\n")
        : contents.join("\n");
      void api
        .copyToClipboard(text)
        .then(() => {
          toast.add({
            title: asList ? "Copied as List" : "Copied",
            type: "success",
          });
        });
    },
    [entries, selectedIds]
  );

  const toggleDoneSelected = useCallback(() => {
    if (selectedIds.size === 0) return;
    const selected = entries.filter((e) => selectedIds.has(e.key));
    // Group semantics: if any selected entry is still undone, check them all;
    // only when every selected entry is done do we uncheck them all.
    const anyUndone = selected.some(
      (e) => (e.kind === "text" ? !e.item.pinned : !e.recording.pinned)
    );
    const target = anyUndone;
    const undoSteps: UndoStep[] = [];
    for (const entry of selected) {
      const done = entry.kind === "text" ? entry.item.pinned : entry.recording.pinned;
      if (done === target) continue;
      if (entry.kind === "text") {
        undoSteps.push({ type: "restoreItem", workspaceId: wsId, item: entry.item });
        setEntryDone("text", entry.item.id, target);
      } else {
        undoSteps.push({
          type: "restoreRecording",
          workspaceId: wsId,
          recording: entry.recording,
        });
        setEntryDone("voice", entry.recording.id, target);
      }
    }
    if (undoSteps.length > 0) recordUndo(undoSteps);
    toast.add({
      title: target ? "Marked as Done" : "Marked as Not Done",
      type: "success",
    });
  }, [entries, selectedIds, setEntryDone, recordUndo, wsId]);

  const requestEdit = useCallback((id: string) => {
    setEditRequest({ id, nonce: ++nonce.current });
  }, []);

  const requestExpand = useCallback((id: string) => {
    setExpandRequest({ id, nonce: ++nonce.current });
  }, []);

  const mergeSelected = useCallback(() => {
    // One atomic backend operation: text folds into the oldest note, images
    // move to it, and any embedded voice notes are released to the feed —
    // media files are never parked or trashed, so Ctrl+Z is lossless.
    // Standalone voice notes in the selection are left untouched.
    const noteIds = entries
      .filter(
        (e): e is { key: string; kind: "text"; item: Item } =>
          e.kind === "text" && selectedIds.has(e.key)
      )
      .map((e) => e.item.id);
    if (noteIds.length < 2) return;
    void mergeItems(noteIds).then((ok) => {
      if (!ok) {
        toast.add({
          title: "Merge Not Possible",
          description: "Merging would combine text, images and a voice note in one note.",
          type: "error",
        });
        return;
      }
      toast.add({ title: "Notes Merged", type: "success" });
    });
    setSelectedIds(new Set());
  }, [entries, selectedIds, mergeItems]);

  const deleteSelected = useCallback(() => {
    if (selectedIds.size === 0) return;
    const undoSteps: UndoStep[] = [];
    const itemIds: string[] = [];
    const recordingIds: string[] = [];
    for (const entry of entries) {
      if (!selectedIds.has(entry.key)) continue;
      if (entry.kind === "text") {
        undoSteps.push({ type: "restoreItem", workspaceId: wsId, item: entry.item });
        itemIds.push(entry.item.id);
      } else {
        undoSteps.push({
          type: "restoreRecording",
          workspaceId: wsId,
          recording: entry.recording,
        });
        recordingIds.push(entry.recording.id);
      }
    }
    // One bulk call: per-item deletes would round-trip and persist once per
    // entry, which visibly lags with hundreds of selected rows.
    void api
      .deleteEntriesBulk(wsId, itemIds, recordingIds)
      .then(() => {
        recordUndo(undoSteps);
        toast.add({ title: "Deleted", type: "success" });
        setSelectedIds(new Set());
      })
      .catch(() => toast.add({ title: "Delete failed", type: "error" }));
  }, [entries, selectedIds, recordUndo, wsId]);

  const moveSelectedTo = useCallback(
    (workspaceId: string, workspaceName: string) => {
      if (selectedTextIds.length === 0) return;
      void (async () => {
        const wsId = usePocket.getState().settings?.activeWorkspaceId ?? "";
        const moving = entries.filter(
          (e): e is { key: string; kind: "text"; item: Item } =>
            e.kind === "text" && selectedIds.has(e.key)
        );
        const undoSteps: UndoStep[] = [];
        for (const e of moving) {
          try {
            undoSteps.push({ type: "restoreItem", workspaceId: wsId, item: e.item });
            // move_item copies attachments (images / embedded voice) to the
            // target workspace, so media notes arrive intact.
            await api.moveItem(wsId, workspaceId, e.item.id);
            undoSteps.push({
              type: "removeItem",
              workspaceId,
              itemId: e.item.id,
            });
          } catch {
            toast.add({ title: "Move failed", type: "error" });
            return;
          }
        }
        recordUndo(undoSteps);
        toast.add({ title: `Moved to ${workspaceName}`, type: "success" });
        setSelectedIds(new Set());
      })();
    },
    [entries, selectedIds, selectedTextIds, recordUndo]
  );

  // Bulk shortcuts: active whenever a selection exists and the user is not
  // typing in a field.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement;
      if (
        target.tagName === "INPUT" ||
        target.tagName === "TEXTAREA" ||
        target.isContentEditable
      )
        return;
      const mod = e.ctrlKey || e.metaKey;
      // Physical-key matching (e.code) keeps shortcuts working on every
      // keyboard layout — on Persian/Arabic/Russian layouts e.key holds the
      // layout character (e.g. "ک"), never the Latin letter.
      // Ctrl+A toggles select-all / deselect-all; Ctrl+D always deselects.
      if (mod && !e.shiftKey && e.code === "KeyA") {
        e.preventDefault();
        if (entries.length > 0 && selectedIds.size === entries.length) {
          clearSelection();
        } else {
          setSelectedIds(new Set(entries.map((entry) => entry.key)));
        }
        return;
      }
      if (mod && !e.shiftKey && e.code === "KeyD") {
        e.preventDefault();
        clearSelection();
        return;
      }
      if (mod && !e.shiftKey && e.code === "KeyZ") {
        // Ctrl+Z works even with no selection (e.g. undoing a capture).
        e.preventDefault();
        void undo().then((did) => {
          if (did) toast.add({ title: "Undone", type: "success" });
        });
        return;
      }
      if (selectedIds.size === 0) return;
      if (mod && !e.shiftKey && e.code === "KeyC") {
        e.preventDefault();
        copySelected(false);
      } else if (mod && e.shiftKey && e.code === "KeyC") {
        e.preventDefault();
        copySelected(true);
      } else if (mod && e.shiftKey && e.code === "KeyM") {
        e.preventDefault();
        mergeSelected();
      } else if (e.code === "Space") {
        e.preventDefault();
        toggleDoneSelected();
      } else if (e.code === "Delete") {
        e.preventDefault();
        deleteSelected();
      } else if (e.code === "Enter" && selectedTextIds.length === 1) {
        e.preventDefault();
        requestEdit(selectedTextIds[0]);
      } else if (e.code === "Escape") {
        clearSelection();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [
    entries,
    selectedIds,
    selectedTextIds,
    copySelected,
    mergeSelected,
    toggleDoneSelected,
    requestEdit,
    clearSelection,
    deleteSelected,
    undo,
  ]);

  const actions: FeedActions = {
    selectedIds,
    toggleSelect,
    clearSelection,
    copySelected,
    toggleDoneSelected,
    requestEdit,
    requestExpand,
    mergeSelected,
    moveSelectedTo,
    deleteSelected,
    selectedTextIds,
  };

  const renderEntry = (entry: FeedEntry) =>
    entry.kind === "text" ? (
      <ItemRow
        item={entry.item}
        focused={entry.item.id === focusItemId}
        selected={selectedIds.has(entry.key)}
        toggleSelect={toggleSelect}
        editRequest={editRequest && editRequest.id === entry.key ? editRequest : null}
        expandRequest={expandRequest && expandRequest.id === entry.key ? expandRequest : null}
        actions={actions}
      />
    ) : (
      <VoiceRow
        recording={entry.recording}
        focused={entry.recording.id === focusItemId}
        selected={selectedIds.has(entry.key)}
        toggleSelect={toggleSelect}
        actions={actions}
      />
    );

  if (!data || entries.length === 0) return null;

  return (
    // Only the rows opt out of the background drag region (see ItemRow /
    // VoiceRow): gaps between rows and the empty space below the feed stay
    // draggable through the card container's deep region.
    <div className="flex h-full min-h-0 flex-col">
      <MessageScrollerProvider autoScroll defaultScrollPosition="end">
        <MessageScroller>
          <MessageScrollerViewport>
            <MessageScrollerContent className="px-3 py-2">
              {entries.map((entry) => (
                <MessageScrollerItem key={entry.key} messageId={entry.key}>
                  {renderEntry(entry)}
                </MessageScrollerItem>
              ))}
            </MessageScrollerContent>
          </MessageScrollerViewport>
          <MessageScrollerButton />
        </MessageScroller>
      </MessageScrollerProvider>
    </div>
  );
}

/** Pinned bottom capture bar (rendered outside the scroll flow). */
export function AddBar({ staging }: { staging: StagingApi }) {
  const createItem = usePocket((s) => s.createItem);
  const activeWorkspaceId = usePocket((s) => s.settings?.activeWorkspaceId);
  const [value, setValue] = useState("");
  const [savingVoice, setSavingVoice] = useState(false);
  const inputId = useId();
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const recorder = useRecorder();
  const voiceActive = recorder.recording || savingVoice;

  const save = async () => {
    const text = value.trim();
    if (!text && staging.staged.length === 0) {
      setValue("");
      return;
    }
    const created = await createItem(text || " ", staging.staged);
    if (created) {
      setValue("");
      staging.reset();
      textareaRef.current?.focus();
    }
  };

  const stopAndSaveVoice = async () => {
    // Optimistic flag keeps the bar in voice mode across the stop() gap.
    setSavingVoice(true);
    try {
      const result = await recorder.stop();
      if (!result || result.blob.size === 0) return;
      const buffer = await result.blob.arrayBuffer();
      const saved = await api.saveRecording(
        activeWorkspaceId ?? "",
        `Voice note ${new Date().toLocaleString()}`,
        result.durationMs,
        buffer
      );
      if (staging.staged.length > 0) {
        // Images were staged while recording: fold the voice note into an
        // image+voice note (no plain text allowed alongside both).
        const created = await createItem("", staging.staged, saved.id);
        if (created) {
          staging.reset();
          setValue("");
          textareaRef.current?.focus();
          toast.add({ title: "Voice + image note saved", type: "success" });
        }
      } else {
        usePocket
          .getState()
          .recordUndo([
            {
              type: "removeRecording",
              workspaceId: activeWorkspaceId ?? "",
              recordingId: saved.id,
            },
          ]);
      }
    } catch (error) {
      void api.log(`voice AddBar save FAILED: ${error}`);
      toast.add({ title: "Could not save recording", type: "error" });
    } finally {
      setSavingVoice(false);
    }
  };

  return (
    // Clicks in the capture bar focus the textarea instead of dragging the
    // window, so the bar opts out of the default whole-window dragging.
    <form
      data-tauri-drag-region="false"
      onSubmit={(event) => {
        event.preventDefault();
        void save();
      }}
    >
      {staging.staged.length > 0 && (
        <StagedImageStrip
          images={staging.staged}
          pending={staging.pendingCount}
          onRemove={(index) =>
            staging.setStaged((current) => current.filter((_, i) => i !== index))
          }
        />
      )}
      <div className={CAPTURE_BAR_CLASS}>
        <AnimatePresence mode="wait" initial={false}>
          {voiceActive ? (
            <motion.div
              key="voice"
              className="w-full self-center"
              initial={{ opacity: 0, scale: 0.96, filter: "blur(4px)" }}
              animate={{ opacity: 1, scale: 1, filter: "blur(0px)" }}
              exit={{ opacity: 0, scale: 0.96, filter: "blur(4px)" }}
              transition={{ duration: 0.18, ease: [0.22, 1, 0.36, 1] }}
            >
            <div className="flex w-full items-center gap-2.5">
              <span className="flex shrink-0 items-center gap-2">
                <span
                  className={
                    "size-2 rounded-full " +
                    (recorder.paused
                      ? "bg-amber-500"
                      : "bg-red-500" + (recorder.recording ? " animate-pulse" : ""))
                  }
                  aria-hidden
                />
                <span
                  className="font-mono text-xs tabular-nums text-muted-foreground"
                  aria-live="polite"
                >
                  {formatDuration(recorder.elapsedMs)}
                </span>
              </span>
              <div className="min-w-0 flex-1" />
              <motion.div
                className="flex shrink-0 items-center gap-0.5"
                initial={{ opacity: 0, scale: 0.9 }}
                animate={{ opacity: 1, scale: 1 }}
                transition={{ duration: 0.18, delay: 0.05, ease: [0.22, 1, 0.36, 1] }}
              >
                <Button
                  type="button"
                  size="icon-sm"
                  variant="ghost"
                  className="text-muted-foreground hover:text-foreground"
                  disabled={savingVoice}
                  aria-label={recorder.paused ? "Resume recording" : "Pause recording"}
                  onClick={() => (recorder.paused ? recorder.resume() : recorder.pause())}
                >
                  {recorder.paused ? <Play /> : <Pause />}
                </Button>
                <Button
                  type="button"
                  size="icon-sm"
                  className="rounded-full bg-red-500 text-white hover:bg-red-600"
                  disabled={savingVoice}
                  aria-label="Save recording"
                  onClick={() => void stopAndSaveVoice()}
                >
                  {savingVoice ? (
                    <Loader2 className="animate-spin" />
                  ) : (
                    <Square className="size-3.5 fill-current" />
                  )}
                </Button>
                <Button
                  type="button"
                  size="icon-sm"
                  variant="ghost"
                  className="text-muted-foreground hover:text-foreground"
                  disabled={savingVoice}
                  aria-label="Discard recording"
                  onClick={() => {
                    recorder.cancel();
                  }}
                >
                  <X />
                </Button>
              </motion.div>
            </div>
            <div className="mt-1 w-full">
              <LiveWaveform levelRef={recorder.levelRef} paused={recorder.paused} />
            </div>
          </motion.div>
        ) : (
            <motion.div
              key="text"
              className="flex w-full items-start gap-3"
              initial={{ opacity: 0, scale: 0.96, filter: "blur(4px)" }}
              animate={{ opacity: 1, scale: 1, filter: "blur(0px)" }}
              exit={{ opacity: 0, scale: 0.96, filter: "blur(4px)" }}
              transition={{ duration: 0.18, ease: [0.22, 1, 0.36, 1] }}
            >
            <button
              type="submit"
              aria-label="Add note"
              className="size-5 shrink-0 rounded-full border-[1.5px] border-muted-foreground/60"
            />
          {/* One control inside the label: clicking the flexible middle area
              of the bar focuses the textarea. */}
          <label htmlFor={inputId} className="block min-w-0 flex-1">
            <Textarea
              ref={textareaRef}
              id={inputId}
              value={value}
              dir="auto"
              autoComplete="off"
              onChange={(event) => setValue(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter" && !event.shiftKey) {
                  event.preventDefault();
                  void save();
                } else if (event.key === "Escape") {
                  setValue("");
                }
              }}
              onPaste={(event) => {
                // Rich paste: when the source provides an HTML flavor (browsers,
                // Word, editors), convert it to markdown with the backend's
                // engine so captured and pasted notes render identically.
                // Plain-text-only sources fall through to the default insert.
                const html = event.clipboardData.getData("text/html");
                if (!html.trim()) return;
                event.preventDefault();
                void api
                  .convertHtmlToMarkdown(html)
                  .then((md) => {
                    const insert = md || event.clipboardData.getData("text/plain");
                    if (!insert) return;
                    const target = event.currentTarget;
                    const start = target.selectionStart ?? value.length;
                    const end = target.selectionEnd ?? value.length;
                    const next = value.slice(0, start) + insert + value.slice(end);
                    setValue(next);
                    // Caret lands after the inserted markdown.
                    requestAnimationFrame(() => {
                      const pos = start + insert.length;
                      target.setSelectionRange(pos, pos);
                    });
                  });
              }}
              placeholder="Add a note or a prompt…"
              aria-label="Add a text item"
              rows={2}
              className="addbar-textarea max-h-32 min-h-0 resize-none overflow-y-auto rounded-none border-none !bg-transparent p-0 text-sm leading-5 shadow-none outline-none [overflow-wrap:anywhere]"
            />
          </label>
          <button
            type="button"
            onClick={() => {
              void recorder.start();
            }}
            aria-label="Record a voice note"
            className="mt-0 flex size-7 shrink-0 items-center justify-center rounded-full text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
          >
            <Mic className="size-4" />
          </button>
            </motion.div>
          )}
        </AnimatePresence>
      </div>
      {recorder.error && (
        <p
          key={recorder.error}
          className="pocket-shake mt-1 rounded-lg bg-destructive/10 px-3 py-1.5 text-xs text-destructive"
        >
          {recorder.error}
        </p>
      )}
    </form>
  );
}

/** Thumbs shown in the staging strip before the "+N" overlay kicks in. */
const MAX_SHOWN_STAGED = 5;

/**
 * Thumbnails of staged (not yet saved) images above the capture textarea.
 * Up to five thumbs, a dark "+N" overlay on the fifth, and clicking any
 * thumb opens the fullscreen viewer over all staged images. `pending`
 * images are still uploading — one dashed spinner tile each, so a big
 * paste shows exactly how many pictures are in flight.
 */
function StagedImageStrip({
  images,
  pending,
  onRemove,
}: {
  images: ItemImage[];
  pending: number;
  onRemove: (index: number) => void;
}) {
  const activeWorkspaceId =
    usePocket((s) => s.settings?.activeWorkspaceId) ?? "";
  const shown = images.slice(0, MAX_SHOWN_STAGED);
  const extra = images.length - shown.length;
  const openViewer = (index: number) => {
    void openImageViewer(
      images.map((img) => ({ wsId: activeWorkspaceId, file: img.file })),
      index
    ).catch(() => {
      /* viewer unavailable — ignore */
    });
  };
  return (
    <div
      className="-mx-1 mb-1.5 flex max-w-full items-center gap-1.5 overflow-x-auto px-1 py-0.5"
      data-tauri-drag-region="false"
    >
      {shown.map((image, index) => (
        <StagedThumb
          key={image.id}
          image={image}
          wsId={activeWorkspaceId}
          overflow={index === MAX_SHOWN_STAGED - 1 ? extra : 0}
          onRemove={() => onRemove(images.indexOf(image))}
          onOpen={() => openViewer(images.indexOf(image))}
        />
      ))}
      {Array.from({ length: pending }, (_, i) => (
        <div
          key={`pending-${i}`}
          className="grid size-16 shrink-0 animate-pulse place-items-center rounded-xl border border-dashed border-border/60 bg-muted/40 text-muted-foreground"
          aria-label="Attaching image"
        >
          <Loader2 className="size-4 animate-spin" />
        </div>
      ))}
    </div>
  );
}

function StagedThumb({
  image,
  wsId,
  overflow,
  onRemove,
  onOpen,
}: {
  image: ItemImage;
  wsId: string;
  overflow: number;
  onRemove: () => void;
  onOpen: () => void;
}) {
  const [landed, setLanded] = useState(false);
  const hasOverlay = overflow > 0;
  return (
    <div
      className={cn(
        "group/staged relative size-16 shrink-0 overflow-hidden rounded-xl border bg-muted/40 transition-colors duration-500",
        landed ? "border-border/60" : "border-primary/60"
      )}
    >
      <img
        src={imageUrl(wsId, image.file)}
        alt=""
        draggable={false}
        onLoad={() => setLanded(true)}
        className="size-full object-cover"
      />
      {/* Viewer opener: the whole tile clicks through to the fullscreen
          viewer; the tiny remove button stops propagation on top of it. */}
      <button
        type="button"
        onClick={onOpen}
        aria-label={
          hasOverlay
            ? `Show ${overflow + MAX_SHOWN_STAGED} attached images`
            : "Show attached image"
        }
        className="absolute inset-0 size-full cursor-pointer"
        tabIndex={-1}
      />
      <button
        type="button"
        onClick={(e) => {
          e.stopPropagation();
          onRemove();
        }}
        aria-label="Remove image"
        className="absolute right-0.5 top-0.5 z-10 grid size-5 place-items-center rounded-full bg-black/60 text-white opacity-0 transition-opacity group-hover/staged:opacity-100 focus-visible:opacity-100"
      >
        <X className="size-3" />
      </button>
      {hasOverlay && (
        <span
          className="pointer-events-none absolute inset-0 grid place-items-center bg-black/45 text-sm font-medium text-white"
          aria-hidden
        >
          +{overflow}
        </span>
      )}
    </div>
  )
}
