import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// `clearScreen: false` keeps cargo's own output visible during `tauri dev`;
// the fixed port matches `devUrl` in tauri.conf.json, and `strictPort` makes a
// clash fail loudly rather than silently serving somewhere Tauri is not looking.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: { host: "127.0.0.1", port: 5273, strictPort: true },
  build: { outDir: "dist", emptyOutDir: true, target: "chrome110" },
});
