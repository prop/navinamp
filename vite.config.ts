import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// https://vite.dev/config/
export default defineConfig({
  plugins: [svelte()],

  // Tauri expects a fixed port; fail if that port is not available.
  server: {
    port: 1420,
    strictPort: true,
  },

  // Avoid Vite clearing the Rust build output in the terminal.
  clearScreen: false,
});
