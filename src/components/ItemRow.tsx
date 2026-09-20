import { useEffect, useLayoutEffect, useRef, useState } from "react";
import {
  Check,
  ChevronDown,
  Copy,
  FolderInput,
  List,
  Merge,
  Pencil,
  StretchHorizontal,
  Trash2,
} from "lucide-react";

import { usePocket } from "@/store";
import type { FeedActions } from "@/components/ItemList";
import { NoteImages } from "@/components/NoteImages";
import { NoteText } from "@/components/NoteText";
import { NoteVoicePlayer } from "@/components/VoiceList";
import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuShortcut,
  ContextMenuSub,
  ContextMenuSubContent,
  ContextMenuSubTrigger,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import { Textarea } from "@/components/ui/textarea";
import type { Item } from "@/types";

interface Props {
  item: Item;
  focused: boolean;
  selected: boolean;
  toggleSelect: (id: string) => void;
  editRequest: { id: string; nonce: number } | null;
  expandRequest: { id: string; nonce: number } | null;
  actions: FeedActions;
}

export function ItemRow({
  item,
  focused,
  selected,
  toggleSelect,
  editRequest,
  expandRequest,
  actions,
}: Props) {
  // Field selectors: rows must not re-render on unrelated store traffic such
  // as voice-player progress while a recording plays.
  const updateItem = usePocket((s) => s.updateItem);
  const setEntryDone = usePocket((s) => s.setEntryDone);
  const clearEditRequest = usePocket((s) => s.clearEditRequest);
  const workspaces = usePocket((s) => s.workspaces);
  const activeWorkspaceId = usePocket((s) => s.settings?.activeWorkspaceId);
  const previewLineLimit = usePocket((s) => s.settings?.notePreviewLines ?? 5);
  const [menuOpen, setMenuOpen] = useState(false);
  const [isExpandable, setIsExpandable] = useState(false);
  const [expanded, setExpanded] = useState(false);
  const [localEditing, setLocalEditing] = useState(false);
  const [draft, setDraft] = useState(item.content);
  const rowRef = useRef<HTMLLIElement>(null);
  const previewRef = useRef<HTMLDivElement>(null);
  const editRef = useRef<HTMLTextAreaElement>(null);
  const collapseEnabled = previewLineLimit > 0;
  const canCollapse = collapseEnabled && isExpandable;
  const previewStyle = collapseEnabled
    ? { WebkitLineClamp: previewLineLimit }
    : undefined;

  // An outstanding edit request (from search or the bulk Enter shortcut)
  // drives editing by derivation — the row is editing while its request is
  // open.
  const editing = localEditing || editRequest !== null;
  const done = item.pinned;

  useEffect(() => {
    if (focused) {
      rowRef.current?.scrollIntoView({ block: "center" });
      usePocket.getState().setFocusItem(null);
    }
  }, [focused]);

  useEffect(() => {
    if (!editing) return;
    setDraft(item.content);
    // Radix restores focus to the card after the context menu closes —
    // after our first frame. Keep claiming focus until the field has it,
    // caret at the end, or entering edit from the menu appears dead.
    const focus = () => {
      if (document.activeElement === editRef.current) return true;
      editRef.current?.focus();
      editRef.current?.setSelectionRange(item.content.length, item.content.length);
      return document.activeElement === editRef.current;
    };
    let raf = 0;
    const tries = [0, 50, 150].map((delay) =>
      window.setTimeout(() => {
        if (focus()) return;
        if (delay === 0) raf = requestAnimationFrame(() => void focus());
      }, delay)
    );
    return () => {
      tries.forEach(clearTimeout);
      cancelAnimationFrame(raf);
    };
  }, [editing, item.content]);

  useLayoutEffect(() => {
    if (!collapseEnabled) {
      setIsExpandable(false);
      setExpanded(false);
      return;
    }

    const preview = previewRef.current;
    if (!preview) return;

    let cancelled = false;
    const measure = () => {
      if (cancelled) return;
      const overflows = preview.scrollHeight > preview.clientHeight + 1;
      setIsExpandable((current) => (current === overflows ? current : overflows));
      if (!overflows) setExpanded(false);
    };

    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(preview);
    void document.fonts.ready.then(measure);

    return () => {
      cancelled = true;
      observer.disconnect();
    };
  }, [collapseEnabled, editing, isExpandable, item.content, previewLineLimit]);

  useEffect(() => {
    if (!expanded || editing) return;

    const collapseOnOutsideClick = (event: PointerEvent) => {
      if (!rowRef.current?.contains(event.target as Node)) {
        setExpanded(false);
      }
    };
    const collapseOnWindowBlur = () => setExpanded(false);

    document.addEventListener("pointerdown", collapseOnOutsideClick);
    window.addEventListener("blur", collapseOnWindowBlur);
    return () => {
      document.removeEventListener("pointerdown", collapseOnOutsideClick);
      window.removeEventListener("blur", collapseOnWindowBlur);
    };
  }, [editing, expanded]);

  const editStartedAt = useRef(0);
  useEffect(() => {
    if (editing) editStartedAt.current = Date.now();
  }, [editing]);

  // Bulk Expand action (single request per selection change).
  useEffect(() => {
    if (expandRequest) setExpanded(true);
  }, [expandRequest]);

  const endEditing = () => {
    setLocalEditing(false);
    if (editRequest) actions.requestEdit("__cancel__");
    clearEditRequest();
  };

  const saveEdit = async () => {
    const content = draft.trim();
    if (content && content !== item.content) {
      await updateItem(item.id, { content });
    }
    endEditing();
  };

  const onKeyDownRow = (e: React.KeyboardEvent) => {
    if (
      editing ||
      (e.target as HTMLElement).closest("button, textarea, input, a")
    ) return;
    if (e.key === "Enter" && canCollapse) {
      e.preventDefault();
      setExpanded((current) => !current);
    } else if (e.key === "Escape") {
      e.preventDefault();
      setExpanded(false);
      rowRef.current?.blur();
    }
  };

  // One shared editing field: the whole card becomes the textarea.
  const editField = (
    <Textarea
      ref={editRef}
      dir="auto"
      autoFocus
      value={draft}
      rows={1}
      className="field-sizing-content max-h-64 w-full resize-none border-none bg-transparent p-0 text-sm leading-snug shadow-none outline-none [overflow-wrap:anywhere]"
      onChange={(e) => setDraft(e.target.value)}
      onKeyDown={(e) => {
        // Enter commits, Shift+Enter is a newline, Escape cancels.
        if (e.key === "Enter" && !e.shiftKey) {
          e.preventDefault();
          void saveEdit();
        } else if (e.key === "Escape") {
          endEditing();
        }
      }}
      // Guarded blur-save: the context menu's close shuffles focus through
      // the card right after Edit is chosen, which must not close the
      // editor (and looks like Edit doing nothing).
      onBlur={() => {
        if (Date.now() - editStartedAt.current > 250) void saveEdit();
      }}
    />
  );

  /** Right-click selects the card first, so every menu action can operate
      on the whole selection (single click-select + right-click = same). */
  const ensureSelected = () => {
    if (!selected) toggleSelect(item.id);
  };

  const { deleteSelected } = actions;

  const contextMenu = (
    <ContextMenuContent>
      <ContextMenuItem
        onSelect={() => {
          ensureSelected();
          actions.copySelected(false);
        }}
      >
        <Copy /> Copy
        <ContextMenuShortcut>⌃C</ContextMenuShortcut>
      </ContextMenuItem>
      <ContextMenuItem
        onSelect={() => {
          ensureSelected();
          actions.copySelected(true);
        }}
      >
        <List /> Copy as List
        <ContextMenuShortcut>⇧⌃C</ContextMenuShortcut>
      </ContextMenuItem>
      <ContextMenuItem
        onSelect={() => {
          ensureSelected();
          actions.toggleDoneSelected();
        }}
      >
        <Check /> {done ? "Mark as Not Done" : "Mark as Done"}
        <ContextMenuShortcut>Space</ContextMenuShortcut>
      </ContextMenuItem>
      {canCollapse && !expanded ? (
        <ContextMenuItem onSelect={() => actions.requestExpand(item.id)}>
          <StretchHorizontal /> Expand
        </ContextMenuItem>
      ) : null}
      <ContextMenuSeparator />
      <ContextMenuItem
        onSelect={() => {
          ensureSelected();
          if (actions.selectedTextIds.length === 1) {
            actions.requestEdit(actions.selectedTextIds[0]);
          } else {
            setLocalEditing(true);
          }
        }}
      >
        <Pencil /> Edit
        <ContextMenuShortcut>⏎</ContextMenuShortcut>
      </ContextMenuItem>
      <ContextMenuItem
        disabled={actions.selectedTextIds.length < 2}
        onSelect={() => {
          actions.mergeSelected();
        }}
      >
        <Merge /> Merge Notes
        <ContextMenuShortcut>⇧⌃M</ContextMenuShortcut>
      </ContextMenuItem>
      {workspaces.length > 1 ? (
        <ContextMenuSub>
          <ContextMenuSubTrigger disabled={actions.selectedTextIds.length === 0}>
            <FolderInput /> Move to
          </ContextMenuSubTrigger>
          <ContextMenuSubContent>
            {workspaces
              .filter((w) => w.id !== activeWorkspaceId)
              .map((w) => (
                <ContextMenuItem key={w.id} onSelect={() => actions.moveSelectedTo(w.id, w.name)}>
                  {w.name}
                </ContextMenuItem>
              ))}
          </ContextMenuSubContent>
        </ContextMenuSub>
      ) : null}
      <ContextMenuSeparator />
      <ContextMenuItem variant="destructive" onSelect={deleteSelected}>
        <Trash2 /> Delete
      </ContextMenuItem>
    </ContextMenuContent>
  );

  return (
    <ContextMenu onOpenChange={(open) => setMenuOpen(open)}>
      <ContextMenuTrigger asChild>
        {/* Rows opt out of whole-window dragging: plain click selects,
            text is selectable, and edit fields live here. */}
        <li
          ref={rowRef}
          tabIndex={0}
          data-tauri-drag-region="false"
          data-item-id={item.id}
          onKeyDown={onKeyDownRow}
          onClick={(e) => {
            // Plain click selects (todo-style); interactive children opt out.
            if (editing) return;
            if ((e.target as HTMLElement).closest("button, textarea, input, a")) return;
            toggleSelect(item.id);
          }}
          onContextMenu={() => {
            if (!selected) toggleSelect(item.id);
          }}
          className={
            "group flex items-start gap-3 rounded-[24px] border bg-card px-3 py-2.5 transition-colors " +
            (menuOpen || selected
              ? "border-blue-500"
              : "border-border/60 hover:border-border")
          }
        >
          <button
            type="button"
            role="checkbox"
            aria-checked={done}
            aria-label={done ? "Mark as not done" : "Mark as done"}
            onClick={(e) => {
              e.stopPropagation();
              void setEntryDone("text", item.id, !done);
            }}
            className="t-check flex size-5 shrink-0 items-center justify-center rounded-full border-[1.5px]"
          >
            <svg
              viewBox="0 0 10.1668 10.1668"
              className="size-2"
              fill="none"
              stroke="currentColor"
              strokeWidth={2.2}
              strokeLinecap="round"
              strokeLinejoin="round"
              aria-hidden
            >
              <path d="M1 5.52L3.92 9.17L9.17 1" />
            </svg>
          </button>
          {editing ? (
            <div className="min-w-0 flex-1">{editField}</div>
          ) : (
            <ItemBody
              item={item}
              done={done}
              canCollapse={canCollapse}
              collapseEnabled={collapseEnabled}
              previewStyle={previewStyle}
              expanded={expanded}
              setExpanded={setExpanded}
              previewRef={previewRef}
              wsId={activeWorkspaceId ?? ""}
            />
          )}
        </li>
      </ContextMenuTrigger>
      {contextMenu}
    </ContextMenu>
  );
}

