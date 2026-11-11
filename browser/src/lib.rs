//! Shared reader commands, delivery handling, and answer retries.
mod app;
#[cfg_attr(not(target_family = "wasm"), path = "native/entries.rs")]
mod entries;
#[cfg(target_family = "wasm")]
mod notification;
mod outbox;
mod readers;
#[cfg_attr(not(target_family = "wasm"), path = "native/storage.rs")]
mod storage;

pub use app::App;
