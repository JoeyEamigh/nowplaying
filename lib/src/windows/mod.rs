use std::collections::HashMap;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use windows::Media::Control::{
  GlobalSystemMediaTransportControlsSession, GlobalSystemMediaTransportControlsSessionManager,
  GlobalSystemMediaTransportControlsSessionMediaProperties, GlobalSystemMediaTransportControlsSessionPlaybackStatus,
};

use crate::{Command, CommandData, CommandRx, CommandTx, NowPlaying, Player, Result, StateTx};
pub use windows::core::Error;

struct PlayerSession {
  session_id: String,
  session: GlobalSystemMediaTransportControlsSession,
  state: NowPlaying,

  tx: StateTx,
  rx: CommandRx,
  session_rx: SessionRx,
  session_tx: SessionTx,

  cancel_token: CancellationToken,
}

impl PlayerSession {
  pub fn spawn(
    session: GlobalSystemMediaTransportControlsSession,
    tx: StateTx,
    rx: CommandRx,
    cancel_token: CancellationToken,
  ) -> Result<JoinHandle<()>> {
    tracing::debug!("spawning new player connection");

    let (session_tx, session_rx) = tokio::sync::mpsc::channel(16);
    Ok(tokio::spawn(async move {
      Self {
        session_id: session.SourceAppUserModelId().unwrap_or_default().to_string(),
        session,
        state: NowPlaying::default(),

        tx,
        rx,
        session_rx,
        session_tx,

        cancel_token,
      }
      .event_loop()
      .await
    }))
  }

  async fn event_loop(&mut self) {
    if let Err(err) = self.fetch_state().await {
      tracing::error!("({}) error fetching initial state: {:?}", self.session_id, err);
    }

    if let Err(err) = self.tx.send(self.state.clone()).await {
      tracing::error!("({}) error sending initial state: {:?}", self.session_id, err);
    }

    tracing::debug!("({}) player session initialized", &self.session_id);

    let media_session_id = self.session_id.clone();
    let media_session_tx = self.session_tx.clone();
    let Ok(media_props_changed_token) =
      self
        .session
        .MediaPropertiesChanged(&windows::Foundation::TypedEventHandler::new(move |session, _| {
          tracing::debug!("({}) media properties changed: {:?}", media_session_id, &session);
          if let Err(err) = media_session_tx.try_send(true) {
            tracing::error!(
              "({}) error sending playback info changed event: {:?}",
              media_session_id,
              err
            );
          }
          Ok(())
        }))
    else {
      tracing::error!(
        "({}) failed to register media properties changed handler",
        &self.session_id
      );
      return;
    };

    let playback_session_id = self.session_id.clone();
    let playback_session_tx = self.session_tx.clone();
    let Ok(playback_info_changed_token) =
      self
        .session
        .PlaybackInfoChanged(&windows::Foundation::TypedEventHandler::new(move |session, _| {
          tracing::debug!("({}) playback info changed: {:?}", playback_session_id, &session);
          if let Err(err) = playback_session_tx.try_send(true) {
            tracing::error!(
              "({}) error sending playback info changed event: {:?}",
              playback_session_id,
              err
            );
          }
          Ok(())
        }))
    else {
      tracing::error!(
        "({}) failed to register playback info changed handler",
        &self.session_id
      );
      return;
    };

    loop {
      tokio::select! {
        _ = self.session_rx.recv() => {
          tracing::debug!("({}) playback info changed event received", &self.session_id);
          if let Err(err) = self.fetch_state().await {
            tracing::error!("({}) error updating state: {:?}", self.session_id, err);
            continue;
          }
          if let Err(err) = self.tx.send(self.state.clone()).await {
            tracing::error!("({}) error sending state: {:?}", self.session_id, err);
          }
        },
        Some(command) = self.rx.recv() => {
          if let Err(err) = self.handle_command(command).await {
            tracing::error!("({}) error handling command: {:?}", &self.session_id, err);
          }
        },
        _ = self.cancel_token.cancelled() => break,
      }
    }

    tracing::debug!(
      "({}) cancellation received, shutting down player session thread",
      &self.session_id
    );

    if let Err(err) = self.session.RemoveMediaPropertiesChanged(media_props_changed_token) {
      tracing::error!("Error removing media properties changed handler: {:?}", err);
    }
    if let Err(err) = self.session.RemovePlaybackInfoChanged(playback_info_changed_token) {
      tracing::error!("Error removing playback info changed handler: {:?}", err);
    }
  }

