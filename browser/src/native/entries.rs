//! Indexed entry storage with transactional saves and bounded page reads.
use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use yappr::{disk, entry::Entry};

use crate::{readers::LIMIT, storage::Dir};

fn open(dir: &Dir) -> Result<Connection> {
    let _guard = disk::lock(&dir.path, "entries.lock")?;
    let path = dir.path.join("entries.sqlite3");
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) => {
            anyhow::ensure!(
                metadata.is_file(),
                "the entry database is not a regular file"
            );
            #[cfg(unix)]
            {
                use std::os::unix::fs::{MetadataExt, PermissionsExt};
                anyhow::ensure!(
                    metadata.uid() == std::fs::metadata(&dir.path)?.uid(),
                    "the entry database belongs to another user"
                );
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            disk::write_bytes(&path, &[])?
        }
        Err(error) => return Err(error.into()),
    }
    let db = Connection::open(path)?;
    db.busy_timeout(std::time::Duration::from_secs(5))?;
    db.execute_batch(
        "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
        CREATE TABLE IF NOT EXISTS entries (
            writer TEXT NOT NULL, log TEXT NOT NULL, id INTEGER NOT NULL,
            ms INTEGER NOT NULL, entry TEXT NOT NULL,
            PRIMARY KEY(writer, log, id));
        CREATE INDEX IF NOT EXISTS entry_order ON entries(ms, writer, log, id);",
    )?;
    Ok(db)
}

async fn blocking<T: Send + 'static>(
    dir: &Dir,
    work: impl FnOnce(&mut Connection) -> Result<T> + Send + 'static,
) -> Result<T> {
    let dir = dir.clone();
    tokio::task::spawn_blocking(move || {
        let mut slot = dir
            .database
            .lock()
            .map_err(|_| anyhow::anyhow!("the entry database lock failed"))?;
        if slot.is_none() {
            *slot = Some(open(&dir)?);
        }
        work(slot.as_mut().expect("database is open"))
    })
    .await?
}

pub async fn save(
    dir: &Dir,
    writer: &str,
    log: &str,
    entries: &[Entry],
    drop: &[String],
) -> Result<()> {
    let (writer, log, entries, drop) = (
        writer.to_owned(),
        log.to_owned(),
        entries.to_vec(),
        drop.to_vec(),
    );
    blocking(dir, move |db| {
        let tx = db.transaction()?;
        for entry in &entries {
            let id = i64::try_from(entry.id).context("the entry ID is too large")?;
            tx.execute("INSERT OR REPLACE INTO entries(writer, log, id, ms, entry) VALUES (?1,?2,?3,?4,?5)",
                params![writer, log, id, entry.time.as_millisecond(), serde_json::to_string(entry)?])?;
        }
        if let Some(last) = entries.iter().map(|e| e.id).max() {
            let keep = last.saturating_add(1).saturating_sub(LIMIT);
            tx.execute("DELETE FROM entries WHERE writer=?1 AND log=?2 AND id < ?3", params![writer, log, i64::try_from(keep)?])?;
        }
        for old in drop {
            tx.execute("DELETE FROM entries WHERE writer=?1 AND log=?2", params![writer, old])?;
        }
        tx.commit()?;
        Ok(())
    }).await
}

pub async fn all(dir: &Dir, writer: &str) -> Result<Vec<Entry>> {
    let writer = writer.to_owned();
    blocking(dir, move |db| {
        let mut query = db.prepare("SELECT entry FROM entries WHERE writer=?1 ORDER BY log, id")?;
        let rows = query.query_map([writer], |row| row.get::<_, String>(0))?;
        rows.map(|row| Ok(serde_json::from_str(&row?)?)).collect()
    })
    .await
}

pub async fn rename_log(dir: &Dir, writer: &str, from: &str, to: &str) -> Result<()> {
    if from == to {
        return Ok(());
    }
    let (writer, from, to) = (writer.to_owned(), from.to_owned(), to.to_owned());
    blocking(dir, move |db| {
        let tx = db.transaction()?;
        tx.execute("INSERT OR IGNORE INTO entries SELECT writer,?3,id,ms,entry FROM entries WHERE writer=?1 AND log=?2", params![writer,from,to])?;
        tx.execute("DELETE FROM entries WHERE writer=?1 AND log=?2", params![writer,from])?;
        tx.commit()?;
        Ok(())
    }).await
}

