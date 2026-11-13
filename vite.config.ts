import { execSync } from "node:child_process";
import { readFileSync } from "node:fs";

import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

/** Short commit of the checkout this bundle was built from. */
function gitCommit(): string {
  try {
    return execSync("git rev-parse --short HEAD", { encoding: "utf8" }).trim();
  } catch {
    return "unknown";
  }
}

/** package.json version, shown next to the commit in the DEV tab. */
function appVersion(): string {
  try {
    const pkg = JSON.parse(readFileSync("package.json", "utf8")) as {
      version?: string;
    };
    return pkg.version ?? "unknown";
  } catch {
    return "unknown";
  }
}

// https://vite.dev/config/
export default defineConfig({
  plugins: [svelte()],

  // Build identity: the DEV tab shows which binary/frontend is running, so
  // "is this the new build?" has an answer that doesn't need the terminal.
  define: {
    __APP_VERSION__: JSON.stringify(appVersion()),
    __BUILD_COMMIT__: JSON.stringify(gitCommit()),
    __BUILD_TIME__: JSON.stringify(new Date().toISOString()),
  },

  // Tauri expects a fixed port; fail if that port is not available.
  server: {
    port: 1420,
    strictPort: true,
  },

  // Avoid Vite clearing the Rust build output in the terminal.
  clearScreen: false,
});
