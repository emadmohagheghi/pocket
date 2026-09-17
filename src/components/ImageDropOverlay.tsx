import { motion } from "motion/react";
import { ImagePlus } from "lucide-react";

/**
 * Fullscreen veil shown while image files are dragged anywhere over the
 * main window: a translucent card with the drop hint, outside the app card
 * so it covers the whole window including the rounded corners.
 */
export function ImageDropOverlay() {
  return (
    <motion.div
      initial={{ opacity: 0 }}
      animate={{ opacity: 1 }}
      exit={{ opacity: 0 }}
      transition={{ duration: 0.15 }}
      className="pointer-events-none absolute inset-0 z-50 grid place-items-center p-3"
      aria-hidden
    >
      <div className="grid h-full w-full place-items-center rounded-[36px] border-2 border-dashed border-blue-500/70 bg-blue-500/10 backdrop-blur-[2px]">
        <div className="flex items-center gap-3 rounded-full bg-background/90 px-5 py-2.5 text-sm font-medium text-foreground shadow-lg">
          <ImagePlus className="size-5 text-blue-500" />
          Drop images to attach them
        </div>
      </div>
    </motion.div>
  );
}
