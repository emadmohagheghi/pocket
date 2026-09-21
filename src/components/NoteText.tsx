import { lazy, memo, Suspense } from "react";

/**
 * Markdown rendering lives in NoteTextImpl behind a lazy chunk boundary: the
 * markdown stack is the heaviest dependency in the app and the feed also
 * serves plain-text notes. Until the chunk loads (one-time, on first note
 * render) the raw text shows with the same whitespace styling so content is
 * never blank.
 */
const MarkdownImpl = lazy(() => import("@/components/NoteTextImpl"));

export const NoteText = memo(function NoteText({
  text,
  done,
}: {
  text: string;
  done: boolean;
}) {
  return (
    <Suspense
      fallback={
        <span
          dir="auto"
          className="whitespace-pre-wrap [overflow-wrap:anywhere]"
        >
          {text}
        </span>
      }
    >
      <MarkdownImpl text={text} done={done} />
    </Suspense>
  );
});
