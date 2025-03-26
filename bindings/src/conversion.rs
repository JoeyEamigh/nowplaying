use napi_derive::napi;
use nowplaying::{Command, CommandData, NowPlaying};

/// now playing info
#[napi(object)]
#[derive(derive_more::Debug, Clone)]
pub struct NowPlayingMessage {
  pub album: Option<String>,
  pub artist: Option<Vec<String>>,
  pub playlist: Option<String>,
  pub playlist_id: Option<String>,
  pub track_name: String,
  pub shuffle_state: Option<bool>,
  /// "off", "all", "track"
  pub repeat_state: Option<String>,
  pub is_playing: bool,
  pub can_fast_forward: bool,
  pub can_skip: bool,
  pub can_like: bool,
  pub can_change_volume: bool,
  pub can_set_output: bool,
  pub track_duration: Option<u32>,
  pub track_progress: Option<u32>,
  /// 1.0 = normal speed, 0.5 = half speed, 2.0 = double speed
  pub playback_rate: Option<f64>,
  /// percentage 0-100
  pub volume: u8,
  /// Name of device that is playing the audio
  pub device: Option<String>,
  /// A way to identify the current song (if possible - doesn't work on macos)
  pub id: Option<String>,
  /// a way to identify the current device if needed
  pub device_id: Option<String>,
  /// the url of the current song
  pub url: Option<String>,
  #[debug(skip)]
  /// either a path on disk or a base64 encoding that includes data:image/png;base64, at the beginning
  pub thumbnail: Option<String>,
}

impl From<NowPlaying> for NowPlayingMessage {
  fn from(state: NowPlaying) -> Self {
    NowPlayingMessage {
      album: state.album,
      artist: state.artist,
      playlist: state.playlist,
      playlist_id: state.playlist_id,
      track_name: state.track_name,
      shuffle_state: state.shuffle_state,
      repeat_state: state.repeat_state,
      is_playing: state.is_playing,
      can_fast_forward: state.can_fast_forward,
      can_skip: state.can_skip,
      can_like: state.can_like,
      can_change_volume: state.can_change_volume,
      can_set_output: state.can_set_output,
      track_duration: state.track_duration,
      track_progress: state.track_progress,
      playback_rate: state.playback_rate,
      volume: state.volume,
      device: state.device,
      id: state.id,
      device_id: state.device_id,
      url: state.url,
      thumbnail: state.thumbnail,
    }
  }
}

#[napi]
#[derive(Debug, Clone)]
pub enum PlayerCommandData {
  Play,
  Pause,
  PlayPause,
  NextTrack,
  PreviousTrack,
  SeekTo { position_ms: u32 }, // position in milliseconds
  SetVolume { volume: u8 },    // volume as 0-100
  SetShuffle { shuffle: bool },
}

impl From<PlayerCommandData> for CommandData {
  fn from(data: PlayerCommandData) -> Self {
    match data {
      PlayerCommandData::Play => CommandData::Play,
      PlayerCommandData::Pause => CommandData::Pause,
      PlayerCommandData::PlayPause => CommandData::PlayPause,
      PlayerCommandData::NextTrack => CommandData::NextTrack,
      PlayerCommandData::PreviousTrack => CommandData::PreviousTrack,
      PlayerCommandData::SeekTo { position_ms } => CommandData::SeekTo(position_ms),
      PlayerCommandData::SetVolume { volume } => CommandData::SetVolume(volume),
      PlayerCommandData::SetShuffle { shuffle } => CommandData::SetShuffle(shuffle),
    }
  }
}

/// Command to send to the player
#[napi(object)]
#[derive(Debug, Clone)]
pub struct PlayerCommand {
  /// the id of the device to send the command to
  ///
  /// if None, the command is sent to all devices; ignored on macos
  pub to: Option<String>,
  pub data: PlayerCommandData,
}

impl From<PlayerCommand> for Command {
  fn from(command: PlayerCommand) -> Self {
    Command {
      to: command.to,
      data: command.data.into(),
    }
  }
}
