import { useEffect } from "react";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";

import MainWindow from "@/windows/MainWindow";
import QuickCaptureWindow from "@/windows/QuickCaptureWindow";
import HudWindow from "@/windows/HudWindow";
import ImageViewerWindow from "@/windows/ImageViewerWindow";

export default function App() {
  const label = getCurrentWebviewWindow().label;

  useEffect(() => {
    document.title = label === "quick-capture" ? "Pocket Capture" : "Pocket";
  }, [label]);

  if (label.startsWith("hud")) return <HudWindow />;
  if (label === "image-viewer") return <ImageViewerWindow />;
  return label === "quick-capture" ? <QuickCaptureWindow /> : <MainWindow />;
}
