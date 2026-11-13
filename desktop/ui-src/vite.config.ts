import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// Tauri expects a fixed dev port (see devUrl in desktop/tauri.conf.json).
export default defineConfig({
  plugins: [svelte()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: { ignored: ["**/dist/**"] },
  },
  build: {
    // WebKitGTK (Linux), WKWebView (macOS) and WebView2 (Windows).
    target: ["safari15", "chrome105"],
    sourcemap: false,
  },
});
