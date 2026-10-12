/// <reference types="vitest/config" />
import { defineConfig } from "vite";
import solid from "vite-plugin-solid";

const backend = process.env.ZUNDER_DEV_BACKEND ?? "http://127.0.0.1:8080";
const proxied = ["/api", "/auth", "/mcp", "/healthz"];
// Set by `tauri ios|android dev` for a physical device: the address of this
// machine on the network, which the app's webview loads the dev server from.
const tauriDevHost = process.env.TAURI_DEV_HOST;

export default defineConfig({
  plugins: [solid()],
  server: {
    host: tauriDevHost || false,
    port: 5173,
    // The Tauri app expects exactly this port (devUrl in tauri.conf.json).
    strictPort: true,
    hmr: tauriDevHost ? { protocol: "ws", host: tauriDevHost, port: 5174 } : undefined,
    proxy: Object.fromEntries(proxied.map((path) => [path, { target: backend, changeOrigin: false }])),
  },
  build: {
    target: "es2022",
    cssMinify: true,
    sourcemap: false,
    modulePreload: { polyfill: false },
  },
  test: {
    include: ["test/**/*.test.ts"],
    environment: "node",
    env: { TZ: "Europe/Berlin" },
  },
});
