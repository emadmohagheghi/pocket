import { useEffect, useMemo, useRef, useState } from "react";

import { Kbd } from "@/components/ui/kbd";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { api } from "@/lib/api";
import { cn, normalizeUrl, splitLinks } from "@/lib/utils";

/** macOS opens with ⌘; everything else says Ctrl. */
const MODIFIER_KEY = /Mac|iPhone|iPad/.test(navigator.userAgent)
  ? "⌘"
  : "Ctrl";

/** Hover time before the "how to open" tooltip appears. */
const TOOLTIP_DELAY_MS = 350;

/**
 * Note text with inline link detection: URLs render as links that announce
 * themselves with a short-hover tooltip and open on Ctrl+Click (⌘+Click on
 * macOS). Plain clicks fall through, so text selection and row selection
 * keep working; detection is render-time only, nothing is persisted.
 */
export function NoteText({ text, done }: { text: string; done: boolean }) {
  const segments = useMemo(() => splitLinks(text), [text]);
  return (
    <>
      {segments.map((segment, i) =>
        segment.url ? (
          <NoteLink
            key={i}
            url={segment.url}
            done={done}
            // A single link spanning the whole note keeps the pre-existing
            // standalone link look; links inside larger text stay
            // persistently underlined so they are discoverable.
            standalone={segments.length === 1}
          />
        ) : (
          <span key={i}>{segment.text}</span>
        )
      )}
    </>
  );
}

/** One detected URL inside a note. */
function NoteLink({
  url,
  done,
  standalone,
}: {
  url: string;
  done: boolean;
  standalone: boolean;
}) {
  const [tipOpen, setTipOpen] = useState(false);
  const hoverTimer = useRef<number | undefined>(undefined);

  // A pending hover timer must not fire after unmount.
  useEffect(() => () => window.clearTimeout(hoverTimer.current), []);

  const close = () => {
    window.clearTimeout(hoverTimer.current);
    setTipOpen(false);
  };

  return (
    <Tooltip open={tipOpen}>
      <TooltipTrigger asChild>
        <span
          role="link"
          // Short hover announces the modifier; presses and drags (text
          // selection) never open the tooltip.
          onPointerEnter={(event) => {
            if (event.pointerType !== "mouse" || event.buttons !== 0) return;
            window.clearTimeout(hoverTimer.current);
            hoverTimer.current = window.setTimeout(
              () => setTipOpen(true),
              TOOLTIP_DELAY_MS
            );
          }}
          onPointerLeave={close}
          onPointerDown={close}
          onClick={(event) => {
            // Plain click: do nothing here, the row selects as usual.
            if (!(event.ctrlKey || event.metaKey)) return;
            event.preventDefault();
            event.stopPropagation();
            close();
            void api.openUrl(normalizeUrl(url));
          }}
          className={cn(
            "cursor-pointer underline-offset-2",
            standalone
              ? "hover:underline"
              : "underline decoration-current/40 hover:decoration-current",
            done ? "text-inherit" : "text-primary"
          )}
        >
          {url}
        </span>
      </TooltipTrigger>
      <TooltipContent side="top" className="px-2 py-1">
        Hold <Kbd>{MODIFIER_KEY}</Kbd> and click to open
      </TooltipContent>
    </Tooltip>
  );
}
