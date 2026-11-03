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
  await page.getByLabel("Pairing code, machine code, or feed ticket", { exact: true }).fill(code);
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
  await page.getByRole("button", { name: "Copy text", exact: true }).click();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe("Build finished\n\nAll checks passed.");
  await page.getByRole("button", { name: "Copy signed entry", exact: true }).click();
  expect(JSON.parse(await page.evaluate(() => navigator.clipboard.readText())).entry.title).toBe("Build finished");
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
  // A question that nobody answers keeps its text when it expires.
  await cli("approve", "my-browser", "Run the migration?", "--body", "psql -f migrate.sql", "--timeout", "10s", "--no-wait");
  await expect(page.getByText("Expired", { exact: true })).toBeVisible();
  await expect(page.getByText("psql -f migrate.sql", { exact: true })).toBeVisible();
  // With no start, the page cannot reach the machine: offline, not reconnecting.
  // The label also shows the last delivery.
  await expect(page.getByRole("button", { name: /^Connection: Offline · last delivery .+\. Open setup$/ })).toBeVisible({ timeout: 90_000 });
  await page.getByRole("tab", { name: "Setup", exact: true }).click();
  await expect(page.getByText("This device cannot reach it.")).toBeVisible();
});

test("a long history loads a page at a time and keeps the book small", async ({ page }) => {
  // 1200 entries before pairing. The writer delivers them all after pairing.
  const lines = Array.from({ length: 1200 }, (_, i) => JSON.stringify({ title: `Entry ${i + 1}`, body: "x" }));
  const posting = execFile(binary, ["--dir", dir, "post", "--json"], { env: env() });
  posting.stdin!.end(lines.join("\n") + "\n");
  await new Promise((done) => posting.once("exit", done));
  await pair(page);
  const rows = page.locator("article.feed-row");
  await expect(page.getByRole("button", { name: "Entry 1200. Show full text and details", exact: true })).toBeVisible();
  await expect(rows).toHaveCount(100);
  await page.getByRole("button", { name: "Show older entries", exact: true }).click();
  await expect(rows).toHaveCount(200);
  // The machine list stays small: the entries are in IndexedDB.
  const book = await page.evaluate(() => localStorage.getItem("yappr.readers") ?? "");
  expect(book.length).toBeLessThan(10_000);
  await expect.poll(async () => JSON.parse(await page.evaluate(() => localStorage.getItem("yappr.readers") ?? "{}"))
    .writers?.[Object.keys(JSON.parse(book).writers)[0]]?.last_id, { timeout: 60_000 }).toBe(1200);
  await page.reload();
  await expect(page.getByRole("button", { name: "Entry 1200. Show full text and details", exact: true })).toBeVisible();
  await expect(rows).toHaveCount(100);
});

test("entries that an older page saved move to IndexedDB", async ({ page }) => {
  const ticket = (await cli("ticket")).stdout.trim();
  const writer = (await cli("id")).stdout.trim();
  const entries = [1, 2, 3].map((id) => ({ v: 1, id, time: `2026-11-01T10:00:0${id}Z`, source: "old", kind: "event", title: `Old ${id}`, body: "b" }));
  const book = { writers: { [writer]: { ticket, name: "Old laptop", entries, enabled: false } }, selected: null, invitation: null };
  await page.addInitScript((value) => { if (!localStorage.getItem("yappr.readers")) localStorage.setItem("yappr.readers", value); }, JSON.stringify(book));
  await page.goto("./");
  await page.getByRole("tab", { name: "Feed", exact: true }).click();
  await expect(page.getByRole("button", { name: "Old 3. Show full text and details", exact: true })).toBeVisible();
  await expect(page.locator("article.feed-row")).toHaveCount(3);
  const saved = JSON.parse(await page.evaluate(() => localStorage.getItem("yappr.readers") ?? "{}"));
  expect(saved.writers[writer].entries).toBeUndefined();
  expect(saved.writers[writer].last_id).toBe(3);
});

