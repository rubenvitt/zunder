import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { defineConfig, devices, type PlaywrightTestConfig } from "@playwright/test";

// ZUNDER_E2E_BACKEND=mock (default): Node mock server serving web/dist.
// ZUNDER_E2E_BACKEND=real: the Rust server with dev auth and a temporary data dir.
const backend = process.env.ZUNDER_E2E_BACKEND ?? "mock";
const port = Number(process.env.ZUNDER_E2E_PORT ?? (backend === "real" ? 18080 : 4173));
const baseURL = `http://127.0.0.1:${port}`;
const repoRoot = resolve(import.meta.dirname, "..");

const webServer: PlaywrightTestConfig["webServer"] =
  backend === "real"
    ? {
        command: "cargo run -p zunder-server",
        cwd: repoRoot,
        url: `${baseURL}/healthz`,
        timeout: 600_000,
        reuseExistingServer: false,
        env: {
          ZUNDER_DEV_AUTH: "true",
          ZUNDER_DATA_DIR: mkdtempSync(join(tmpdir(), "zunder-e2e-")),
          ZUNDER_WEB_DIR: "web/dist",
          ZUNDER_BIND: `127.0.0.1:${port}`,
          ZUNDER_PUBLIC_URL: baseURL,
        },
      }
    : {
        command: "node e2e/mock-server.ts",
        url: `${baseURL}/healthz`,
        timeout: 30_000,
        reuseExistingServer: !process.env.CI,
        env: { MOCK_PORT: String(port) },
      };

export default defineConfig({
  testDir: "e2e",
  fullyParallel: false,
  workers: 1,
  retries: process.env.CI ? 1 : 0,
  reporter: process.env.CI ? [["list"], ["html", { open: "never" }]] : "list",
  timeout: 30_000,
  use: {
    baseURL,
    trace: "retain-on-failure",
    locale: "de-DE",
    timezoneId: "Europe/Berlin",
  },
  metadata: { backend },
  projects: [
    { name: "desktop", use: { ...devices["Desktop Chrome"], viewport: { width: 1360, height: 860 } }, grepInvert: /@mobile/ },
    { name: "mobile", use: { ...devices["Pixel 7"] }, grep: /@mobile/ },
  ],
  webServer,
});
