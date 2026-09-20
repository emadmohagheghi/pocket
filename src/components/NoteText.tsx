import { memo, useEffect, useRef, useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkBreaks from "remark-breaks";
import remarkGfm from "remark-gfm";

import { Kbd } from "@/components/ui/kbd";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { api } from "@/lib/api";
import { normalizeUrl } from "@/lib/utils";

/** macOS opens with ⌘; everything else says Ctrl. */
const MODIFIER_KEY = /Mac|iPhone|iPad/.test(navigator.userAgent)
  ? "⌘"
  : "Ctrl";

/** Hover time before the "how to open" tooltip appears. */
const TOOLTIP_DELAY_MS = 350;

/** Plugins: GFM (tables, strikethrough, task lists, autolinks) + single
 * newlines become <br> so plain multiline notes keep their visual layout. */
const REMARK_PLUGINS = [remarkGfm, remarkBreaks];

/**
 * Note text with markdown rendering. Notes store plain text (or markdown
 * produced by rich capture/paste); rendering turns headings, emphasis,
 * lists, links and GFM tables into styled elements while keeping the row
 * interactive — selection, Ctrl+Click links, and the done strike. Detection
 * is render-time only, nothing is persisted.
 */
export const NoteText = memo(function NoteText({
  text,
  done,
}: {
  text: string;
  done: boolean;
}) {
  return (
    <div className="note-md">
      <ReactMarkdown
        remarkPlugins={REMARK_PLUGINS}
        components={{
          a: (props) => <NoteLink {...props} done={done} />,
          img: () => null,
        }}
      >
        {text}
      </ReactMarkdown>
    </div>
  );
});

/**
 * A link inside a note — either an autolink detected by remark-gfm in plain
 * text or an explicit markdown `[label](url)` link.
 */
function NoteLink({
  href: mdHref,
  children,
  done,
}: {
  href?: string;
  children?: React.ReactNode;
  done: boolean;
}) {
  const [tipOpen, setTipOpen] = useState(false);
  const hoverTimer = useRef<number | undefined>(undefined);

  // A pending hover timer must not fire after unmount.
  useEffect(() => () => window.clearTimeout(hoverTimer.current), []);

  const close = () => {
    window.clearTimeout(hoverTimer.current);
    setTipOpen(false);
  };

  // Only real http(s) targets are openable (blocks javascript: etc. from
  // markdown sources). Detection links normalize their www shorthand.
  let href: string | null = null;
  if (mdHref) {
    try {
      const normalized = normalizeUrl(mdHref);
      const parsed = new URL(normalized);
      if (parsed.protocol === "http:" || parsed.protocol === "https:") {
        href = normalized;
      }
    } catch {
      href = null;
    }
  }

  return (
    <Tooltip open={href ? tipOpen : false}>
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
            if (!(event.ctrlKey || event.metaKey) || !href) return;
            event.preventDefault();
            event.stopPropagation();
            close();
            void api.openUrl(href);
          }}
          className={
            "cursor-pointer underline decoration-current/40 underline-offset-2 hover:decoration-current" +
            (done ? " text-inherit" : " text-primary")
          }
        >
          {children}
        </span>
      </TooltipTrigger>
      <TooltipContent side="top" className="px-2 py-1">
        Hold <Kbd>{MODIFIER_KEY}</Kbd> and click to open
      </TooltipContent>
    </Tooltip>
  );
}
