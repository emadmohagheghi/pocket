import { ImageIcon, Sparkles, type LucideIcon } from "lucide-react";

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
 *
 * Releases without headline-worthy changes list their version here so their
 * update launches show nothing at all (the backend marks the version seen
 * regardless). Add an entry WHEN a release deserves the one-shot note.
 */
const HIGHLIGHTS: Record<string, { icon: LucideIcon; title: string; text: string }[]> = {
  "0.2.5": [
    {
      icon: ImageIcon,
      title: "Images in notes",
      text: "Attach images by dropping them into the window or with Ctrl+V. Images can also share a note with a voice recording.",
    },
  ],
};

export function WhatsNewModal({
  version,
  onDismiss,
}: {
  version: string;
  onDismiss: (version: string) => void;
}) {
  const highlights = HIGHLIGHTS[version] ?? [];
  // Version without curated highlights: the modal has nothing to say.
  // Dismiss immediately so the version is marked seen and future launches
  // stay quiet.
  if (highlights.length === 0) {
    onDismiss(version);
    return null;
  }
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
          {highlights.map(({ icon: Icon, title, text }) => (
            <div key={title} className="flex gap-3">
              <div className="mt-0.5 flex size-7 shrink-0 items-center justify-center rounded-md bg-muted text-muted-foreground">
                <Icon className="size-4" />
              </div>
              <div>
                <div className="text-sm font-medium">{title}</div>
                <div className="text-[13px] text-muted-foreground">{text}</div>
              </div>
            </div>
          ))}
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
