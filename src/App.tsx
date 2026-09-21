import { lazy, useEffect } from "react";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";

// Each Tauri webview window renders exactly one of these roots, but they all
// share one bundle: every window ships every window's code. Lazy roots give
// each window only its own chunk graph (e.g. the HUD no longer downloads the
// main window, markdown rendering, the image viewer, …). The window-label
// routing itself stays in the entry chunk so routing starts instantly; a
// plain fallback covers the brief chunk load with a blank pane.
const MainWindow = lazy(() => import("@/windows/MainWindow"));
const QuickCaptureWindow = lazy(() => import("@/windows/QuickCaptureWindow"));
const HudWindow = lazy(() => import("@/windows/HudWindow"));
const ImageViewerWindow = lazy(() => import("@/windows/ImageViewerWindow"));

export default function App() {
  const label = getCurrentWebviewWindow().label;

  useEffect(() => {
    document.title = label === "quick-capture" ? "Pocket Capture" : "Pocket";
  }, [label]);

  if (label.startsWith("hud")) return <HudWindow />;
  if (label === "image-viewer") return <ImageViewerWindow />;
  return label === "quick-capture" ? (
    <QuickCaptureWindow />
  ) : (
    <MainWindow />
  );
}
