//! The yappr reader in a browser: it reads machines, receives their
//! deliveries, pairs with them, and sends answers. The page calls `App.invoke`
//! with a command name.
// Only the WebAssembly build and the tests use the readers.
#[cfg_attr(not(any(target_family = "wasm", test)), allow(dead_code))]
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
