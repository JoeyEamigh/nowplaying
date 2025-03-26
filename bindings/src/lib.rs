#![allow(clippy::missing_safety_doc)]

mod conversion;
mod monitoring;

use std::sync::Arc;

use monitoring::{LogMessage, init_logger};
use napi::{bindgen_prelude::*, threadsafe_function::*, tokio};
use napi_derive::napi;

use conversion::{NowPlayingMessage, PlayerCommand};

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
    let (tx, command_rx) = tokio::sync::mpsc::channel(16);
    self.tx = Some(tx);

    let cancel_token = CancellationToken::new();
    self.cancel_token = cancel_token.clone();

    let callback = Arc::clone(&self.callback);
    let mut worker = NowPlayingWorker::init(command_rx, callback, cancel_token).await?;
    self.handle = Some(tokio::spawn(async move { worker.event_loop().await }));

    Ok(())
  }

  #[napi]
  pub async unsafe fn unsubscribe(&mut self) -> Result<()> {
    if let Some(handle) = self.handle.take() {
      self.cancel_token.cancel();
      handle
        .await
        .map_err(|e| napi::Error::from_reason(format!("failed to join worker: {:?}", e)))?;
    }

    Ok(())
  }

  #[napi]
  pub async fn send_command(&self, command: PlayerCommand) -> Result<()> {
    if let Some(tx) = &self.tx {
      tx.send(command.into())
        .await
        .map_err(|e| napi::Error::from_reason(format!("failed to send command: {:?}", e)))?;
    } else {
      return Err(napi::Error::from_reason("worker not initialized"));
    }

    Ok(())
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

    let player = get_player(tx)
      .await
      .map_err(|e| napi::Error::from_reason(format!("failed to create player: {:?}", e)))?;

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
  }
}
