// The saved entries of each machine in IndexedDB. Each entry is one record,
// so that a new entry does not rewrite the history.
const DB = 'yappr';
const ENTRIES = 'entries';

let opening = null;
function db() {
  opening ??= new Promise((resolve, reject) => {
    const request = indexedDB.open(DB, 1);
    request.onupgradeneeded = () => {
      // The key is [machine, entry id]. The order index sorts the feed by
      // time, then by machine and id, so that each entry has one place.
      const store = request.result.createObjectStore(ENTRIES, { keyPath: ['writer', 'id'] });
      store.createIndex('order', ['ms', 'writer', 'id']);
    };
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
  return opening;
}

// Resolve when the transaction is on disk, not when the request is done.
function done(tx) {
  return new Promise((resolve, reject) => {
    tx.oncomplete = () => resolve();
    tx.onerror = () => reject(tx.error);
    tx.onabort = () => reject(tx.error ?? new Error('the save stopped'));
  });
}

function result(request) {
  return new Promise((resolve, reject) => {
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
}

// Save the entries of `writer` (a JSON array of {ms, entry}), and remove the
// entries with an id below `keep_from`.
export async function putEntries(writer, json, keep_from) {
  const tx = (await db()).transaction(ENTRIES, 'readwrite');
  const store = tx.objectStore(ENTRIES);
  for (const { ms, entry } of JSON.parse(json)) {
    store.put({ writer, id: entry.id, ms, entry });
  }
  if (keep_from > 1) store.delete(IDBKeyRange.bound([writer, 0], [writer, keep_from], false, true));
  await done(tx);
}

// All saved entries of `writer` in id order, as a JSON array.
export async function writerEntries(writer) {
  const store = (await db()).transaction(ENTRIES).objectStore(ENTRIES);
  const rows = await result(store.getAll(IDBKeyRange.bound([writer, 0], [writer, Infinity])));
  return JSON.stringify(rows.map((row) => row.entry));
}

// Remove all saved entries of `writer`.
export async function clearWriter(writer) {
  const tx = (await db()).transaction(ENTRIES, 'readwrite');
  tx.objectStore(ENTRIES).delete(IDBKeyRange.bound([writer, 0], [writer, Infinity]));
  await done(tx);
}

// Up to `limit` entries before the place `before` ([ms, writer, id] as JSON,
// or empty for the newest), newest first, as a JSON array of
// {writer, ms, entry}. `writer` limits them to one machine.
export async function page(before, limit, writer) {
  const index = (await db()).transaction(ENTRIES).objectStore(ENTRIES).index('order');
  const range = before ? IDBKeyRange.upperBound(JSON.parse(before), true) : null;
  const rows = [];
  await new Promise((resolve, reject) => {
    const request = index.openCursor(range, 'prev');
    request.onerror = () => reject(request.error);
    request.onsuccess = () => {
      const cursor = request.result;
      if (!cursor || rows.length >= limit) { resolve(); return; }
      if (!writer || cursor.value.writer === writer) rows.push({ writer: cursor.value.writer, ms: cursor.value.ms, entry: cursor.value.entry });
      cursor.continue();
    };
  });
  return JSON.stringify(rows);
}
