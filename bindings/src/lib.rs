#![allow(clippy::missing_safety_doc)]

mod conversion;
mod monitoring;

use std::sync::Arc;

use monitoring::{LogMessage, init_logger};
use napi::{bindgen_prelude::*, threadsafe_function::*, tokio};
use napi_derive::napi;

use conversion::{NowPlayingMessage, PlayerCommand, PlayerCommandData};

use nowplaying::{CommandRx, CommandTx, Player, StateRx, get_player};
use tokio_util::sync::CancellationToken;

type NowPlayingCallback = ThreadsafeFunction<NowPlayingMessage, Unknown, NowPlayingMessage, false>;

#[napi(object)]
#[derive(Default)]
pub struct NowPlayingOptions<'a> {
  pub log_level_directive: Option<String>,
  #[napi(ts_type = "(event: LogMessage) => void")]
  pub log_callback: Option<Function<'a, LogMessage>>,
}

#[napi]
pub struct NowPlaying {
  tx: Option<CommandTx>,
  callback: Arc<NowPlayingCallback>,
  cancel_token: CancellationToken,
  handle: Option<tokio::task::JoinHandle<()>>,
}

#[napi]
impl NowPlaying {
  #[napi(
    constructor,
    ts_args_type = "callback: (event: NowPlayingMessage) => void, options?: NowPlayingOptions"
  )]
  pub fn new(callback: Function<NowPlayingMessage>, options: Option<NowPlayingOptions>) -> Result<Self> {
    let callback = Arc::new(callback.build_threadsafe_function().build()?);

    let options = options.unwrap_or_default();
    let log_fn = options
      .log_callback
      .and_then(|callback| callback.build_threadsafe_function().build().ok())
      .map(Arc::new);
    init_logger(log_fn, options.log_level_directive);

    Ok(Self {
      callback,
      cancel_token: CancellationToken::new(),

      tx: None,
      handle: None,
    })
  }

  #[napi]
  pub async unsafe fn subscribe(&mut self) -> Result<()> {
    tracing::debug!("subscribing to now playing events");
    if !self.cancel_token.is_cancelled() {
      self.cancel_token.cancel();
    }

    self.cancel_token = CancellationToken::new();
    let (tx, command_rx) = tokio::sync::mpsc::channel(16);
    self.tx = Some(tx);

    let mut worker = NowPlayingWorker::init(command_rx, self.callback.clone(), self.cancel_token.clone()).await?;
    self.handle = Some(tokio::spawn(async move { worker.event_loop().await }));

    Ok(())
  }

  #[napi]
  pub async unsafe fn unsubscribe(&mut self) -> Result<()> {
    tracing::debug!("unsubscribing from now playing events");
    self.cancel_token.cancel();

    if let Some(handle) = self.handle.take() {
      handle
        .await
        .map_err(|e| napi::Error::from_reason(format!("failed to join worker: {:?}", e)))?;
    }

    Ok(())
  }

  #[napi]
  pub async fn send_command(&self, command: PlayerCommand) -> Result<()> {
    tracing::debug!("sending command: {:?}", command);
    if let Some(tx) = &self.tx {
      tx.send(command.into())
        .await
        .map_err(|e| napi::Error::from_reason(format!("failed to send command: {:?}", e)))?;
    } else {
      return Err(napi::Error::from_reason("worker not initialized"));
    }

    Ok(())
  }

  #[napi]
  pub async fn play(&self, to: Option<String>) -> Result<()> {
    self
      .send_command(PlayerCommand {
        to,
        data: PlayerCommandData::Play,
      })
      .await
  }

  #[napi]
  pub async fn pause(&self, to: Option<String>) -> Result<()> {
    self
      .send_command(PlayerCommand {
        to,
        data: PlayerCommandData::Pause,
      })
      .await
  }

  #[napi]
  pub async fn play_pause(&self, to: Option<String>) -> Result<()> {
    self
      .send_command(PlayerCommand {
        to,
        data: PlayerCommandData::PlayPause,
      })
      .await
  }

  #[napi]
  pub async fn next_track(&self, to: Option<String>) -> Result<()> {
    self
      .send_command(PlayerCommand {
        to,
        data: PlayerCommandData::NextTrack,
      })
      .await
  }

  #[napi]
  pub async fn previous_track(&self, to: Option<String>) -> Result<()> {
    self
      .send_command(PlayerCommand {
        to,
        data: PlayerCommandData::PreviousTrack,
      })
      .await
  }

  #[napi]
  pub async fn seek_to(&self, position_ms: u32, to: Option<String>) -> Result<()> {
    self
      .send_command(PlayerCommand {
        to,
        data: PlayerCommandData::SeekTo { position_ms },
      })
      .await
  }

  #[napi]
  pub async fn set_volume(&self, volume: u8, to: Option<String>) -> Result<()> {
    self
      .send_command(PlayerCommand {
        to,
        data: PlayerCommandData::SetVolume { volume },
      })
      .await
  }

  #[napi]
  pub async fn set_shuffle(&self, shuffle: bool, to: Option<String>) -> Result<()> {
    self
      .send_command(PlayerCommand {
        to,
        data: PlayerCommandData::SetShuffle { shuffle },
      })
      .await
  }
}

pub struct NowPlayingWorker {
  state_rx: StateRx,
  command_rx: CommandRx,

  callback: Arc<NowPlayingCallback>,
  player: Box<dyn Player>,

  cancel_token: CancellationToken,
}

impl NowPlayingWorker {
  pub async fn init(
    command_rx: CommandRx,
    callback: Arc<NowPlayingCallback>,
    cancel_token: CancellationToken,
  ) -> Result<Self> {
    let (tx, state_rx) = tokio::sync::mpsc::channel(16);

    let mut player = get_player(tx)
      .await
      .map_err(|e| napi::Error::from_reason(format!("failed to create player: {:?}", e)))?;
    tracing::debug!("created player");

    player
      .subscribe()
      .await
      .map_err(|e| napi::Error::from_reason(format!("failed to subscribe to player: {:?}", e)))?;
    tracing::debug!("subscribed to player");

    Ok(Self {
      state_rx,
      command_rx,

      callback,
      player,

      cancel_token,
    })
  }

  pub async fn event_loop(&mut self) {
    loop {
      tokio::select! {
        Some(command) = self.command_rx.recv() => {
          if let Err(e) = self.player.send_command(command).await {
            tracing::error!("failed to send command: {:?}", e);
          }
        }
        Some(state) = self.state_rx.recv() => {
          let status = self.callback.call(state.into(), ThreadsafeFunctionCallMode::NonBlocking);
          if status != Status::Ok {
            tracing::error!("failed to call callback: {:?}", status);
          }
        }
        _ = self.cancel_token.cancelled() => {
          tracing::debug!("worker cancel token triggered, exiting event loop");
          break;
        }
      }
    }

    tracing::debug!("event loop exited");
    if let Err(e) = self.player.unsubscribe().await {
      tracing::error!("failed to unsubscribe from player: {:?}", e);
    } else {
      tracing::debug!("unsubscribed from player, exiting worker");
    }
  }
}
