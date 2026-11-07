//! The saved entries of each machine, in IndexedDB. See `entry_store.js`.
use anyhow::{anyhow, Result};
use serde::Serialize;
use wasm_bindgen::prelude::*;
use yappr::entry::Entry;

use crate::{readers::LIMIT, storage::Dir};

#[wasm_bindgen(module = "/src/entry_store.js")]
extern "C" {
    #[wasm_bindgen(catch, js_name = putEntries)]
    async fn put_entries(
        writer: &str,
        log: &str,
        json: &str,
        keep_from: f64,
        drop: &str,
    ) -> Result<(), JsValue>;
    #[wasm_bindgen(catch, js_name = moveLog)]
    async fn move_log(writer: &str, from: &str, to: &str) -> Result<(), JsValue>;
    #[wasm_bindgen(catch, js_name = writerEntries)]
    async fn writer_entries(writer: &str) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch, js_name = page)]
    async fn page_of(before: &str, limit: u32, writer: &str) -> Result<JsValue, JsValue>;
}

fn js(error: JsValue) -> anyhow::Error {
    anyhow!(
        "browser storage: {}",
        error.as_string().unwrap_or_else(|| format!("{error:?}"))
    )
}

#[derive(Serialize)]
struct Record<'a> {
    ms: i64,
    entry: &'a Entry,
}

/// Save `entries` of the log `log` of `writer`. Keep at most [`LIMIT`]
/// entries of that log, and remove the logs in `drop`. Returns when the
/// browser has the entries on disk.
pub async fn save(
    _: &Dir,
    writer: &str,
    log: &str,
    entries: &[Entry],
    drop: &[String],
) -> Result<()> {
    if entries.is_empty() && drop.is_empty() {
        return Ok(());
    }
    let records: Vec<Record> = entries
        .iter()
        .map(|entry| Record {
            ms: entry.time.as_millisecond(),
            entry,
        })
        .collect();
    let keep_from = entries
        .iter()
        .map(|e| e.id)
        .max()
        .map(|last| last.saturating_add(1).saturating_sub(LIMIT))
        .unwrap_or(0);
    put_entries(
        writer,
        log,
        &serde_json::to_string(&records)?,
        keep_from as f64,
        &serde_json::to_string(drop)?,
    )
    .await
    .map_err(js)
}

/// All saved entries of `writer`, in id order.
pub async fn all(_: &Dir, writer: &str) -> Result<Vec<Entry>> {
    let json = writer_entries(writer).await.map_err(js)?;
    Ok(serde_json::from_str(&json.as_string().unwrap_or_default())?)
}

/// Move the entries of the log `from` of `writer` to the log `to`.
pub async fn rename_log(_: &Dir, writer: &str, from: &str, to: &str) -> Result<()> {
    move_log(writer, from, to).await.map_err(js)
}

/// Up to `limit` entries before the place `before`, newest first, as JSON.
pub async fn page(_: &Dir, before: &str, limit: u32, writer: &str) -> Result<serde_json::Value> {
    let json = page_of(before, limit, writer).await.map_err(js)?;
    Ok(serde_json::from_str(&json.as_string().unwrap_or_default())?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasm_bindgen_test::*;
    wasm_bindgen_test_configure!(run_in_browser);

    #[wasm_bindgen_test]
    async fn empty_save_drops_old_logs_without_changing_other_logs() {
        let dir = Dir::default();
        let entry: Entry = serde_json::from_value(serde_json::json!({
            "v":1,"id":1,"time":"2026-11-01T10:00:00Z","source":"test","kind":"event","body":"saved"
        }))
        .unwrap();
        save(&dir, "drop-test", "old", &[entry.clone()], &[])
            .await
            .unwrap();
        save(&dir, "drop-test", "current", &[entry], &[])
            .await
            .unwrap();
        save(&dir, "drop-test", "current", &[], &["old".into()])
            .await
            .unwrap();
        let rows = page(&dir, "", 100, "drop-test").await.unwrap();
        assert_eq!(rows.as_array().unwrap().len(), 1);
        assert_eq!(rows[0]["log"], "current");
        save(&dir, "drop-test", "current", &[], &["current".into()])
            .await
            .unwrap();
    }
}
