import { self } from "$app/service-worker";
import { findYapprPages, notification, openYappr, validQuestionTarget, type QuestionNotice } from "../lib/notifications.ts";
import { forgetNotificationAnswer, loadNotificationSnapshot, queueNotificationAnswer, type AnswerResult, type NotificationAnswer } from "../lib/notification-store.ts";
import init, { notification_reply } from "./reader.generated.js";

declare const __YAPPR_BUILD__: string;

let reader: Promise<unknown> | undefined;
const scope = self.registration.scope;
const pages = () => self.clients.matchAll({ type: "window", includeUncontrolled: true });

self.addEventListener("push", (event) => {
  const maxActions = (Notification as typeof Notification & { maxActions?: number }).maxActions ?? 0;
  // Keep the browser's declarative notification and its URL if it cannot show buttons.
  if (!maxActions && (event as PushEvent & { notification?: Notification }).notification) return;
  let message: unknown;
  try { message = event.data?.json(); }
  catch { return; }
  const notice = notification(message, scope, maxActions);
  if (!notice) return;
  event.waitUntil((async () => {
    await self.registration.showNotification(notice.title, notice.options);
    if (notice.resolved && notice.options.data) {
      const {writer, question} = notice.options.data.target;
      await forgetNotificationAnswer(writer, question).catch(() => {});
    }
  })());
});

function handOff(page: WindowClient, answer: NotificationAnswer): Promise<AnswerResult> {
  return new Promise((resolve) => {
    const channel = new MessageChannel();
    const finish = (result: AnswerResult) => {
      clearTimeout(timer);
      channel.port1.close();
      resolve(result);
    };
    const timer = setTimeout(() => finish("retry"), 9500);
    channel.port1.onmessage = (event) => {
      const result: unknown = event.data;
      finish(result === "sent" || result === "closed" ? result : "retry");
    };
    try { page.postMessage({ kind: "notification-answer", answer }, [channel.port2]); }
    catch { finish("retry"); }
  });
}

async function submit(answer: NotificationAnswer): Promise<AnswerResult> {
  const appPages = findYapprPages(scope, await pages());
  if (appPages.length) {
    const results = await Promise.all(appPages.map((page) => handOff(page, answer)));
    return results.find((result) => result === "sent") ?? results.find((result) => result === "closed") ?? "retry";
  }
  const locks = self.navigator.locks;
  if (!locks) return "retry";
  const snapshot = await loadNotificationSnapshot();
  if (!snapshot) return "retry";
  const wasm = new URL("wasm/yappr_browser_bg.wasm", scope);
  wasm.searchParams.set("v", __YAPPR_BUILD__);
  reader ??= init({ module_or_path: wasm.href }).catch((error) => { reader = undefined; throw error; });
  await reader;
  return locks.request("yappr.notification-reply", { ifAvailable: true }, async (replyLock) => {
    if (!replyLock) return "retry";
    return locks.request("yappr.endpoint", { ifAvailable: true }, async (endpointLock) => {
      if (!endpointLock) return "retry";
      return JSON.parse(await notification_reply(JSON.stringify(snapshot), JSON.stringify(answer))) as AnswerResult;
    });
  });
}

async function answerNotice(data: QuestionNotice, value: string) {
  const { writer, question } = data.target;
  const show = (title: string) => self.registration.showNotification(title, {
    body: "Select this notification to open Yappr.",
    tag: question,
    data: { ...data, choices: [] },
  });
  let answer;
  try { answer = await queueNotificationAnswer(writer, question, value); }
  catch { await show("Answer not saved"); return; }
  const result = await submit(answer).catch(() => "retry");
  if (result === "retry") {
    await show("Answer not sent yet");
  } else {
    await forgetNotificationAnswer(writer, question, answer.submission.id);
    if (result === "closed") await show("Answer not accepted");
  }
}

self.addEventListener("notificationclick", (event) => {
  event.notification.close();
  const data = event.notification.data as QuestionNotice | undefined;
  const target = validQuestionTarget(data?.target) ? data?.target : undefined;
  if (target && data && event.action && data.choices.some((choice) => choice.value === event.action)
      && !(data.unlock && event.action === "approve")
      && (data.expires === null || data.expires > Date.now())) {
    event.waitUntil(answerNotice(data, event.action));
  } else {
    event.waitUntil((async () => {
      await openYappr(scope, await pages(), (url) => self.clients.openWindow(url), target);
    })());
  }
});
