import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

// Tauri serves the interface from this dev server (tauri.conf.json `devUrl`) and embeds `dist/`
// in the desktop build (`frontendDist`). The port is fixed: Tauri cannot follow a port Vite picks.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    // The Rust side is watched by Tauri, not by Vite.
    watch: { ignored: ["**/src-tauri/**"] },
  },
  build: {
    // WebKitGTK 2.44 (Ubuntu 24.04), WebView2 and WKWebView all run ES2022.
    target: "es2022",
    sourcemap: true,
  },
  test: {
    environment: "jsdom",
    setupFiles: ["./src/test-setup.ts"],
    include: ["src/**/*.test.{ts,tsx}"],
    // The tests of the whole window click through dialogs and wait for frames: a few seconds each
    // when every file runs at once, more on a shared runner. A test that waits for ever still
    // fails, after 20 s.
    testTimeout: 20_000,
  },
});
