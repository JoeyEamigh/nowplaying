use serde::{Deserialize, Serialize};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

#[derive(Default, derive_more::Debug, Serialize, Deserialize, Clone)]
pub struct NowPlaying {
  pub album: Option<String>,
  pub artist: Option<Vec<String>>,
  pub playlist: Option<String>,
  pub playlist_id: Option<String>,
  pub track_name: String,
  pub shuffle_state: Option<bool>,
  pub repeat_state: Option<String>, // "off", "all", "track"
  pub is_playing: bool,
  pub can_fast_forward: bool,
  pub can_skip: bool,
  pub can_like: bool,
  pub can_change_volume: bool,
  pub can_set_output: bool,
  pub track_duration: Option<u32>,
  pub track_progress: Option<u32>,
  pub playback_rate: Option<f64>, // 1.0 = normal speed, 0.5 = half speed, 2.0 = double speed
  pub volume: u8,                 // percentage 0-100
  pub device: Option<String>,     // Name of device that is playing the audio
  pub id: Option<String>,         // A way to identify the current song (is used for certain actions)
  pub device_id: Option<String>,  // a way to identify the current device if needed
  pub url: Option<String>,        // the url of the current song
  #[debug(skip)]
  pub thumbnail: Option<String>, // either a path on disk or a base64 encoding that includes data:image/png;base64, at the beginning
}

pub type StateTx = tokio::sync::mpsc::Sender<NowPlaying>;
pub type StateRx = tokio::sync::mpsc::Receiver<NowPlaying>;

#[derive(Debug, Clone)]
pub enum CommandData {
  Play,
  Pause,
  PlayPause,
  NextTrack,
  PreviousTrack,
  SeekTo(u32),   // position in milliseconds
  SetVolume(u8), // volume as 0-100
  SetShuffle(bool),
}

#[derive(Debug, Clone)]
pub struct Command {
  pub to: Option<String>, // the id of the device to send the command to
  pub data: CommandData,
}

pub type CommandTx = tokio::sync::mpsc::Sender<Command>;
pub type CommandRx = tokio::sync::mpsc::Receiver<Command>;

#[async_trait::async_trait]
pub trait Player: Send {
  async fn init(tx: StateTx) -> Result<Box<Self>>
  where
    Self: Sized;

  async fn subscribe(&mut self) -> Result<()>;
  async fn unsubscribe(&mut self) -> Result<()>;
  async fn send_command(&mut self, command: Command) -> Result<()>;
}

pub async fn get_player(tx: StateTx) -> Result<Box<dyn Player>> {
  #[cfg(target_os = "linux")]
  let player = linux::LinuxPlayer::init(tx).await;
  #[cfg(target_os = "macos")]
  let player = macos::MacPlayer::init(tx).await;
  #[cfg(target_os = "windows")]
  let player = windows::WindowsPlayer::init(tx).await;

  Ok(player?)
}

pub type Result<T> = std::result::Result<T, Error>;
#[derive(Debug, thiserror::Error)]
#[allow(clippy::large_enum_variant)]
pub enum Error {
  #[cfg(target_os = "linux")]
  #[error(transparent)]
  ZBus(#[from] zbus::Error),
  #[cfg(target_os = "linux")]
  #[error(transparent)]
  ZBusFdo(#[from] zbus::fdo::Error),
  #[cfg(target_os = "linux")]
  #[error(transparent)]
  ZBusZVariant(#[from] zbus::zvariant::Error),
  #[error(transparent)]
  StateCommunication(#[from] tokio::sync::mpsc::error::SendError<NowPlaying>),
  #[error(transparent)]
  CommandCommunication(#[from] tokio::sync::mpsc::error::SendError<Command>),
  #[cfg(target_os = "windows")]
  #[error(transparent)]
  Windows(#[from] windows::Error),
  #[cfg(target_os = "windows")]
  #[error(transparent)]
  JoinHandle(#[from] tokio::task::JoinError),
}
