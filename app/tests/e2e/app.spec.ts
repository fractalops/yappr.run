import { test, expect, type Page } from "@playwright/test";
import { appendFileSync } from "node:fs";
import { execFile, spawn, type ChildProcess } from "node:child_process";
import { promisify } from "node:util";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { resolve } from "node:path";

// The CLI on the computer side. These tests use the public relays.
const binary = process.env.YAPPR_BIN ?? "yappr";
const execute = promisify(execFile);
let dir: string;
let writer: ChildProcess | undefined;
let writerLog = "";

function env() {
  const env = { ...process.env };
  delete env.YAPPR_NETWORK; delete env.IROH_SECRET; delete env.RUST_LOG;
  return env;
}
async function cli(...args: string[]) {
  return execute(binary, ["--dir", dir, ...args], { env: env(), timeout: 60_000 });
}
// Start the writer with a pairing code. Returns the code and the next line of
// stderr that names a device to accept.
async function startWriter(): Promise<{ code: string; device: Promise<string> }> {
  writer = spawn(binary, ["--dir", dir, "start", "--pair", "--name", "Work laptop"], { env: env() });
  let out = "", err = "";
  const device = new Promise<string>((done) => {
    writer!.stderr!.on("data", (chunk) => {
      err += chunk;
      writerLog += chunk;
      const id = err.match(/yappr pair --accept ([0-9a-f]{64})/);
      if (id) done(id[1]);
    });
  });
  const code = await new Promise<string>((done) => {
    writer!.stdout!.on("data", (chunk) => {
      out += chunk;
      const line = out.split("\n").find((l) => l.startsWith("YAPPR:"));
      if (line) done(line.trim());
    });
  });
  return { code, device };
}
async function stopWriter() {
  if (writer && writer.exitCode === null) {
    const stopped = new Promise((done) => writer!.once("exit", done));
    writer.kill("SIGINT");
    const killed = setTimeout(() => writer?.kill("SIGKILL"), 10_000);
    await stopped;
    clearTimeout(killed);
  }
  writer = undefined;
}
async function pair(page: Page) {
  const { code, device } = await startWriter();
  await page.goto("./");
  await expect(page.getByRole("tab", { name: "Setup", exact: true })).toHaveAttribute("aria-selected", "true");
  await page.getByRole("button", { name: "Paste a code", exact: true }).click();
  await page.getByLabel("Pairing code or feed ticket", { exact: true }).fill(code);
  await page.getByRole("button", { name: "Continue", exact: true }).click();
  await expect(page.getByText(/Work laptop ·/)).toBeVisible();
  await page.getByRole("button", { name: "Request pairing", exact: true }).click();
  await cli("pair", "--accept", await device);
  await expect(page.getByRole("button", { name: "Pause Work laptop", exact: true })).toBeVisible();
  await page.getByRole("tab", { name: "Feed", exact: true }).click();
}
async function answer(page: Page, question: string, value: string) {
  const asking = cli("ask", "my-browser", question, "--choice", "yes", "--choice", "no");
  await page.getByRole("button", { name: `${question}. Show full text and details`, exact: true }).waitFor();
  await page.getByRole("button", { name: value, exact: true }).click();
  const { stdout } = await asking;
  expect(stdout.trim()).toBe(value);
  await expect(page.getByText(/· confirmed/)).toBeVisible();
}

test.beforeEach(async ({ page }, info) => {
  dir = await mkdtemp(resolve(tmpdir(), "yappr-app-e2e-"));
  // The page log helps to find the cause of a failed test.
  const log = info.outputPath("page.log");
  page.on("console", (message) => appendFileSync(log, `${new Date().toISOString()} ${message.type()} ${message.text()}\n`));
});
test.afterEach(async ({}, info) => {
  await stopWriter();
  appendFileSync(info.outputPath("start.log"), writerLog);
  writerLog = "";
  await rm(dir, { recursive: true, force: true });
});

test("a paired page gets new entries and answers a question while start runs", async ({ page }) => {
  await pair(page);
  await cli("post", "--title", "Build finished", "--body", "All checks passed.");
  const row = page.getByRole("button", { name: "Build finished. Show full text and details", exact: true });
  await expect(row).toBeVisible();
  await row.click();
  await expect(page.getByText("Signed", { exact: true })).toBeVisible();
  await answer(page, "Deploy?", "yes");
  await page.reload();
  await expect(row).toHaveCount(1);
});

test("post and ask deliver to the page when start does not run", async ({ page }) => {
  await pair(page);
  await stopWriter();
  await cli("post", "--title", "From a script", "--body", "No writer process runs.");
  await expect(page.getByRole("button", { name: "From a script. Show full text and details", exact: true })).toBeVisible();
  await answer(page, "Retry the backup?", "no");
  const status = JSON.parse((await cli("status", "--output", "json")).stdout);
  expect(status.peers[0].receives).toBe(true);
});