/** Note content: collapsible preview/full branches. */
function ItemBody({
  item,
  done,
  canCollapse,
  collapseEnabled,
  previewStyle,
  expanded,
  setExpanded,
  previewRef,
  wsId,
}: {
  item: Item;
  done: boolean;
  canCollapse: boolean;
  collapseEnabled: boolean;
  previewStyle: { WebkitLineClamp: number } | undefined;
  expanded: boolean;
  setExpanded: (value: boolean | ((current: boolean) => boolean)) => void;
  previewRef: React.RefObject<HTMLDivElement | null>;
  wsId: string;
}) {
  const open = expanded;

  return (
    <div className="min-w-0 flex-1">
      {item.content.trim().length > 0 ? (
        canCollapse ? (
          <CollapsibleItemBody
            item={item}
            done={done}
            open={open}
            setExpanded={setExpanded}
            previewStyle={previewStyle}
            previewRef={previewRef}
          />
        ) : (
          <div
            ref={previewRef}
            dir="auto"
            style={previewStyle}
            className={cn(
              "whitespace-pre-wrap text-sm font-normal leading-5 text-foreground [overflow-wrap:anywhere]",
              collapseEnabled && "overflow-hidden [display:-webkit-box] [-webkit-box-orient:vertical]",
              done && "text-muted-foreground line-through"
            )}
          >
            <NoteText text={item.content} done={done} />
          </div>
        )
      ) : null}
      {item.recording && (
        <NoteVoicePlayer wsId={wsId} recording={item.recording} />
      )}
      <NoteImages wsId={wsId} images={item.images} />
    </div>
  );
}

