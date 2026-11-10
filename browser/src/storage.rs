//! Browser storage for the reader: identity, cache, outbox, and relay
//! settings. Each value is one JSON string in `localStorage`.
use std::str::FromStr;

use anyhow::{Context, Result};
use iroh::SecretKey;
use serde::{de::DeserializeOwned, Serialize};
use yappr::net::RelaySettings;

use crate::readers::Readers;

/// The storage of this page. The app code passes it where the native code
/// passes a data directory.
#[derive(Debug, Default, Clone, Copy)]
pub struct Dir;

fn local() -> Result<web_sys::Storage> {
    web_sys::window()
        .context("the page has no window")?
        .local_storage()
        .map_err(|e| anyhow::anyhow!("{e:?}"))?
        .context("the browser has no local storage")
}

/// The value at `key`, or the default value when the key does not exist.
pub fn read<T: DeserializeOwned + Default>(_: &Dir, key: &str) -> Result<T> {
    match local()?
        .get_item(key)
        .map_err(|e| anyhow::anyhow!("{e:?}"))?
    {
        Some(text) => serde_json::from_str(&text).with_context(|| format!("read {key}")),
        None => Ok(T::default()),
    }
}

pub fn write<T: Serialize>(_: &Dir, key: &str, value: &T) -> Result<()> {
    local()?
        .set_item(key, &serde_json::to_string(value)?)
        .map_err(|e| anyhow::anyhow!("cannot save {key}: {e:?}"))
}

/// The identity of this page. Create it if it does not exist.
pub fn load_or_create_secret(_: &Dir) -> Result<SecretKey> {
    let saved: Option<String> = read(&Dir, "yappr.secret")?;
    if let Some(text) = saved {
        return SecretKey::from_str(&text).context("the saved identity is not valid");
    }
    let key = SecretKey::generate();
    write(
        &Dir,
        "yappr.secret",
        &data_encoding::HEXLOWER.encode(&key.to_bytes()),
    )?;
    Ok(key)
}

/// Relay settings in browser storage.
pub trait RelayStore: Sized {
    fn load(dir: &Dir) -> Result<Self>;
    fn save(&self, dir: &Dir) -> Result<()>;
}

impl RelayStore for RelaySettings {
    fn load(_: &Dir) -> Result<Self> {
        let settings: Self = read(&Dir, "yappr.network")?;
        settings.validate()?;
        Ok(settings)
    }
    fn save(&self, _: &Dir) -> Result<()> {
        self.validate()?;
        write(&Dir, "yappr.network", self)
    }
}

impl Readers {
    pub fn load(_: &Dir) -> Result<Self> {
        let book: Self = read(&Dir, "yappr.readers")?;
        book.validate()?;
        Ok(book)
    }
    pub fn save(&self, _: &Dir) -> Result<()> {
        self.validate()?;
        write(&Dir, "yappr.readers", self)
    }
}

/// A synchronous browser update runs without another task between its reads and writes.
pub struct OutboxGuard;
pub fn lock_outbox(_: &Dir) -> Result<OutboxGuard> {
    Ok(OutboxGuard)
}
