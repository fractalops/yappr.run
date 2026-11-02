import { defineConfig } from "@playwright/test";
import { existsSync } from "node:fs";

const macChrome = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
export default defineConfig({
  testDir: "./tests/e2e", workers: 1, timeout: 180_000,
  expect: { timeout: 30_000 }, reporter: "list",
  use: {
    baseURL: process.env.YAPPR_TEST_URL ?? "http://127.0.0.1:8089/", viewport: { width: 1100, height: 900 },
    permissions: ["clipboard-read", "clipboard-write"], trace: "retain-on-failure", screenshot: "only-on-failure",
    launchOptions: { executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE ?? (existsSync(macChrome) ? macChrome : undefined) },
  },
  webServer: process.env.YAPPR_TEST_URL ? undefined : {
    command: "cargo run --quiet --manifest-path ../Cargo.toml -p yappr-serve -- --static ../pages --listen 127.0.0.1:8089",
    url: "http://127.0.0.1:8089/", reuseExistingServer: false, timeout: 120_000,
  },
});
