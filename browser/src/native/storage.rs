//! Private native files for identity, machine metadata, and queued answers.
use std::path::PathBuf;

use anyhow::{Context, Result};
use iroh::SecretKey;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use yappr::{disk, net::RelaySettings, pairing::Invitation};

use crate::readers::{PendingPair, Readers};

#[derive(Debug, Clone, Default)]
pub struct Dir {
    pub path: PathBuf,
    pub database: std::sync::Arc<std::sync::Mutex<Option<rusqlite::Connection>>>,
}
impl Dir {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            ..Self::default()
        }
    }
}

fn file(key: &str) -> Result<&'static str> {
    match key {
        "yappr.readers" => Ok("readers.json"),
        "yappr.outbox" => Ok("outbox.json"),
        "yappr.network" => Ok("network.json"),
        _ => anyhow::bail!("unknown storage key"),
    }
}

pub fn read<T: DeserializeOwned + Default>(dir: &Dir, key: &str) -> Result<T> {
    let path = dir.path.join(file(key)?);
    if !path.try_exists()? {
        return Ok(T::default());
    }
    let value: serde_json::Value = disk::read(&path)?;
    let value = if key == "yappr.outbox" && value.is_object() {
        value
            .get("deliveries")
            .cloned()
            .context("the saved outbox has no deliveries")?
    } else {
        value
    };
    Ok(serde_json::from_value(value)?)
}

pub fn write<T: Serialize>(dir: &Dir, key: &str, value: &T) -> Result<()> {
    let value = serde_json::to_value(value)?;
    let value = if key == "yappr.outbox" {
        serde_json::json!({"deliveries": value})
    } else {
        value
    };
    disk::write(&dir.path.join(file(key)?), &value)
}

pub fn load_or_create_secret(dir: &Dir) -> Result<SecretKey> {
    yappr::net::load_or_create_secret(&dir.path)
}

pub trait RelayStore: Sized {
    fn load(dir: &Dir) -> Result<Self>;
    fn save(&self, dir: &Dir) -> Result<()>;
}
impl RelayStore for RelaySettings {
    fn load(dir: &Dir) -> Result<Self> {
        let settings: Self = read(dir, "yappr.network")?;
        settings.validate()?;
        Ok(settings)
    }
    fn save(&self, dir: &Dir) -> Result<()> {
        self.validate()?;
        let _lock = disk::lock(&dir.path, "network.lock")?;
        write(dir, "yappr.network", self)
    }
}

impl Readers {
    pub fn load(dir: &Dir) -> Result<Self> {
        let path = dir.path.join("readers.json");
        let book = if path.try_exists()? {
            let book: Self = disk::read(&path)?;
            if book.writers.values().any(|w| !w.entries.is_empty()) {
                let backup = dir.path.join("readers.inline.json");
                if !backup.try_exists()? {
                    disk::write(&backup, &book)?;
                }
            }
            book
        } else {
            #[derive(Default, Deserialize)]
            #[serde(default)]
            struct Legacy {
                ticket: Option<String>,
                entries: Vec<yappr::entry::Entry>,
                invitation: Option<Invitation>,
                writer_name: Option<String>,
            }
            let old: Legacy = disk::read(&dir.path.join("reader.json"))?;
            let mut book = Self::default();
            if let Some(ticket) = old.ticket {
                let id = book.add(&ticket.parse()?, old.writer_name)?;
                book.writers
                    .get_mut(&id)
                    .context("the machine does not exist")?
                    .entries = old.entries;
            }
            book.invitation = old.invitation.map(|invitation| PendingPair {
                invitation,
                name: "My device".into(),
            });
            book
        };
        book.validate()?;
        Ok(book)
    }
    pub fn save(&self, dir: &Dir) -> Result<()> {
        self.validate()?;
        write(dir, "yappr.readers", self)
    }
}

/// Serialize changes to the answer list across native worker tasks.
pub fn lock_outbox(dir: &Dir) -> Result<std::fs::File> {
    disk::lock(&dir.path, "outbox.lock")
}
