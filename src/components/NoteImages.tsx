import { imageUrl } from "@/lib/api";
import { openImageViewer } from "@/lib/imageViewer";
import type { ItemImage } from "@/types";
import { cn } from "@/lib/utils";

/** Thumbnails shown per note before the overflow badge kicks in. */
export const MAX_SHOWN_IMAGES = 3;

/**
 * Clickable image thumbnails for a note row. Shows up to three pictures;
 * the third darkens and carries a "+N" overlay when the note holds more,
 * and every click opens the fullscreen image viewer starting at that image.
 *
 * The strip never wraps — a note with many images would otherwise grow the
 * row and break the feed's rhythm. It stays one line and scrolls sideways
 * instead (hidden scrollbar; drag or wheel to pan).
 */
export function NoteImages({
  wsId,
  images,
}: {
  wsId: string;
  images: ItemImage[];
}) {
  if (images.length === 0) return null;
  const shown = images.slice(0, MAX_SHOWN_IMAGES);
  const extra = images.length - shown.length;

  return (
    <div
      className="-mx-1 mt-1.5 flex max-w-full items-center gap-1.5 overflow-x-auto px-1 py-0.5"
      data-tauri-drag-region="false"
    >
      {shown.map((image, index) => (
        <NoteImageThumb
          key={image.id}
          wsId={wsId}
          image={image}
          allImages={images}
          index={index}
          overflow={index === MAX_SHOWN_IMAGES - 1 ? extra : 0}
        />
      ))}
    </div>
  );
}

function NoteImageThumb({
  wsId,
  image,
  allImages,
  index,
  overflow,
}: {
  wsId: string;
  image: ItemImage;
  allImages: ItemImage[];
  index: number;
  overflow: number;
}) {
  const hasOverlay = overflow > 0;
  return (
    <button
      type="button"
      className={cn(
        "group/thumb relative size-20 shrink-0 overflow-hidden rounded-xl border border-border/60 bg-muted/40 transition-colors hover:border-border focus-visible:border-ring focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50",
        hasOverlay && "cursor-pointer"
      )}
      aria-label={
        hasOverlay
          ? `Show ${allImages.length} images`
          : `Show image ${index + 1} of ${allImages.length}`
      }
      onClick={(e) => {
        e.stopPropagation();
        void openImageViewer(
          allImages.map((img) => ({ wsId, file: img.file })),
          index
        ).catch(() => {
          /* viewer unavailable — ignore */
        });
      }}
    >
      <img
        src={imageUrl(wsId, image.file)}
        alt=""
        draggable={false}
        className="size-full object-cover"
        loading="lazy"
      />
      {hasOverlay && (
        <span
          className="absolute inset-0 grid place-items-center bg-black/45 text-sm font-medium text-white transition-colors group-hover/thumb:bg-black/35"
          aria-hidden
        >
          +{overflow}
        </span>
      )}
    </button>
  );
}
