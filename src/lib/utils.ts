export { cn } from "cn";

export function formatDuration(ms: number): string {
  const total = Math.floor(ms / 1000);
  const m = Math.floor(total / 60);
  const s = total % 60;
  return `${m}:${String(s).padStart(2, "0")}`;
}

export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(2)} GB`;
}

export function formatRelative(epochMs: number): string {
  const diff = Date.now() - epochMs;
  const min = Math.floor(diff / 60000);
  if (min < 1) return "just now";
  if (min < 60) return `${min}m ago`;
  const h = Math.floor(min / 60);
  if (h < 24) return `${h}h ago`;
  const d = Math.floor(h / 24);
  if (d < 7) return `${d}d ago`;
  return new Date(epochMs).toLocaleDateString();
}

/** Detects an URL-ish single-line string for auto-typing captured content. */
export function looksLikeUrl(text: string): boolean {
  const t = text.trim();
  return /^https?:\/\/\S+$/i.test(t) || /^www\.\S+\.\S{2,}\/?\S*$/i.test(t);
}

/** Inline URL detection: scheme'd or www-prefixed, and must contain a dot. */
const URL_PATTERN = /\b(?:https?:\/\/|www\.)[^\s<>"']*\.[^\s<>"']+/gi;

/** One chunk of note text: either plain, or a detected link. */
export interface TextSegment {
  text: string;
  /** The link this segment points at, or null for plain text. */
  url: string | null;
}

/** Drop trailing sentence punctuation and unmatched closing brackets. */
function trimUrlTail(raw: string): string {
  let url = raw;
  for (;;) {
    const last = url[url.length - 1];
    if (!last) break;
    if (".,;:!?".includes(last) || last === "'" || last === '"') {
      url = url.slice(0, -1);
      continue;
    }
    if (last === ")" && !url.includes("(")) {
      url = url.slice(0, -1);
      continue;
    }
    if (last === "]" && !url.includes("[")) {
      url = url.slice(0, -1);
      continue;
    }
    break;
  }
  return url;
}

/**
 * Split note text into plain-text and URL segments for rich rendering.
 * Detection is render-time only — nothing is persisted as a type.
 */
export function splitLinks(text: string): TextSegment[] {
  const segments: TextSegment[] = [];
  let last = 0;
  for (const match of text.matchAll(URL_PATTERN)) {
    const start = match.index;
    const url = trimUrlTail(match[0]);
    // Require a dot beyond the scheme/www prefix so "https://" or "www."
    // alone never turns into a link.
    const bare = url.replace(/^https?:\/\//i, "").replace(/^www\./i, "");
    if (!bare.includes(".")) continue;
    if (start > last) segments.push({ text: text.slice(last, start), url: null });
    segments.push({ text: url, url });
    last = start + url.length;
  }
  if (segments.length === 0) return [{ text, url: null }];
  if (last < text.length) segments.push({ text: text.slice(last), url: null });
  return segments;
}

/** Ensure a scheme so the backend opener (http/https only) accepts it. */
export function normalizeUrl(url: string): string {
  return /^https?:\/\//i.test(url) ? url : `https://${url}`;
}
