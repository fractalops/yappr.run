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

test("shows every choice or none and gives each question its own tag", () => {
  const choices = [{ value: "approve", label: "Approve" }, { value: "reject", label: "Reject" }];
  const push = {
    writer: "Laptop · deploy", title: "Deploy?", body: "Ready.", entry_id: 12,
    writer_id: "a".repeat(64), question_id: "b".repeat(64), kind: "approval",
    choices,
  };
  const notice = notification(push, scope, 2);
  assert.deepEqual(notice?.options.actions, choices.map(({value, label}) => ({action: value, title: label})));
  assert.equal(notice?.options.body, "Laptop · deploy\nApproval: Approve or Reject\nReady.");
  assert.equal(notice?.options.data?.unlock, false);
  assert.equal(notice?.options.tag, push.question_id);
  for (const limit of [0, 1]) assert.equal(notification(push, scope, limit)?.options.actions, undefined);
  const resolved = notification({ ...push, kind: "resolved", title: "Answered: Approve", choices: [] }, scope, 2);
  assert.equal(resolved?.options.tag, notice?.options.tag);
  assert.equal(resolved?.options.silent, true);
  assert.equal(resolved?.options.actions, undefined);
  assert.equal(notification({ ...push, expires_at: "2000-01-01T00:00:00Z" }, scope, 2)?.options.actions, undefined);
});

test("shows only Reject for an approval that opens the app", () => {
  const reject = { value: "reject", label: "Reject" };
  const push = {
    web_push: 8030, mutable: true,
    notification: { title: "Deploy?", navigate: scope },
    writer: "Laptop · deploy", title: "Deploy?", body: "Ready.", entry_id: 12,
    writer_id: "a".repeat(64), question_id: "b".repeat(64),
    kind: "approval", unlock: true, choices: [reject],
  };
  const notice = notification(push, scope, 1);
  assert.deepEqual(notice?.options.actions, [{ action: "reject", title: "Reject" }]);
  assert.deepEqual(notice?.options.data?.choices, [reject]);
  assert.equal(notice?.options.data?.unlock, true);
  assert.equal(notice?.options.body, "Laptop · deploy\nTap the notification to approve.\nReady.");
  assert.equal(notification(push, scope, 0)?.options.actions, undefined);
  assert.deepEqual(notification({
    ...push, choices: [{ value: "approve", label: "Approve" }, reject],
  }, scope, 2)?.options.actions, notice?.options.actions);
});

test("passes the question to the open page or opens its URL", async () => {
  const target = { writer: "a".repeat(64), question: "b".repeat(64), entry: 3 };
  const sent: unknown[] = [];
  const opened: string[] = [];
  const open = async (url: string) => opened.push(url);
  await openYappr(scope, [{ url: scope, focus: async () => {}, postMessage: (data) => sent.push(data) }], open, target);
  assert.deepEqual(sent, [{kind: "open-question", target}]);
  assert.deepEqual(opened, []);
  await openYappr(scope, [], open, target);
  assert.deepEqual(Object.fromEntries(new URL(opened[0]).searchParams), {
    writer: target.writer, question: target.question, entry: "3",
  });
});
