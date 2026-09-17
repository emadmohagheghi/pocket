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
      className="mt-1.5 flex flex-wrap items-center gap-1.5"
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
        "group/thumb relative size-20 overflow-hidden rounded-xl border border-border/60 bg-muted/40 transition-colors hover:border-border focus-visible:border-ring focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50",
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
