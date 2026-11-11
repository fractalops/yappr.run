import { self } from "$app/service-worker";
import { notification, openYappr } from "../lib/notifications.ts";

self.addEventListener("push", (event) => {
  let message: unknown;
  try { message = event.data?.json(); }
  catch { return; }
  const notice = notification(message, self.registration.scope);
  if (notice) event.waitUntil(self.registration.showNotification(notice.title, notice.options));
});

self.addEventListener("notificationclick", (event) => {
  event.notification.close();
  event.waitUntil((async () => {
    const pages = await self.clients.matchAll({ type: "window", includeUncontrolled: true });
    await openYappr(self.registration.scope, pages, (url) => self.clients.openWindow(url));
  })());
});
