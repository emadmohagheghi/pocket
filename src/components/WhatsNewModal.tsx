import { AudioLines, Sparkles, Combine } from "lucide-react";

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogContent,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogMedia,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";

/**
 * One-shot "what's new" note. The backend decides eligibility (in-place
 * update + not dismissed before — see `whats_new_pending_version`) and hands
 * the version down in the initial state; dismissing records that version so
 * the modal never appears again. The highlights below describe the current
 * release; update them with each release's headline changes.
 */
export function WhatsNewModal({
  version,
  onDismiss,
}: {
  version: string;
  onDismiss: (version: string) => void;
}) {
  return (
    <AlertDialog open onOpenChange={(open) => !open && onDismiss(version)}>
      {/* No visible description: opt out of Radix's aria-describedby lookup. */}
      <AlertDialogContent aria-describedby={undefined}>
        <AlertDialogHeader>
          <AlertDialogMedia>
            <Sparkles />
          </AlertDialogMedia>
          <AlertDialogTitle>Welcome to Pocket v{version}</AlertDialogTitle>
        </AlertDialogHeader>
        <div className="flex flex-col gap-3">
          <div className="flex gap-3">
            <div className="mt-0.5 flex size-7 shrink-0 items-center justify-center rounded-md bg-muted text-muted-foreground">
              <AudioLines className="size-4" />
            </div>
            <div>
              <div className="text-sm font-medium">Smoother voice playback</div>
              <div className="text-[13px] text-muted-foreground">
                Scrub back and forth freely — seeking no longer jumps to the
                start, and deleting a playing voice now stops playback.
              </div>
            </div>
          </div>
          <div className="flex gap-3">
            <div className="mt-0.5 flex size-7 shrink-0 items-center justify-center rounded-md bg-muted text-muted-foreground">
              <Combine className="size-4" />
            </div>
            <div>
              <div className="text-sm font-medium">Lossless note merging</div>
              <div className="text-[13px] text-muted-foreground">
                Merging notes now keeps their images and releases embedded
                voices back to the feed — nothing is lost, and Ctrl+Z reverts
                the whole merge at once.
              </div>
            </div>
          </div>
        </div>
        <AlertDialogFooter>
          <AlertDialogAction onClick={() => onDismiss(version)}>
            Got it
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