test("a CI job with its own key delivers and asks, and each run keeps its entries", async ({ page }) => {
  await pair(page);
  // On the laptop: a key for the job, and the address of this page.
  const key = JSON.parse((await cli("key", "--name", "CI deploys", "--for", "my-browser", "--output", "json")).stdout);
  await page.getByRole("tab", { name: "Setup", exact: true }).click();
  await page.getByRole("button", { name: "Paste a code", exact: true }).click();
  await page.getByLabel("Pairing code, machine code, or feed ticket", { exact: true }).fill(key.machine_code);
  await page.getByRole("button", { name: "Continue", exact: true }).click();
  await expect(page.getByText(/CI deploys ·/)).toBeVisible();
  await page.getByRole("button", { name: "Allow machine", exact: true }).click();
  await expect(page.getByRole("button", { name: "Pause CI deploys", exact: true })).toBeVisible();
  await expect(page.getByLabel("Connected machines").getByText("Waiting for deliveries")).toBeVisible();
  await page.getByRole("tab", { name: "Feed", exact: true }).click();

  // Each CI run starts with an empty feed directory and the same key.
  async function run(...args: string[]) {
    const ci = await mkdtemp(resolve(tmpdir(), "yappr-ci-run-"));
    const job = (...a: string[]) => execute(binary, ["--dir", ci, ...a], { env: { ...env(), YAPPR_KEY: key.private_key }, timeout: 60_000 });
    await job("allow", key.device.address, "--alias", "phone", "--receives");
    return job(...args);
  }
  await run("post", "--title", "Build 1 passed", "--body", "run 1");
  await expect(page.getByRole("button", { name: "Build 1 passed. Show full text and details", exact: true })).toBeVisible();
  const asking = run("approve", "phone", "Deploy build 2?");
  await page.getByRole("button", { name: "Deploy build 2?. Show full text and details", exact: true }).waitFor();
  await page.getByRole("button", { name: "Approve", exact: true }).click();
  await asking;
  await expect(page.getByText(/· confirmed/)).toBeVisible();
  // The first run's entry stays after the second run.
  await expect(page.getByRole("button", { name: "Build 1 passed. Show full text and details", exact: true })).toBeVisible();
  await page.reload();
  await expect(page.getByRole("button", { name: "Build 1 passed. Show full text and details", exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Deploy build 2?. Show full text and details", exact: true })).toBeVisible();
});

test("entries in the first IndexedDB layout stay after the upgrade", async ({ page }) => {
  const ticket = (await cli("ticket")).stdout.trim();
  const writer = (await cli("id")).stdout.trim();
  const log = "2026-11-01T10:00:00Z";
  const entry = { v: 1, id: 1, time: log, source: "old", kind: "event", title: "Saved before logs", body: "b" };
  const book = { writers: { [writer]: { ticket, name: "Old laptop", last_id: 1, log, enabled: false } }, selected: null, invitation: null };
  await page.addInitScript(async ({ book, writer, entry }) => {
    if (localStorage.getItem("yappr.readers")) return;
    localStorage.setItem("yappr.readers", book);
    // The layout of version 1: the key is [machine, entry id].
    await new Promise<void>((done) => {
      const open = indexedDB.open("yappr", 1);
      open.onupgradeneeded = () => {
        const store = open.result.createObjectStore("entries", { keyPath: ["writer", "id"] });
        store.createIndex("order", ["ms", "writer", "id"]);
        store.put({ writer, id: 1, ms: Date.parse(entry.time), entry });
      };
      open.onsuccess = () => { open.result.close(); done(); };
    });
  }, { book: JSON.stringify(book), writer, entry });
  await page.goto("./");
  await page.getByRole("tab", { name: "Feed", exact: true }).click();
  await expect(page.getByRole("button", { name: "Saved before logs. Show full text and details", exact: true })).toBeVisible();
});
