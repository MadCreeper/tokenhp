import { defineConfig } from "vite";

// Tauri serves the frontend from a fixed dev port and expects a static build.
export default defineConfig({
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
  },
  build: {
    target: "es2021",
    minify: "esbuild",
    emptyOutDir: true,
  },
});
