type PushMessage = { writer: string; title: string; body: string; entry_id: number };

export function notification(data: unknown, scope: string): { title: string; options: NotificationOptions } | null {
  if (!data || typeof data !== "object") return null;
  const message = data as Partial<PushMessage>;
  if (typeof message.writer !== "string" || typeof message.title !== "string"
    || typeof message.body !== "string" || !Number.isSafeInteger(message.entry_id)) return null;
  return {
    title: message.title || "Yappr",
    options: {
      body: [message.writer, message.body].filter(Boolean).join("\n"),
      icon: new URL("favicon.png", scope).href,
      tag: `${message.writer}:${message.entry_id}`,
    },
  };
}

type Page = { url: string; focus(): Promise<unknown> };

export async function openYappr(scope: string, pages: readonly Page[], open: (url: string) => Promise<unknown>) {
  const page = pages.find(({ url }) => new URL(url).origin === new URL(scope).origin
    && new URL(url).pathname === new URL(scope).pathname);
  if (page) await page.focus();
  else await open(scope);
}
