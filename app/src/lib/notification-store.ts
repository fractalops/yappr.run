export type NotificationSnapshot = {
  secret: string;
  tickets: Record<string, string>;
  relays: { disabled: boolean; urls: string[] };
};

export type NotificationAnswer = {
  writer: string;
  submission: { id: string; question_id: string; value: string };
};

export type AnswerResult = "sent" | "retry" | "closed";

function database(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const request = indexedDB.open("yappr.notification-actions", 1);
    request.onupgradeneeded = () => {
      request.result.createObjectStore("settings");
      request.result.createObjectStore("answers");
    };
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
}

async function transaction<T>(
  name: "settings" | "answers",
  mode: IDBTransactionMode,
  run: (store: IDBObjectStore, result: (value: T) => void) => void,
): Promise<T> {
  const db = await database();
  try {
    return await new Promise((resolve, reject) => {
      const tx = db.transaction(name, mode);
      let value: T;
      tx.oncomplete = () => resolve(value);
      tx.onabort = () => reject(tx.error);
      run(tx.objectStore(name), (result) => { value = result; });
    });
  } finally {
    db.close();
  }
}

export function saveNotificationSnapshot(snapshot: NotificationSnapshot | null): Promise<void> {
  return transaction("settings", "readwrite", (store) => {
    if (snapshot) store.put(snapshot, "writer");
    else store.delete("writer");
  });
}

export function loadNotificationSnapshot(): Promise<NotificationSnapshot | undefined> {
  return transaction("settings", "readonly", (store, done) => {
    const request = store.get("writer");
    request.onsuccess = () => done(request.result);
  });
}

function answerKey(writer: string, question: string) {
  return writer + ":" + question;
}

export function queueNotificationAnswer(writer: string, question: string, value: string): Promise<NotificationAnswer> {
  return transaction("answers", "readwrite", (store, done) => {
    const key = answerKey(writer, question);
    const request = store.get(key);
    request.onsuccess = () => {
      if (request.result) { done(request.result); return; }
      const id = Array.from(crypto.getRandomValues(new Uint8Array(32)), (n) => n.toString(16).padStart(2, "0")).join("");
      const answer = { writer, submission: { id, question_id: question, value } };
      store.put(answer, key);
      done(answer);
    };
  });
}

export function notificationAnswers(): Promise<NotificationAnswer[]> {
  return transaction("answers", "readonly", (store, done) => {
    const request = store.getAll();
    request.onsuccess = () => done(request.result);
  });
}

export function forgetNotificationAnswer(writer: string, question: string, submission?: string): Promise<void> {
  return transaction("answers", "readwrite", (store) => {
    const key = answerKey(writer, question);
    if (!submission) { store.delete(key); return; }
    const request = store.get(key);
    request.onsuccess = () => {
      if (request.result?.submission.id === submission) store.delete(key);
    };
  });
}
