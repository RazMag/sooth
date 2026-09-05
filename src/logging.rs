use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Initializes structured logging to stdout. When `sooth` runs as a systemd
/// user service, systemd captures unit stdout into the journal natively, so
/// no separate journal-writing crate is needed for v1 -- `journalctl --user
/// -u sooth.service` just works.
pub fn init(configured_filter: Option<&str>) {
    let filter = configured_filter
        .map(String::from)
        .or_else(|| std::env::var("RUST_LOG").ok())
        .unwrap_or_else(|| "sooth=info,tower_http=info,zbus=warn".to_string());

    tracing_subscriber::registry()
        .with(EnvFilter::new(filter))
        .with(tracing_subscriber::fmt::layer())
        .init();
}