  async fn handle_command(&mut self, command: Command) -> Result<()> {
    tracing::debug!("({}) received command: {:?}", self.session_id, command);

    match command.data {
      CommandData::Play => {
        self.session.TryPlayAsync()?.await?;
      }
      CommandData::Pause => {
        self.session.TryPauseAsync()?.await?;
      }
      CommandData::PlayPause => {
        self.session.TryTogglePlayPauseAsync()?.await?;
      }
      CommandData::NextTrack => {
        self.session.TrySkipNextAsync()?.await?;
      }
      CommandData::PreviousTrack => {
        self.session.TrySkipPreviousAsync()?.await?;
      }
      CommandData::SeekTo(position) => {
        self.session.TryChangePlaybackPositionAsync(position as i64)?.await?;
      }
      CommandData::SetVolume(_volume) => {
        tracing::warn!(
          "({}) i don't really want to figure out how to change media volume :)",
          self.session_id
        );
      }
      CommandData::SetShuffle(shuffle) => {
        self.session.TryChangeShuffleActiveAsync(shuffle)?.await?;
      }
    }

    Ok(())
  }

  async fn fetch_state(&mut self) -> Result<()> {
    if let Ok(props) = self.session.TryGetMediaPropertiesAsync() {
      if let Ok(props) = props.await {
        self.state.track_name = props.Title().unwrap_or_default().to_string();

        if let Ok(album_title) = props.AlbumTitle() {
          self.state.album = Some(album_title.to_string());
        } else {
          self.state.album = None;
          tracing::debug!("({}) could not get album title", self.session_id);
        }

        self.state.artist = Some(vec![props.Artist().unwrap_or_default().to_string()]);

        // if let Ok(thumbnail) = self.fetch_thumbnail(&props).await {
        //   self.state.thumbnail = Some(thumbnail);
        // } else {
        //   tracing::warn!("({}) failed to fetch thumbnail", &self.session_id);
        // }

        // TODO: actually get the thumbnail
        if let Ok(thumbnail) = props.Thumbnail() {
          tracing::debug!("({}) thumbnail: {:?}", self.session_id, thumbnail);
        }
      } else {
        tracing::trace!("({}) failed to await media properties", self.session_id);
      }
    } else {
      tracing::error!("({}) failed to get media properties", self.session_id);
    }

    if let Ok(playback) = self.session.GetPlaybackInfo() {
      if let Ok(status) = playback.PlaybackStatus() {
        self.state.is_playing = status == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing;
      } else {
        tracing::error!("({}) failed to get playback status", self.session_id);
      }

      if let Ok(rate) = playback.PlaybackRate() {
        if let Ok(rate_double) = rate.GetDouble() {
          self.state.playback_rate = Some(rate_double);
        } else {
          tracing::trace!("({}) failed to get playback rate double", self.session_id);
        }
      } else {
        tracing::trace!("({}) failed to get playback rate", self.session_id);
      }

      if let Ok(shuffle) = playback.IsShuffleActive() {
        if let Ok(shuffle_bool) = shuffle.GetBoolean() {
          self.state.shuffle_state = Some(shuffle_bool);
        } else {
          tracing::trace!("({}) failed to get shuffle boolean", self.session_id);
        }
      } else {
        tracing::trace!("({}) failed to get shuffle state", self.session_id);
      }

      if let Ok(controls) = playback.Controls() {
        if let Ok(can_skip) = controls.IsNextEnabled() {
          self.state.can_skip = can_skip;
        } else {
          tracing::trace!("({}) failed to get next enabled state", self.session_id);
        }

        if let Ok(can_ff) = controls.IsFastForwardEnabled() {
          self.state.can_fast_forward = can_ff;
        } else {
          tracing::trace!("({}) failed to get fast forward enabled state", self.session_id);
        }
      } else {
        tracing::error!("({}) failed to get playback controls", self.session_id);
      }
    } else {
      tracing::error!("({}) failed to get playback info", self.session_id);
    }

    self.state.can_change_volume = false; // Not exposed via this API and i'm too lazy to figure it out
    self.state.can_set_output = false;
    self.state.can_like = false;

    if let Ok(timeline) = self.session.GetTimelineProperties() {
      if let Ok(end_time) = timeline.EndTime() {
        self.state.track_duration = Some(end_time.Duration as u32);
      } else {
        tracing::trace!("({}) failed to get track duration", self.session_id);
      }

      if let Ok(position) = timeline.Position() {
        self.state.track_progress = Some(position.Duration as u32);
      } else {
        tracing::trace!("({}) failed to get track position", self.session_id);
      }
    } else {
      tracing::error!("({}) failed to get timeline properties", self.session_id);
    }

    self.state.device_id = Some(self.session_id.clone());

    Ok(())
  }

