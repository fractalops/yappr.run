export type QuestionTarget = { writer: string; question: string; entry: number };
export type NoticeChoice = { value: string; label: string };
export type QuestionNotice = {
  target: QuestionTarget;
  choices: NoticeChoice[];
  unlock: boolean;
  expires: number | null;
};
type PushMessage = {
  writer: string; title: string; body: string; entry_id: number;
  writer_id?: string; question_id?: string; choices?: unknown;
  kind?: string; unlock?: boolean; expires_at?: string;
};

export const validId = (value: unknown): value is string => typeof value === "string" && /^[0-9a-f]{64}$/.test(value);
const validChoice = (value: unknown): value is NoticeChoice => {
  if (!value || typeof value !== "object") return false;
  const choice = value as Partial<NoticeChoice>;
  return typeof choice.value === "string" && /^[A-Za-z0-9_-]{1,64}$/.test(choice.value)
    && typeof choice.label === "string" && choice.label.length > 0;
};
export function validQuestionTarget(value: unknown): value is QuestionTarget {
  if (!value || typeof value !== "object") return false;
  const target = value as Partial<QuestionTarget>;
  return validId(target.writer) && validId(target.question) && Number.isSafeInteger(target.entry) && target.entry! > 0;
}

export function notification(data: unknown, scope: string, maxActions = 0) {
  if (!data || typeof data !== "object") return null;
  const message = data as Partial<PushMessage>;
  if (typeof message.writer !== "string" || typeof message.title !== "string"
    || typeof message.body !== "string" || !Number.isSafeInteger(message.entry_id)) return null;
  const target = { writer: message.writer_id, question: message.question_id, entry: message.entry_id };
  const unlock = message.kind === "approval" && message.unlock === true;
  const allChoices = Array.isArray(message.choices) && message.choices.every(validChoice) ? message.choices : [];
  const choices = unlock ? allChoices.filter(choice => choice.value === "reject") : allChoices;
  const resolved = message.kind === "resolved";
  const question = validQuestionTarget(target) ? {
    target, choices, unlock,
    expires: message.expires_at ? Date.parse(message.expires_at) : null,
  } : null;
  const label = unlock ? "Tap the notification to approve." : choices.length ? (message.kind === "approval" ? "Approval: " : "Question: ")
    + choices.map(c => c.label).join(" or ") : "";
  const actions = question && !resolved && (question.expires === null || question.expires > Date.now())
    && choices.length >= (unlock ? 1 : 2) && choices.length <= maxActions
    ? choices.map(({value, label}) => ({action: value, title: label})) : [];
  return {
    title: message.title || "Yappr",
    resolved,
    options: {
      body: [message.writer, label, message.body].filter(Boolean).join("\n"),
      icon: new URL("favicon.png", scope).href,
      tag: question?.target.question ?? `${message.writer}:${message.entry_id}`,
      ...(question ? { data: question } : {}),
      ...(actions.length ? { actions } : {}),
      ...(resolved ? { silent: true } : {}),
    },
  };
}

export function questionUrl(scope: string, target: QuestionTarget): string {
  const url = new URL(scope);
  url.searchParams.set("writer", target.writer);
  url.searchParams.set("question", target.question);
  url.searchParams.set("entry", String(target.entry));
  return url.href;
}

export function questionFromUrl(url: URL): QuestionTarget | null {
  const target = {
    writer: url.searchParams.get("writer"),
    question: url.searchParams.get("question"),
    entry: Number(url.searchParams.get("entry")),
  };
  return validQuestionTarget(target) ? target : null;
}

type Page = { url: string; focus(): Promise<unknown>; postMessage?(value: unknown): void };
export function findYapprPages<T extends {url: string}>(scope: string, pages: readonly T[]): T[] {
  const base = new URL(scope);
  return pages.filter(({url}) => {
    const candidate = new URL(url);
    return candidate.origin === base.origin && candidate.pathname === base.pathname;
  });
}

export async function openYappr(
  scope: string, pages: readonly Page[], open: (url: string) => Promise<unknown>,
  target?: QuestionTarget,
) {
  const [page] = findYapprPages(scope, pages);
  if (page) {
    if (target) page.postMessage?.({ kind: "open-question", target });
    await page.focus();
  } else await open(target ? questionUrl(scope, target) : scope);
}
