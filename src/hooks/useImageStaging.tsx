import { useCallback, useRef, useState } from "react";

import { api, extFromMimeType } from "@/lib/api";
import { usePocket } from "@/store";
import type { ItemImage } from "@/types";

/**
 * Uploads are unlimited — the display cap lives in NoteImages (three thumbs +
 * a dark "+N" overflow badge on the third). While files upload, `pendingCount`
 * tells the strip how many spinner placeholders to show, so pasting a batch
 * gives per-image feedback instead of a single silent spinner.
 */

/** Detect an image File by MIME type or extension fallback. */
export function isImageFile(file: File): boolean {
  if (file.type.startsWith("image/")) return true;
  return /\.(png|jpe?g|gif|webp|bmp|avif)$/i.test(file.name);
}

/**
 * Public surface of `useImageStaging` (staging state lives in MainWindow and
 * is shared down to AddBar). Images get in via drag & drop and Ctrl+V only.
 */
export interface StagingApi {
  staged: ItemImage[];
  setStaged: React.Dispatch<React.SetStateAction<ItemImage[]>>;
  addFiles: (files: readonly File[]) => Promise<void>;
  reset: () => void;
  busy: boolean;
  /** Images still uploading — the strip renders one spinner tile each. */
  pendingCount: number;
}

/**
 * Shared staging pipeline for drag & drop and clipboard paste: each image
 * file is saved into the workspace images directory immediately and returns
 * its metadata row, ready to attach when the note is created or updated.
 */
export function useImageStaging(): StagingApi {
  const [staged, setStaged] = useState<ItemImage[]>([]);
  const [pendingCount, setPendingCount] = useState(0);
  const requestId = useRef(0);

  const addFiles = useCallback(
    async (files: readonly File[]) => {
      const images = files.filter(isImageFile);
      if (images.length === 0) return;
      const wsId = usePocket.getState().settings?.activeWorkspaceId ?? null;
      if (!wsId) return;

      const ticket = ++requestId.current;
      setPendingCount(images.length);
      try {
        const saved: ItemImage[] = [];
        for (const file of images) {
          const bytes = await file.arrayBuffer();
          const ext = extFromMimeType(file.type);
          const image = await api.saveImage(wsId, ext, bytes);
          saved.push(image);
          if (ticket === requestId.current) setPendingCount(images.length - saved.length);
        }
        if (ticket !== requestId.current) return; // superseded by a reset()
        setPendingCount(0);
        setStaged((current) => [...current, ...saved]);
      } catch (error) {
        if (ticket === requestId.current) setPendingCount(0);
        void api.log(`useImageStaging addFiles FAILED: ${error}`);
        throw error;
      }
    },
    []
  );

  const reset = useCallback(() => {
    requestId.current += 1;
    setStaged([]);
    setPendingCount(0);
  }, []);

  return {
    staged,
    setStaged,
    addFiles,
    reset,
    busy: pendingCount > 0,
    pendingCount,
  };
}
