import { useCallback, useRef, useState } from "react";

import { api, extFromMimeType } from "@/lib/api";
import { usePocket } from "@/store";
import { toast } from "@/components/ui/toast";
import type { ItemImage } from "@/types";

/**
 * Uploads are unlimited — the display cap lives in NoteImages (three thumbs +
 * a dark "+N" overflow badge on the third).
 */

/** Detect an image File by MIME type or extension fallback. */
export function isImageFile(file: File): boolean {
  if (file.type.startsWith("image/")) return true;
  return /\.(png|jpe?g|gif|webp|bmp|avif)$/i.test(file.name);
}

/**
 * Public surface of `useImageStaging` (staging state lives in MainWindow and
 * is shared down to AddBar).
 */
export interface StagingApi {
  staged: ItemImage[];
  setStaged: React.Dispatch<React.SetStateAction<ItemImage[]>>;
  addFiles: (files: readonly File[]) => Promise<void>;
  reset: () => void;
  busy: boolean;
  /** Open the OS file picker; picked images are staged automatically. */
  pickFiles: () => void;
  /** Hidden file input backing `pickFiles` — render it once, anywhere. */
  fileInput: React.ReactNode;
}

/**
 * Shared staging pipeline for drag & drop and clipboard paste: each image
 * file is saved into the workspace images directory immediately and returns
 * its metadata row, ready to attach when the note is created or updated.
 */
export function useImageStaging(): StagingApi {
  const [staged, setStaged] = useState<ItemImage[]>([]);
  const [busy, setBusy] = useState(false);
  const requestId = useRef(0);
  const fileInputRef = useRef<HTMLInputElement>(null);

  const addFiles = useCallback(
    async (files: readonly File[]) => {
      const images = files.filter(isImageFile);
      if (images.length === 0) return;
      const wsId = usePocket.getState().settings?.activeWorkspaceId ?? null;
      if (!wsId) return;

      const ticket = ++requestId.current;
      setBusy(true);
      try {
        const saved: ItemImage[] = [];
        for (const file of images) {
          const bytes = await file.arrayBuffer();
          const ext = extFromMimeType(file.type);
          const image = await api.saveImage(wsId, ext, bytes);
          saved.push(image);
        }
        if (ticket !== requestId.current) return; // superseded by a reset()
        setStaged((current) => [...current, ...saved]);
      } catch (error) {
        void api.log(`useImageStaging addFiles FAILED: ${error}`);
        throw error;
      } finally {
        if (ticket === requestId.current) setBusy(false);
      }
    },
    []
  );

  const reset = useCallback(() => {
    requestId.current += 1;
    setStaged([]);
    setBusy(false);
  }, []);

  const pickFiles = useCallback(() => {
    fileInputRef.current?.click();
  }, []);

  const fileInput = (
    <input
      ref={fileInputRef}
      type="file"
      accept="image/*"
      multiple
      hidden
      onChange={(event) => {
        const files = Array.from(event.target.files ?? []);
        void addFiles(files).catch(() =>
          toast.add({ title: "Could not attach image", type: "error" })
        );
        event.target.value = "";
      }}
    />
  );

  return { staged, setStaged, addFiles, reset, busy, pickFiles, fileInput };
}
