import { useCallback, useEffect, useState } from "react";
import { AnimatePresence, motion } from "motion/react";
import { ChevronLeft, ChevronRight, X } from "lucide-react";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";

import { imageUrl } from "@/lib/api";
import { listenViewerState, type ViewerState } from "@/lib/imageViewer";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

/**
 * Fullscreen image viewer (runs inside the dedicated `image-viewer` window).
 *
 * - Dark translucent backdrop over the desktop (the window itself is
 *   transparent; the dimming layer is drawn by this component).
 * - The focused image renders at its real pixel size (CSS px) — never
 *   upscaled; a too-big image is contained instead.
 * - Left/right arrows page images; a thumbnail slider at the bottom shows
 *   all images and jumps on click.
 */
export default function ImageViewerWindow() {
  const [state, setState] = useState<ViewerState | null>(null);
  const [shown, setShown] = useState(false);
  const index = state ? Math.min(state.index, state.images.length - 1) : 0;

  // Hide the actual window (not just the overlay): a transparent, visible
  // window would keep intercepting desktop clicks. The backend also hides
  // the window on a native close request — same convention as the HUD.
  const close = useCallback(() => {
    void getCurrentWebviewWindow().hide();
  }, []);

  const step = useCallback(
    (delta: number) => {
      setState((current) => {
        if (!current || current.images.length < 2) return current;
        const next =
          (current.index + delta + current.images.length) % current.images.length;
        return { ...current, index: next };
      });
    },
    []
  );

  useEffect(() => {
    const unlisten = listenViewerState((payload) => {
      setState(payload);
      setShown(true);
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, []);

  // Keyboard: arrows to page, Escape to close.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "ArrowLeft") step(-1);
      else if (event.key === "ArrowRight") step(1);
      else if (event.key === "Escape") close();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [step, close]);

  if (!state || state.images.length === 0) {
    return <div className="h-screen w-screen bg-transparent" />;
  }

  const current = state.images[index];

  return (
    <div className="relative h-screen w-screen overflow-hidden">
      {/* Dim, blurred backdrop over the desktop. Clicks on it close. */}
      <div
        className={cn(
          "absolute inset-0 bg-black/60 backdrop-blur-sm transition-opacity duration-200",
          shown ? "opacity-100" : "opacity-0"
        )}
        onClick={close}
        aria-hidden
      />

      {/* Close button. */}
      <Button
        type="button"
        size="icon"
        variant="ghost"
        onClick={close}
        aria-label="Close image viewer"
        className="absolute right-4 top-4 z-20 size-10 rounded-full text-white/80 hover:bg-white/10 hover:text-white"
      >
        <X />
      </Button>

      {/* Main stage: the focused image at real size (contained when larger). */}
      <div className="absolute inset-0 z-10 flex items-center justify-center p-16 pb-36">
        <AnimatePresence mode="wait" initial={false}>
          <motion.img
            key={current.file}
            src={imageUrl(current.wsId, current.file)}
            alt=""
            draggable={false}
            initial={{ opacity: 0, scale: 0.97 }}
            animate={{ opacity: 1, scale: 1 }}
            exit={{ opacity: 0, scale: 0.97 }}
            transition={{ duration: 0.15, ease: [0.22, 1, 0.36, 1] }}
            className="max-h-full max-w-full rounded-lg object-contain shadow-2xl shadow-black/60"
          />
        </AnimatePresence>
      </div>

      {/* Prev / next arrows (hidden when there is nothing to page to). */}
      {state.images.length > 1 && (
        <>
          <ViewerArrow side="left" onClick={() => step(-1)} />
          <ViewerArrow side="right" onClick={() => step(1)} />
        </>
      )}

      {/* Thumbnail slider. */}
      {state.images.length > 1 && (
        <div className="absolute inset-x-0 bottom-0 z-20 flex justify-center pb-6">
          <div
            className="flex max-w-[80vw] items-center gap-2 overflow-x-auto rounded-2xl border border-white/10 bg-black/40 p-2 backdrop-blur-md"
            data-tauri-drag-region="false"
          >
            {state.images.map((image, i) => (
              <button
                key={image.file}
                type="button"
                onClick={() => setState({ ...state, index: i })}
                aria-label={`Show image ${i + 1} of ${state.images.length}`}
                aria-current={i === index}
                className={cn(
                  "relative size-14 shrink-0 overflow-hidden rounded-lg border-2 transition-all",
                  i === index
                    ? "border-white/90"
                    : "border-transparent opacity-60 hover:opacity-100"
                )}
              >
                <img
                  src={imageUrl(image.wsId, image.file)}
                  alt=""
                  draggable={false}
                  className="size-full object-cover"
                />
              </button>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}

function ViewerArrow({
  side,
  onClick,
}: {
  side: "left" | "right";
  onClick: () => void;
}) {
  const Icon = side === "left" ? ChevronLeft : ChevronRight;
  return (
    <Button
      type="button"
      size="icon"
      variant="ghost"
      onClick={onClick}
      aria-label={side === "left" ? "Previous image" : "Next image"}
      className={cn(
        "absolute top-1/2 z-20 size-12 -translate-y-1/2 rounded-full text-white/80 hover:bg-white/10 hover:text-white",
        side === "left" ? "left-5" : "right-5"
      )}
    >
      <Icon className="size-7" />
    </Button>
  );
}
