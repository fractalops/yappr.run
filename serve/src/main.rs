//! Serves the built browser app for tests. A static host replaces it.
use std::path::PathBuf;

use clap::Parser;
use tower_http::{
    compression::CompressionLayer,
    services::{ServeDir, ServeFile},
    set_header::SetResponseHeaderLayer,
};

#[derive(Parser)]
struct Options {
    /// The directory of the built app: index.html, _app/ and wasm/.
    #[clap(long = "static")]
    static_dir: PathBuf,
    /// The address to listen on.
    #[clap(long, default_value = "127.0.0.1:8081")]
    listen: std::net::SocketAddr,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let options = Options::parse();
    let files = ServeDir::new(&options.static_dir)
        .fallback(ServeFile::new(options.static_dir.join("index.html")));
    let router = axum::Router::new()
        .fallback_service(files)
        // The browser checks every file again before it uses a saved copy. A
        // page and its WebAssembly reader must come from the same build.
        .layer(SetResponseHeaderLayer::overriding(
            axum::http::header::CACHE_CONTROL,
            axum::http::HeaderValue::from_static("no-cache"),
        ))
        // The WebAssembly reader is large. Compression makes the download
        // several times smaller.
        .layer(CompressionLayer::new());
    let listener = tokio::net::TcpListener::bind(options.listen).await?;
    eprintln!(
        "Serving {} on http://{}",
        options.static_dir.display(),
        options.listen
    );
    axum::serve(listener, router).await?;
    Ok(())
}
