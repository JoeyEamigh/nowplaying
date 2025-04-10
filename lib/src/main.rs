use nowplaying::get_player;

mod monitoring;

#[tokio::main]
async fn main() -> nowplaying::Result<()> {
  monitoring::init_logger();

  let (tx, mut rx) = tokio::sync::mpsc::channel(16);
  let mut player = get_player(tx).await?;

  player.subscribe().await?;

  loop {
    tokio::select! {
      Some(info) = rx.recv() => {
        tracing::info!("now playing: {:?}", info);
      }
      _ = monitoring::wait_for_signal() => break,
    }
  }

  player.unsubscribe().await?;

  Ok(())
}
