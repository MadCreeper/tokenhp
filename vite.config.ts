import { defineConfig } from "vite";

// Tauri serves the frontend from a fixed dev port and expects a static build.
export default defineConfig({
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    // Don't let the file watcher crawl the Rust side. `src-tauri/target` is
    // multi-GB of cargo build artifacts that churn on every rebuild; watching
    // it pins the Vite process at hundreds of % CPU. (Standard Tauri config —
    // was missing here.)
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },
  build: {
    target: "es2021",
    minify: "esbuild",
    emptyOutDir: true,
  },
});