  async fn fetch_thumbnail(
    &self,
    props: &GlobalSystemMediaTransportControlsSessionMediaProperties,
  ) -> windows::core::Result<String> {
    use base64::prelude::*;

    let stream = props.Thumbnail()?.to_owned().OpenReadAsync()?.await?;
    let size = stream.Size()? as usize;

    let mut buffer = vec![0u8; size];

    let input_stream = stream.GetInputStreamAt(0)?;
    let reader = windows::Storage::Streams::DataReader::CreateDataReader(&input_stream)?;

    reader.LoadAsync(size as u32)?.await?;
    reader.ReadBytes(&mut buffer)?;
    reader.Close()?;

    Ok(BASE64_STANDARD.encode(&buffer))
  }
}

// bool is current session or not for session manager, doesn't matter for player session
type SessionTx = tokio::sync::mpsc::Sender<bool>;
type SessionRx = tokio::sync::mpsc::Receiver<bool>;

struct PlayerSessionHandle {
  cancel_token: CancellationToken,
  tx: CommandTx,
  _handle: JoinHandle<()>,
}

struct SessionManager {
  tx: StateTx,
  rx: CommandRx,
  session_rx: SessionRx,

  manager: GlobalSystemMediaTransportControlsSessionManager,
  sessions: HashMap<String, PlayerSessionHandle>,

  sessions_changed_token: i64,
  current_session_changed_token: i64,

  cancel_token: CancellationToken,
}

impl SessionManager {
  pub async fn spawn(tx: StateTx, rx: CommandRx, cancel_token: CancellationToken) -> Result<JoinHandle<()>> {
    tracing::debug!("creating new media session manager");

    let mut this = Self::init(tx, rx, cancel_token).await?;
    Ok(tokio::spawn(async move { this.event_loop().await }))
  }

  async fn init(tx: StateTx, rx: CommandRx, cancel_token: CancellationToken) -> Result<Self> {
    tracing::debug!("initializing media session manager");
    let manager = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()?.await?;
    let (session_tx, session_rx) = tokio::sync::mpsc::channel(16);

    let sessions_tx = session_tx.clone();
    let sessions_changed_token =
      manager.SessionsChanged(&windows::Foundation::TypedEventHandler::new(move |_, _| {
        tracing::debug!("sessions changed event received");
        if let Err(err) = sessions_tx.try_send(false) {
          tracing::error!("error sending sessions changed event: {:?}", err);
        }

        Ok(())
      }))?;

    let current_session_changed_token =
      manager.CurrentSessionChanged(&windows::Foundation::TypedEventHandler::new(move |_, _| {
        tracing::debug!("current session changed event received");
        if let Err(err) = session_tx.try_send(true) {
          tracing::error!("error sending sessions changed event: {:?}", err);
        }

        Ok(())
      }))?;

    Ok(Self {
      tx,
      rx,
      session_rx,

      manager,
      sessions: HashMap::new(),

      sessions_changed_token,
      current_session_changed_token,

      cancel_token,
    })
  }

