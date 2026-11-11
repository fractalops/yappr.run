import {
  forgetNotificationAnswer, notificationAnswers, saveNotificationSnapshot,
  type AnswerResult, type NotificationAnswer, type NotificationSnapshot,
} from "./notification-store.ts";
import { questionFromUrl, validId, validQuestionTarget, type QuestionTarget } from "./notifications.ts";

type Invoke = <T>(command: string, args?: Record<string, unknown>) => Promise<T>;

export function canUseNotificationActions(): boolean {
  return ((Notification as typeof Notification & { maxActions?: number }).maxActions ?? 0) > 0;
}

export async function copyNotificationSnapshot(invoke: Invoke, enabled: boolean) {
  await saveNotificationSnapshot(enabled && canUseNotificationActions()
    ? await invoke<NotificationSnapshot>("notification_snapshot") : null);
}

export function startNotificationPage(invoke: Invoke, open: (target: QuestionTarget) => void): () => void {
  const running = new Map<string, Promise<AnswerResult>>();
  const send = (answer: NotificationAnswer) => {
    const id = answer.submission.id;
    const old = running.get(id);
    if (old) return old;
    const attempt = invoke<AnswerResult>("notification_reply", answer)
      .catch((): AnswerResult => "retry")
      .finally(() => running.delete(id));
    running.set(id, attempt);
    return attempt;
  };
  let retrying = false;
  async function retry() {
    if (retrying) return;
    retrying = true;
    try {
      for (const answer of await notificationAnswers()) {
        if (await send(answer) !== "retry") {
          await forgetNotificationAnswer(answer.writer, answer.submission.question_id, answer.submission.id);
        }
      }
    } finally { retrying = false; }
  }
  function message(event: MessageEvent) {
    if (event.origin && event.origin !== location.origin) return;
    if (event.data?.kind === "open-question" && validQuestionTarget(event.data.target)) {
      open(event.data.target);
    } else if (event.data?.kind === "notification-answer" && event.ports[0]) {
      const reply = event.ports[0];
      const answer: NotificationAnswer | undefined = event.data.answer;
      if (!validId(answer?.writer) || !validId(answer?.submission?.id)
        || !validId(answer.submission.question_id) || typeof answer.submission.value !== "string") {
        reply.postMessage("closed");
      } else {
        void send(answer).then(result => reply.postMessage(result));
      }
    }
  }
  const url = new URL(location.href);
  const target = questionFromUrl(url);
  if (target) {
    for (const name of ["writer", "question", "entry"]) url.searchParams.delete(name);
    history.replaceState(history.state, "", url.href);
    open(target);
  }
  navigator.serviceWorker.addEventListener("message", message);
  const flush = () => { if (canUseNotificationActions()) void retry().catch(() => {}); };
  const timer = setInterval(flush, 10000);
  window.addEventListener("online", flush);
  flush();
  return () => {
    navigator.serviceWorker.removeEventListener("message", message);
    window.removeEventListener("online", flush);
    clearInterval(timer);
  };
}

export async function clearResolvedNotifications(ids: readonly string[]) {
  const registration = await navigator.serviceWorker.getRegistration();
  if (!registration) return;
  const resolved = new Set(ids);
  for (const notice of await registration.getNotifications()) {
    if (resolved.has(notice.tag)) notice.close();
  }
}
