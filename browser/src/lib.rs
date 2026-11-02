//! The yappr reader in a browser: it reads machines, receives their
//! deliveries, pairs with them, and sends answers. The page calls `App.invoke`
//! with a command name.
mod readers;

#[cfg(target_family = "wasm")]
mod app;
#[cfg(target_family = "wasm")]
mod entries;
#[cfg(target_family = "wasm")]
mod outbox;
#[cfg(target_family = "wasm")]
mod storage;

#[cfg(target_family = "wasm")]
pub use app::App;