  async fn event_loop(&mut self) {
    if let Err(err) = self.update_sessions().await {
      tracing::error!("Error updating sessions: {:?}", err);
    }

    loop {
      tokio::select! {
        _ = self.session_rx.recv() => {
          tracing::debug!("session changed event received");
          if let Err(err) = self.update_sessions().await {
            tracing::error!("Error updating sessions: {:?}", err);
          }
        },
        Some(command) = self.rx.recv() => {
          tracing::trace!("received command: {:?}", command);
          if let Some(to) = &command.to {
            if let Some(session) = self.sessions.get_mut(to) {
              if let Err(err) = session.tx.send(command.clone()).await {
                  tracing::error!("error sending command to session: {:?}", err);
              }
            } else {
              tracing::warn!("no session found for command: {:?}", command);
            }
          } else {
            for session in self.sessions.values_mut() {
              if let Err(err) = session.tx.send(command.clone()).await {
                tracing::error!("error sending command to session: {:?}", err);
              }
            }
          }
        },
        _ = self.cancel_token.cancelled() => break,
      }
    }

    tracing::debug!("cancellation received, shutting down session manager thread");

    if let Err(err) = self.manager.RemoveSessionsChanged(self.current_session_changed_token) {
      tracing::error!("Error removing current session changed handler: {:?}", err);
    }
    if let Err(err) = self.manager.RemoveCurrentSessionChanged(self.sessions_changed_token) {
      tracing::error!("Error removing sessions changed handler: {:?}", err);
    }

    for (id, session) in self.sessions.drain() {
      tracing::debug!("cancelling session thread for {}", id);
      session.cancel_token.cancel();
    }
  }

  async fn update_sessions(&mut self) -> windows::core::Result<()> {
    let sessions = self.manager.GetSessions()?;
    let mut seen_sessions = Vec::new();

    for i in 0..sessions.Size()? {
      let session = sessions.GetAt(i)?;
      let session_id = session.SourceAppUserModelId()?.to_string();
      seen_sessions.push(session_id.clone());

      if let std::collections::hash_map::Entry::Vacant(e) = self.sessions.entry(session_id.clone()) {
        tracing::debug!("new media session found: {}", session_id);
        let cancel_token = self.cancel_token.child_token();
        let (command_tx, command_rx) = tokio::sync::mpsc::channel(16);

        match PlayerSession::spawn(session, self.tx.clone(), command_rx, cancel_token.clone()) {
          Ok(handle) => {
            e.insert(PlayerSessionHandle {
              _handle: handle,
              tx: command_tx,
              cancel_token,
            });
          }
          Err(err) => tracing::error!("error spawning player session: {:?}", err),
        }
      }
    }

    let current_sessions: Vec<String> = self.sessions.keys().cloned().collect();
    for id in current_sessions {
      if !seen_sessions.iter().any(|s| s == &id) {
        tracing::debug!("session removed: {}", id);
        if let Some(session) = self.sessions.remove(&id) {
          session.cancel_token.cancel();
        }
      }
    }

    Ok(())
  }
}

pub struct WindowsPlayer {
  tx: StateTx,
  command_tx: Option<CommandTx>,
  cancel_token: CancellationToken,
  _session_manager_handle: Option<JoinHandle<()>>,
}

impl Player for WindowsPlayer {
  async fn init(tx: StateTx) -> Result<Self> {
    tracing::debug!("initializing windows media player subsystem");

    Ok(Self {
      tx,
      command_tx: None,
      cancel_token: CancellationToken::new(),
      _session_manager_handle: None,
    })
  }

  async fn subscribe(&mut self) -> Result<()> {
    tracing::debug!("subscribing to media player updates");
    self.handle_cancel_token_live();
    let (command_tx, command_rx) = tokio::sync::mpsc::channel(16);

    self.command_tx = Some(command_tx);
    self._session_manager_handle =
      Some(SessionManager::spawn(self.tx.clone(), command_rx, self.cancel_token.child_token()).await?);

    Ok(())
  }

  async fn unsubscribe(&mut self) -> Result<()> {
    tracing::debug!("unsubscribing from media player updates and killing threads");
    self.cancel_token.cancel();

    Ok(())
  }

  async fn send_command(&mut self, command: crate::Command) -> Result<()> {
    tracing::debug!("sending command to media player: {:?}", command);
    if let Some(command_tx) = &self.command_tx {
      command_tx.send(command).await?;
    } else {
      tracing::warn!("command channel not initialized - please subscribe first");
    }

    Ok(())
  }
}

impl WindowsPlayer {
  fn handle_cancel_token_live(&mut self) {
    if self.cancel_token.is_cancelled() {
      self.cancel_token = CancellationToken::new();
    }
  }
}
