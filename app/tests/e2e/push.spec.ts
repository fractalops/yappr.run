import { expect, test } from "@playwright/test";

test("the notification toggle subscribes from the click", async ({ page }) => {
  await page.addInitScript(() => {
    PushManager.prototype.subscribe = function (options) {
      sessionStorage.setItem("push-test-gesture", JSON.stringify({
        active: navigator.userActivation.isActive,
        visible: options?.userVisibleOnly,
        hasKey: options?.applicationServerKey instanceof Uint8Array,
      }));
      return Promise.resolve({
        endpoint: "https://fcm.googleapis.com/test",
        getKey: (kind: PushEncryptionKeyName) => new ArrayBuffer(kind === "auth" ? 16 : 65),
      } as PushSubscription);
    };
  });
  await page.goto("./");
  await page.getByText("Advanced settings", { exact: true }).click();
  const toggle = page.getByRole("checkbox", { name: "Notify me when this page is closed" });
  await toggle.click();
  await expect(toggle).toBeChecked();
  const gesture = await page.evaluate(() => sessionStorage.getItem("push-test-gesture"));
  expect(JSON.parse(gesture ?? "{}")).toEqual({ active: true, visible: true, hasKey: true });
});

test("the notification toggle stays off when the browser refuses push", async ({ page }) => {
  await page.addInitScript(() => {
    PushManager.prototype.subscribe = () => Promise.reject(new DOMException("No permission", "NotAllowedError"));
  });
  await page.goto("./");
  await page.getByText("Advanced settings", { exact: true }).click();
  const toggle = page.getByRole("checkbox", { name: "Notify me when this page is closed" });
  await toggle.click();
  await expect(page.getByText("Check the notification permissions for Yappr, then try again.")).toBeVisible();
  await expect(toggle).not.toBeChecked();
});

test("the service worker shows a push and opens Yappr on click", async ({ page, context }) => {
  await page.goto("./");
  const worker = context.serviceWorkers()[0] ?? await context.waitForEvent("serviceworker");
  await page.evaluate(() => navigator.serviceWorker.ready.then(() => true));
  const result = await worker.evaluate(async () => {
    const scope = globalThis as unknown as {
      registration: ServiceWorkerRegistration;
      clients: { matchAll: () => Promise<unknown[]>; openWindow: (url: string) => Promise<unknown> };
      dispatchEvent(event: Event): boolean;
    };
    // Test the worker in the browser without using the system notification tray.
    const shown: { title: string; body?: string }[] = [];
    const opened: string[] = [];
    let closed = false;
    scope.registration.showNotification = async (title, options) => {
      shown.push({ title, body: options?.body });
    };
    scope.clients.matchAll = async () => [];
    scope.clients.openWindow = async (url) => { opened.push(url); };
    const dispatch = async (name: string, fields: object) => {
      const work: Promise<unknown>[] = [];
      const event = Object.assign(new Event(name), fields, {
        waitUntil: (pending: Promise<unknown>) => work.push(pending),
      });
      scope.dispatchEvent(event);
      await Promise.all(work);
    };
    await dispatch("push", { data: { json: () => ({
      writer: "Laptop", title: "A test warning", body: "Check the build.", entry_id: 42,
    }) } });
    await dispatch("notificationclick", { notification: { close: () => { closed = true; } } });
    return { shown, opened, closed };
  });
  expect(result).toEqual({
    shown: [{ title: "A test warning", body: "Laptop\nCheck the build." }],
    opened: [page.url()],
    closed: true,
  });
});

test("declarative push keeps actions in Chrome and preserves the native fallback", async ({ page, context }) => {
  await page.goto("./");
  const worker = context.serviceWorkers()[0] ?? await context.waitForEvent("serviceworker");
  await page.evaluate(() => navigator.serviceWorker.ready.then(() => true));
  const result = await worker.evaluate(async () => {
    const scope = globalThis as unknown as {
      registration: ServiceWorkerRegistration;
      dispatchEvent(event: Event): boolean;
    };
    const shown: { title: string; actions: string[]; tag?: string }[] = [];
    scope.registration.showNotification = async (title, options) => {
      shown.push({ title, actions: (options as NotificationOptions & { actions?: { action: string }[] } | undefined)?.actions?.map(choice => choice.action) ?? [], tag: options?.tag });
    };
    const question = "b".repeat(64);
    const writer = "a".repeat(64);
    const navigate = new URL(`?writer=${writer}&question=${question}&entry=12`, scope.registration.scope).href;
    const message = {
      web_push: 8030,
      mutable: true,
      notification: { title: "Deploy?", body: "Work Mac · build", navigate, tag: question },
      title: "Deploy?", writer: "Work Mac · build", body: "", kind: "approval",
      writer_id: writer, question_id: question, entry_id: 12,
      choices: [{value:"approve", label:"Approve"}, {value:"reject", label:"Reject"}],
    };
    const dispatch = async (limit: number, fallback?: object) => {
      Object.defineProperty(Notification, "maxActions", { configurable: true, value: limit });
      const work: Promise<unknown>[] = [];
      scope.dispatchEvent(Object.assign(new Event("push"), {
        data: { json: () => message }, notification: fallback,
        waitUntil: (pending: Promise<unknown>) => work.push(pending),
      }));
      await Promise.all(work);
    };
    await dispatch(2);
    await dispatch(0, message.notification);
    return shown;
  });
  expect(result).toEqual([{ title: "Deploy?", actions: ["approve", "reject"], tag: "b".repeat(64) }]);
});
