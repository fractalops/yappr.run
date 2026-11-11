import assert from "node:assert/strict";
import test from "node:test";
import { notification, openYappr } from "../src/lib/notifications.ts";

const scope = "https://example.test/yappr/";

test("shows text for a valid push without notification actions", () => {
  const notice = notification({
    writer: "Laptop", title: "Build failed", body: "Check the tests.", entry_id: 12,
    choices: ["Approve", "Reject"],
  }, scope);
  assert.equal(notice?.title, "Build failed");
  assert.equal(notice?.options.body, "Laptop\nCheck the tests.");
  assert.equal(notice?.options.icon, scope + "favicon.png");
  assert.equal(notice?.options.tag, "Laptop:12");
  assert.equal("actions" in (notice?.options ?? {}), false);
  assert.equal(notification(null, scope), null);
  assert.equal(notification({ writer: "Laptop" }, scope), null);
});

test("focuses the open app or opens the app within the worker scope", async () => {
  const calls: string[] = [];
  const page = (url: string) => ({ url, focus: async () => calls.push(url) });
  const open = async (url: string) => calls.push("open " + url);
  await openYappr(scope, [page("https://elsewhere.test/yappr/"), page(scope + "#feed")], open);
  assert.deepEqual(calls, [scope + "#feed"]);
  calls.length = 0;
  await openYappr(scope, [page("https://example.test/elsewhere/")], open);
  assert.deepEqual(calls, ["open " + scope]);
});
