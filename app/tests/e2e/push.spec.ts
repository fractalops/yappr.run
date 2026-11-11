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

test("the service worker shows a push and closes the notification on click", async ({ page, context }) => {
  await context.grantPermissions(["notifications"]);
  await page.goto("./");
  const worker = context.serviceWorkers()[0] ?? await context.waitForEvent("serviceworker");
  await page.evaluate(() => navigator.serviceWorker.ready.then(() => true));
  await worker.evaluate(() => {
    const scope = globalThis as unknown as {
      PushEvent: new (type: string, init: { data: string }) => Event;
      dispatchEvent(event: Event): boolean;
    };
    const push = new scope.PushEvent("push", { data: JSON.stringify({
      writer: "Laptop", title: "A test warning", body: "Check the build.", entry_id: 42,
    }) });
    scope.dispatchEvent(push);
  });
  await expect.poll(() => page.evaluate(async () => {
    const registration = await navigator.serviceWorker.ready;
    return (await registration.getNotifications()).map((notice) => ({ title: notice.title, body: notice.body }));
  })).toEqual([{ title: "A test warning", body: "Laptop\nCheck the build." }]);
  await worker.evaluate(async () => {
    const scope = globalThis as unknown as {
      registration: ServiceWorkerRegistration;
      NotificationEvent: new (type: string, init: { notification: Notification }) => Event;
      dispatchEvent(event: Event): boolean;
    };
    const [notice] = await scope.registration.getNotifications();
    scope.dispatchEvent(new scope.NotificationEvent("notificationclick", { notification: notice }));
  });
  await expect.poll(() => page.evaluate(async () => {
    const registration = await navigator.serviceWorker.ready;
    return (await registration.getNotifications()).length;
  })).toBe(0);
});
