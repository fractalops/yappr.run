// The saved entries of each machine in IndexedDB. Each entry is one record,
// so that a new entry does not rewrite the history.
const DB = 'yappr';
// The key of a record is [machine, log, entry id]. A machine that starts a
// new log, such as a new CI run, keeps the entries of its older logs.
const ENTRIES = 'records';
// The store of version 1, with the key [machine, entry id].
const OLD = 'entries';

let opening = null;
function db() {
  opening ??= new Promise((resolve, reject) => {
    const request = indexedDB.open(DB, 2);
    request.onupgradeneeded = (event) => {
      const db = request.result;
      // The order index sorts the feed by time, then by machine, log, and
      // id, so that each entry has one place.
      const store = db.createObjectStore(ENTRIES, { keyPath: ['writer', 'log', 'id'] });
      store.createIndex('order', ['ms', 'writer', 'log', 'id']);
      if (event.oldVersion >= 1) {
        // Copy the version 1 records with an empty log. The page moves them
        // to the log of their machine when it opens.
        const old = request.transaction.objectStore(OLD);
        old.openCursor().onsuccess = (e) => {
          const cursor = e.target.result;
          if (!cursor) { db.deleteObjectStore(OLD); return; }
          store.put({ ...cursor.value, log: '' });
          cursor.continue();
        };
      }
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

function logRange(writer, log) {
  return IDBKeyRange.bound([writer, log, 0], [writer, log, Infinity]);
}

// Save the entries of the log `log` of `writer` (a JSON array of
// {ms, entry}). Remove the entries of that log with an id below `keep_from`,
// and all entries of the logs in `drop` (a JSON array of log names).
export async function putEntries(writer, log, json, keep_from, drop) {
  const tx = (await db()).transaction(ENTRIES, 'readwrite');
  const store = tx.objectStore(ENTRIES);
  for (const { ms, entry } of JSON.parse(json)) {
    store.put({ writer, log, id: entry.id, ms, entry });
  }
  if (keep_from > 1) store.delete(IDBKeyRange.bound([writer, log, 0], [writer, log, keep_from], false, true));
  for (const old of JSON.parse(drop)) store.delete(logRange(writer, old));
  await done(tx);
}

// All saved entries of `writer`, by log and then by id, as a JSON array.
export async function writerEntries(writer) {
  const store = (await db()).transaction(ENTRIES).objectStore(ENTRIES);
  const rows = await result(store.getAll(IDBKeyRange.bound([writer, '', 0], [writer, '￿', Infinity])));
  return JSON.stringify(rows.map((row) => row.entry));
}

// Move the entries of the log `from` of `writer` to the log `to`.
export async function moveLog(writer, from, to) {
  const tx = (await db()).transaction(ENTRIES, 'readwrite');
  const store = tx.objectStore(ENTRIES);
  const rows = await result(store.getAll(logRange(writer, from)));
  for (const row of rows) store.put({ ...row, log: to });
  store.delete(logRange(writer, from));
  await done(tx);
}

// Up to `limit` entries before the place `before` ([ms, writer, log, id] as
// JSON, or empty for the newest), newest first, as a JSON array of
// {writer, log, ms, entry}. `writer` limits them to one machine.
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
      const { value } = cursor;
      if (!writer || value.writer === writer) rows.push({ writer: value.writer, log: value.log, ms: value.ms, entry: value.entry });
      cursor.continue();
    };
  });
  return JSON.stringify(rows);
}
