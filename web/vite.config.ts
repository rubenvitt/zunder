/// <reference types="vitest/config" />
import { defineConfig } from "vite";
import solid from "vite-plugin-solid";

const backend = process.env.ZUNDER_DEV_BACKEND ?? "http://127.0.0.1:8080";
const proxied = ["/api", "/auth", "/mcp", "/healthz"];

export default defineConfig({
  plugins: [solid()],
  server: {
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