/** The collapsible preview/full layout for rows over the line limit. */
function CollapsibleItemBody({
  item,
  done,
  open,
  setExpanded,
  previewStyle,
  previewRef,
}: {
  item: Item;
  done: boolean;
  open: boolean;
  setExpanded: (value: boolean | ((current: boolean) => boolean)) => void;
  previewStyle: { WebkitLineClamp: number } | undefined;
  previewRef: React.RefObject<HTMLDivElement | null>;
}) {
  return (
    <Collapsible open={open} onOpenChange={setExpanded}>
      <div className="grid min-w-0">
        <div
          ref={previewRef}
          dir="auto"
          aria-hidden={open}
          style={previewStyle}
          className={cn(
            "col-start-1 row-start-1 self-start overflow-hidden whitespace-pre-wrap text-sm font-normal leading-5 text-foreground [display:-webkit-box] [-webkit-box-orient:vertical] [overflow-wrap:anywhere] transition-opacity duration-150",
            open && "pointer-events-none opacity-0",
            done && "text-muted-foreground line-through"
          )}
        >
          <NoteText text={item.content} done={done} />
        </div>

        <CollapsibleContent
          aria-hidden={!open}
          className="col-start-1 row-start-1 min-h-0 min-w-0 self-start overflow-hidden data-[state=closed]:pointer-events-none data-[state=closed]:animate-[pocket-collapsible-up_180ms_ease-in] data-[state=open]:animate-[pocket-collapsible-down_220ms_ease-out] motion-reduce:animate-none"
        >
          <div
            dir="auto"
            className={cn(
              "whitespace-pre-wrap text-sm font-normal leading-5 text-foreground [overflow-wrap:anywhere]",
              done && "text-muted-foreground line-through"
            )}
          >
            <NoteText text={item.content} done={done} />
          </div>
        </CollapsibleContent>
      </div>

      {
        <div className="mt-2 flex min-h-6 items-center justify-end gap-1.5">
          <CollapsibleTrigger asChild>
            <Button
              type="button"
              size="xs"
              variant="ghost"
              className="h-5 px-1.5 text-[11px] text-muted-foreground"
            >
              <ChevronDown
                data-icon="inline-start"
                className={cn(
                  "transition-transform duration-200 ease-[cubic-bezier(0.22,1,0.36,1)]",
                  open && "-scale-y-100"
                )}
              />
              {open ? "Show less" : "Show more"}
            </Button>
          </CollapsibleTrigger>
        </div>
      }
    </Collapsible>
  );
}
