import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";
import path from "node:path";

// Standalone vitest config (run via `pnpm test`), separate from vite.config.ts
// whose Tauri-specific settings (host/port/env strictness) don't apply to unit
// tests and would break vitest's own static-serve handling.
export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
    },
  },
  test: {
    environment: "node",
    include: ["src/**/*.test.{ts,tsx}"],
    watch: false,
  },
});
