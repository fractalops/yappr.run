//! The saved entries of each machine, in IndexedDB. See `entry_store.js`.
use anyhow::{anyhow, Result};
use serde::Serialize;
use wasm_bindgen::prelude::*;
use yappr::entry::Entry;

/// The most entries that the page keeps for one machine. A new entry removes
/// the oldest entry above this number.
pub const LIMIT: u64 = 5000;

#[wasm_bindgen(module = "/src/entry_store.js")]
extern "C" {
    #[wasm_bindgen(catch, js_name = putEntries)]
    async fn put_entries(writer: &str, json: &str, keep_from: f64) -> Result<(), JsValue>;
    #[wasm_bindgen(catch, js_name = writerEntries)]
    async fn writer_entries(writer: &str) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch, js_name = clearWriter)]
    async fn clear_writer(writer: &str) -> Result<(), JsValue>;
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

/// Save `entries` of `writer`, and keep at most [`LIMIT`] of its entries.
/// Returns when the browser has the entries on disk.
pub async fn save(writer: &str, entries: &[Entry]) -> Result<()> {
    let Some(last) = entries.iter().map(|e| e.id).max() else {
        return Ok(());
    };
    let records: Vec<Record> = entries
        .iter()
        .map(|entry| Record {
            ms: entry.time.as_millisecond(),
            entry,
        })
        .collect();
    let keep_from = (last + 1).saturating_sub(LIMIT);
    put_entries(writer, &serde_json::to_string(&records)?, keep_from as f64)
        .await
        .map_err(js)
}

/// All saved entries of `writer`, in id order.
pub async fn all(writer: &str) -> Result<Vec<Entry>> {
    let json = writer_entries(writer).await.map_err(js)?;
    Ok(serde_json::from_str(&json.as_string().unwrap_or_default())?)
}

/// Remove all saved entries of `writer`.
pub async fn clear(writer: &str) -> Result<()> {
    clear_writer(writer).await.map_err(js)
}

/// Up to `limit` entries before the place `before`, newest first, as JSON.
pub async fn page(before: &str, limit: u32, writer: &str) -> Result<serde_json::Value> {
    let json = page_of(before, limit, writer).await.map_err(js)?;
    Ok(serde_json::from_str(&json.as_string().unwrap_or_default())?)
}
