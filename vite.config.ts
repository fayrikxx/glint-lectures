import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  // Keep Rust compiler errors visible when running `tauri dev`.
  clearScreen: false,
  server: {
    // tauri.conf.json points devUrl at this port, so Vite must not quietly pick another one.
    port: 1420,
    strictPort: true,
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },
});
