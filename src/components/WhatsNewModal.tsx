import { Sparkles, ImageIcon } from "lucide-react";

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
 * the modal never appears again.
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
        <div className="flex gap-3">
          <div className="mt-0.5 flex size-7 shrink-0 items-center justify-center rounded-md bg-muted text-muted-foreground">
            <ImageIcon className="size-4" />
          </div>
          <div>
            <div className="text-sm font-medium">Images in notes</div>
            <div className="text-[13px] text-muted-foreground">
              Attach images by dropping them into the window or with Ctrl+V.
              Images can also share a note with a voice recording.
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
