pub fn init_logger() {
  use tracing::metadata::LevelFilter;
  use tracing_subscriber::fmt::format::FmtSpan;
  use tracing_subscriber::{
    EnvFilter, Layer, filter::Directive, fmt, prelude::__tracing_subscriber_SubscriberExt, util::SubscriberInitExt,
  };

  // directives for debug builds
  #[cfg(debug_assertions)]
  let default_directive = Directive::from(LevelFilter::TRACE);

  #[cfg(debug_assertions)]
  let filter_directives = if let Ok(filter) = std::env::var("RUST_LOG") {
    filter
  } else {
    "nowplaying=trace".to_string()
  };

  // directives for release builds
  #[cfg(not(debug_assertions))]
  let default_directive = Directive::from(LevelFilter::INFO);

  #[cfg(not(debug_assertions))]
  let filter_directives = if let Ok(filter) = std::env::var("RUST_LOG") {
    filter
  } else {
    "nowplaying=info".to_string()
  };

  let filter = EnvFilter::builder()
    .with_default_directive(default_directive)
    .parse_lossy(filter_directives);

  tracing_subscriber::registry()
    .with(fmt::layer().with_span_events(FmtSpan::CLOSE).with_filter(filter))
    .init();

  tracing::debug!("initialized logger");
}

pub async fn wait_for_signal() {
  #[cfg(unix)]
  wait_for_signal_unix().await;
  #[cfg(not(unix))]
  wait_for_signal_windows().await;
}

#[cfg(not(unix))]
pub async fn wait_for_signal_windows() {
  use tokio::signal::ctrl_c;

  ctrl_c().await.expect("failed to install ctrl-c handler");
  tracing::info!("ctrl-c received, shutting down");
}

#[cfg(unix)]
pub async fn wait_for_signal_unix() {
  use tokio::signal::{
    ctrl_c,
    unix::{SignalKind, signal},
  };

  let mut signal_terminate = signal(SignalKind::terminate()).expect("could not create signal handler");
  let mut signal_interrupt = signal(SignalKind::interrupt()).expect("could not create signal handler");

  tokio::select! {
    _ = signal_terminate.recv() => tracing::info!("received SIGTERM, shutting down"),
    _ = signal_interrupt.recv() => tracing::info!("received SIGINT, shutting down"),
    _ = ctrl_c() => tracing::info!("ctrl-c received, shutting down"),
  };
}