pub async fn page(dir: &Dir, before: &str, limit: u32, writer: &str) -> Result<Value> {
    let cursor: Option<(i64, String, String, i64)> = if before.is_empty() {
        None
    } else {
        Some(serde_json::from_str(before)?)
    };
    let writer = writer.to_owned();
    blocking(dir, move |db| {
        let select = "SELECT writer, log, ms, entry FROM entries";
        let order = "ORDER BY ms DESC, writer DESC, log DESC, id DESC LIMIT :limit";
        let filter = if writer.is_empty() { "" } else { "writer=:writer" };
        let predicate = if cursor.is_some() { "(ms,writer,log,id) < (:ms,:before_writer,:log,:id)" } else { "" };
        let conditions: Vec<_> = [filter, predicate].into_iter().filter(|s| !s.is_empty()).collect();
        let sql = if conditions.is_empty() { format!("{select} {order}") } else { format!("{select} WHERE {} {order}", conditions.join(" AND ")) };
        let mut query = db.prepare(&sql)?;
        if !writer.is_empty() { query.raw_bind_parameter(query.parameter_index(":writer")?.context("missing writer parameter")?, writer)?; }
        if let Some((ms, writer, log, id)) = cursor {
            for (key, value) in [(":ms", rusqlite::types::Value::Integer(ms)), (":before_writer", writer.into()), (":log", log.into()), (":id", id.into())] {
                query.raw_bind_parameter(query.parameter_index(key)?.context("missing cursor parameter")?, value)?;
            }
        }
        query.raw_bind_parameter(query.parameter_index(":limit")?.context("missing page limit")?, i64::from(limit.min(500)))?;
        let mut rows = query.raw_query();
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            let entry: Entry = serde_json::from_str(&row.get::<_, String>(3)?)?;
            out.push(json!({"writer": row.get::<_, String>(0)?, "log": row.get::<_, String>(1)?, "ms": row.get::<_, i64>(2)?, "entry": entry}));
        }
        Ok(Value::Array(out))
    }).await
}

#[cfg(test)]
mod tests {
    use super::*;
    fn entry(id: u64) -> Entry {
        serde_json::from_value(json!({"v":1,"id":id,"time":"2026-11-01T10:00:00Z","source":"test","kind":"event","body":format!("Entry {id}")})).unwrap()
    }
    #[tokio::test]
    async fn pages_are_bounded_and_keep_logs_and_writers_separate() {
        let root = tempfile::tempdir().unwrap();
        let dir = Dir::new(root.path().into());
        let batch: Vec<_> = (1..=1200).map(entry).collect();
        save(&dir, "a", "first", &batch, &[]).await.unwrap();
        save(&dir, "a", "second", &[entry(1)], &[]).await.unwrap();
        save(&dir, "b", "first", &[entry(1)], &[]).await.unwrap();
        let first = page(&dir, "", 100, "a").await.unwrap();
        assert_eq!(first.as_array().unwrap().len(), 100);
        let last = first.as_array().unwrap().last().unwrap();
        let cursor =
            json!([last["ms"], last["writer"], last["log"], last["entry"]["id"]]).to_string();
        let second = page(&dir, &cursor, 100, "a").await.unwrap();
        assert_eq!(second.as_array().unwrap().len(), 100);
        assert_ne!(first[0], second[0]);
        assert_eq!(all(&dir, "a").await.unwrap().len(), 1201);
        assert_eq!(all(&dir, "b").await.unwrap().len(), 1);
        rename_log(&dir, "a", "first", "moved").await.unwrap();
        assert_eq!(all(&dir, "a").await.unwrap().len(), 1201);
        save(&dir, "a", "second", &[], &["moved".into()])
            .await
            .unwrap();
        assert_eq!(all(&dir, "a").await.unwrap().len(), 1);
    }
    #[tokio::test]
    async fn operations_reuse_the_connection() {
        let root = tempfile::tempdir().unwrap();
        let dir = Dir::new(root.path().into());
        blocking(&dir, |db| {
            db.execute_batch("CREATE TEMP TABLE connection_test (value INTEGER); INSERT INTO connection_test VALUES (42)")?;
            Ok(())
        }).await.unwrap();
        let clone = dir.clone();
        let value: i64 = blocking(&clone, |db| {
            Ok(db.query_row("SELECT value FROM connection_test", [], |r| r.get(0))?)
        })
        .await
        .unwrap();
        assert_eq!(value, 42);
    }

    #[tokio::test]
    async fn retention_and_duplicate_replay_do_not_grow_the_database_rows() {
        let root = tempfile::tempdir().unwrap();
        let dir = Dir::new(root.path().into());
        let batch: Vec<_> = (1..=LIMIT + 10).map(entry).collect();
        save(&dir, "a", "log", &batch, &[]).await.unwrap();
        save(&dir, "a", "log", &batch, &[]).await.unwrap();
        let rows = all(&dir, "a").await.unwrap();
        assert_eq!(rows.len(), LIMIT as usize);
        assert_eq!(rows[0].id, 11);
        assert!(page(&dir, "invalid", 100, "").await.is_err());
        assert_eq!(page(&dir, "", 0, "").await.unwrap(), json!([]));
    }
}
