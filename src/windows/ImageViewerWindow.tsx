import { useCallback, useEffect, useState } from "react";
import { ChevronLeft, ChevronRight, X } from "lucide-react";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";

import { imageUrl } from "@/lib/api";
import { listenViewerState, type ViewerState } from "@/lib/imageViewer";
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
 *
 * Deliberately animation-free — no mount fade, no page cross-fade, no
 * backdrop fade: the next image simply replaces the previous one. On a
 * fullscreen window every one of those transitions read as lag.
 */
export default function ImageViewerWindow() {
  const [state, setState] = useState<ViewerState | null>(null);
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
        className="absolute inset-0 bg-black/60 backdrop-blur-sm"
        onClick={close}
        aria-hidden
      />

      {/* Close button. Plain button: the shared Button variant animates on
          press (translate-y), which reads as the control sinking away. */}
      <button
        type="button"
        onClick={close}
        aria-label="Close image viewer"
        className="absolute right-4 top-4 z-20 grid size-10 place-items-center rounded-full text-white/80 hover:bg-white/10 hover:text-white focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-white/40"
      >
        <X className="size-5" />
      </button>

      {/* Main stage: the focused image at real size (contained when larger). */}
      <div className="absolute inset-0 z-10 flex items-center justify-center p-16 pb-36">
        <img
          key={current.file}
          src={imageUrl(current.wsId, current.file)}
          alt=""
          draggable={false}
          className="max-h-full max-w-full rounded-lg object-contain shadow-2xl shadow-black/60"
        />
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
                  "relative size-14 shrink-0 overflow-hidden rounded-lg border-2",
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
  // Plain button again: no press animation, no size jump while clicking.
  return (
    <button
      type="button"
      onClick={onClick}
      aria-label={side === "left" ? "Previous image" : "Next image"}
      className={cn(
        "absolute top-1/2 z-20 grid size-12 -translate-y-1/2 place-items-center rounded-full text-white/80 hover:bg-white/10 hover:text-white focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-white/40",
        side === "left" ? "left-5" : "right-5"
      )}
    >
      <Icon className="size-7" />
    </button>
  );
}
