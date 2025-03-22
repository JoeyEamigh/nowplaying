mod media_player2_player;

use std::collections::HashMap;

use futures::StreamExt;
use media_player2_player::*;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use zvariant::OwnedValue;

use crate::{Command, CommandRx, CommandTx, NowPlaying, Player, Result, StateTx};

use zbus::{
  Connection,
  fdo::{
    DBusProxy, NameOwnerChanged, NameOwnerChangedStream, PropertiesChanged, PropertiesChangedStream, PropertiesProxy,
  },
  names::BusName,
};

// mod media_player2;
// use media_player2::*;

struct PlayerConnection {
  name: BusName<'static>,

  tx: StateTx,
  rx: CommandRx,
  state: NowPlaying,

  player: MediaPlayer2PlayerProxy<'static>,
  props_stream: PropertiesChangedStream,

  cancel_token: CancellationToken,
}

impl PlayerConnection {
  pub async fn spawn(
    connection: &Connection,
    name: BusName<'static>,
    tx: StateTx,
    rx: CommandRx,
    cancel_token: CancellationToken,
  ) -> Result<JoinHandle<()>> {
    tracing::debug!("({}) spawning new player connection for player", name.as_str());
    let player = MediaPlayer2PlayerProxy::builder(connection)
      .destination(&name)?
      .build()
      .await?;
    let player_props = PropertiesProxy::builder(connection)
      .destination(&name)?
      .path("/org/mpris/MediaPlayer2")?
      .build()
      .await?;
    let props_stream = player_props.receive_properties_changed().await?;

    Ok(tokio::spawn(async move {
      Self {
        name,

        tx,
        rx,
        state: NowPlaying::default(),

        player,
        props_stream,

        cancel_token,
      }
      .event_loop()
      .await
    }))
  }

  async fn event_loop(&mut self) {
    self.full_fetch().await;
    let _ = self.tx.send(self.state.clone()).await;
    tracing::debug!("({}) player connection initialized", self.name.as_str());

    loop {
      tokio::select! {
        Some(change) = self.props_stream.next() => {
          if let Err(err) = self.handle_change(change).await {
            tracing::error!("({}) error handling change: {:?}", self.name.as_str(), err);
          }
        },
        Some(command) = self.rx.recv() => {
          if let Err(err) = self.handle_command(command).await {
            tracing::error!("({}) error handling command: {:?}", self.name.as_str(), err);
          }
        },
        _ = self.cancel_token.cancelled() => break,
      }
    }

    tracing::debug!(
      "({}) cancellation received, shutting down player connection thread",
      self.name.as_str()
    );
  }

  async fn handle_command(&mut self, command: Command) -> Result<()> {
    tracing::debug!("({}) received command: {:?}", self.name.as_str(), command);

    match command.data {
      crate::CommandData::Play => self.player.play().await?,
      crate::CommandData::Pause => self.player.pause().await?,
      crate::CommandData::PlayPause => self.player.play_pause().await?,
      crate::CommandData::NextTrack => self.player.next().await?,
      crate::CommandData::PreviousTrack => self.player.previous().await?,
      crate::CommandData::SeekTo(position) => self.player.seek(position as i64).await?,
      crate::CommandData::SetVolume(volume) => self.player.set_volume(volume as f64 / 100.0).await?,
      crate::CommandData::SetShuffle(shuffle) => self.player.set_shuffle(shuffle).await?,
    }

    Ok(())
  }

  async fn handle_change(&mut self, change: PropertiesChanged) -> Result<()> {
    let Ok(args) = change.args() else {
      tracing::warn!("({}) change args missing?", self.name.as_str());
      return Ok(());
    };

    tracing::trace!("({}) new change on player: {:?}", self.name.as_str(), args);
    for (key, value) in args.changed_properties {
      match key {
        "PlaybackStatus" => {
          self.handle_playback_status(value.downcast::<PlaybackStatus>().map_err(zbus::Error::Variant))
        }
        "LoopStatus" => self.handle_loop_status(value.downcast::<LoopStatus>().map_err(zbus::Error::Variant)),
        "Rate" => self.handle_playback_rate(value.downcast::<f64>().map_err(zbus::Error::Variant)),
        "Shuffle" => self.handle_shuffle_state(value.downcast::<bool>().map_err(zbus::Error::Variant)),
        "Metadata" => {
          self.handle_metadata(
            value
              .downcast::<HashMap<String, OwnedValue>>()
              .map_err(zbus::Error::Variant),
          );
        }
        "Volume" => self.handle_volume(value.downcast::<f64>().map_err(zbus::Error::Variant)),
        "Position" => self.handle_playback_position(value.downcast::<i64>().map_err(zbus::Error::Variant)),
        "CanGoNext" => self.handle_can_go_next(value.downcast::<bool>().map_err(zbus::Error::Variant)),
        "CanSeek" => self.handle_can_seek(value.downcast::<bool>().map_err(zbus::Error::Variant)),
        _ => tracing::trace!("({}) unhandled property change: {}", self.name.as_str(), key),
      }
    }

    Ok(self.tx.send(self.state.clone()).await?)
  }

  async fn full_fetch(&mut self) {
    tracing::debug!("({}) fetching player state", self.name.as_str());

    self.handle_playback_status(self.player.playback_status().await);
    self.handle_loop_status(self.player.loop_status().await);
    self.handle_playback_rate(self.player.rate().await);
    self.handle_shuffle_state(self.player.shuffle().await);
    self.handle_metadata(self.player.metadata().await);
    self.handle_volume(self.player.volume().await);
    self.handle_playback_position(self.player.position().await);
    self.handle_can_go_next(self.player.can_go_next().await);
    self.handle_can_seek(self.player.can_seek().await);
  }

  fn handle_playback_status(&mut self, playback_status: zbus::Result<PlaybackStatus>) {
    let playback_status = match playback_status {
      Ok(status) => status,
      Err(err) => return tracing::error!("({}) error handling playback status: {:?}", self.name.as_str(), err),
    };

    tracing::trace!(
      "({}) playback status changed: {:?}",
      self.name.as_str(),
      playback_status
    );
    self.state.is_playing = matches!(playback_status, PlaybackStatus::Playing);
  }

  fn handle_loop_status(&mut self, loop_status: zbus::Result<LoopStatus>) {
    let loop_status = match loop_status {
      Ok(status) => status,
      Err(err) => return tracing::debug!("({}) error handling loop status: {:?}", self.name.as_str(), err),
    };

    tracing::trace!("({}) loop status changed: {:?}", self.name.as_str(), loop_status);
    self.state.repeat_state = match loop_status {
      LoopStatus::None => Some("off".to_string()),
      LoopStatus::Track => Some("track".to_string()),
      LoopStatus::Playlist => Some("all".to_string()),
    };
  }

  fn handle_playback_rate(&mut self, playback_rate: zbus::Result<f64>) {
    let playback_rate = match playback_rate {
      Ok(rate) => rate,
      Err(err) => return tracing::debug!("({}) error handling playback rate: {:?}", self.name.as_str(), err),
    };

    tracing::trace!("({}) playback rate changed: {:?}", self.name.as_str(), playback_rate);
    self.state.playback_rate = Some(playback_rate);
  }

  fn handle_shuffle_state(&mut self, shuffle_state: zbus::Result<bool>) {
    let shuffle_state = match shuffle_state {
      Ok(state) => state,
      Err(err) => return tracing::debug!("({}) error handling shuffle state: {:?}", self.name.as_str(), err),
    };

    tracing::trace!("({}) shuffle status changed: {:?}", self.name.as_str(), shuffle_state);
    self.state.shuffle_state = Some(shuffle_state);
  }

  fn handle_metadata(&mut self, metadata: zbus::Result<HashMap<String, OwnedValue>>) {
    let metadata: Metadata = match metadata {
      Ok(metadata) => metadata.into(),
      Err(err) => return tracing::error!("({}) error handling metadata: {:?}", self.name.as_str(), err),
    };

    tracing::trace!("({}) metadata changed: {:?}", self.name.as_str(), &metadata);
    self.state.id = Some(metadata.track_id);
    self.state.device_id = Some(self.name.as_str().to_string());
    self.state.track_name = metadata.title.unwrap_or_default();
    self.state.track_duration = metadata.length.map(|l| l as u32);
    self.state.album = metadata.album;
    self.state.artist = metadata.artist;
    self.state.thumbnail = metadata.art_url;
    self.state.url = metadata.url;
  }

  fn handle_volume(&mut self, volume: zbus::Result<f64>) {
    let volume = match volume {
      Ok(volume) => volume,
      Err(err) => return tracing::debug!("({}) error handling volume: {:?}", self.name.as_str(), err),
    };

    tracing::trace!("({}) volume changed: {:?}", self.name.as_str(), volume);
    self.state.volume = (volume * 100.0) as u8;
  }

  fn handle_playback_position(&mut self, position: zbus::Result<i64>) {
    let position = match position {
      Ok(position) => position,
      Err(err) => return tracing::debug!("({}) error handling playback position: {:?}", self.name.as_str(), err),
    };

    tracing::trace!("({}) playback position changed: {:?}", self.name.as_str(), position);
    self.state.track_progress = Some(position as u32);
  }

  fn handle_can_go_next(&mut self, can_go_next: zbus::Result<bool>) {
    let can_go_next = match can_go_next {
      Ok(can_go_next) => can_go_next,
      Err(err) => return tracing::debug!("({}) error handling can go next: {:?}", self.name.as_str(), err),
    };

    tracing::trace!("({}) can go next changed: {:?}", self.name.as_str(), can_go_next);
    self.state.can_skip = can_go_next;
  }

  fn handle_can_seek(&mut self, can_seek: zbus::Result<bool>) {
    let can_seek = match can_seek {
      Ok(can_seek) => can_seek,
      Err(err) => return tracing::debug!("({}) error handling can seek: {:?}", self.name.as_str(), err),
    };

    tracing::trace!("({}) can seek changed: {:?}", self.name.as_str(), can_seek);
    self.state.can_fast_forward = can_seek;
  }
}

struct PlayerConnectionHandle {
  cancel_token: CancellationToken,
  tx: CommandTx,
  _handle: JoinHandle<()>,
}

struct PlayerManager {
  connection: Connection,
  changes: NameOwnerChangedStream,
  players: HashMap<String, PlayerConnectionHandle>,

  tx: StateTx,
  rx: CommandRx,
  cancel_token: CancellationToken,
}

impl PlayerManager {
  pub async fn spawn(
    connection: Connection,
    tx: StateTx,
    rx: CommandRx,
    cancel_token: CancellationToken,
  ) -> Result<JoinHandle<()>> {
    tracing::debug!("creating new media player manager");
    let proxy = DBusProxy::new(&connection).await?;
    let changes = proxy.receive_name_owner_changed().await?;

    // connect to existing players
    tracing::debug!("attempting to connect to existing players");
    let names = proxy.list_names().await?;
    let player_names = names
      .into_iter()
      .filter(|name| name.starts_with("org.mpris.MediaPlayer2."))
      .collect::<Vec<_>>();

    let players = player_names
      .into_iter()
      .map(|name| {
        let name_str = name.to_string();
        let tx = tx.clone();
        let cancel_token = cancel_token.child_token();
        let connection = &connection;
        let (command_tx, command_rx) = tokio::sync::mpsc::channel(16);

        async move {
          Ok::<_, crate::Error>((
            name_str,
            PlayerConnectionHandle {
              _handle: PlayerConnection::spawn(connection, name.into(), tx, command_rx, cancel_token.clone()).await?,
              tx: command_tx,
              cancel_token,
            },
          ))
        }
      })
      .collect::<Vec<_>>();

    let initial_players = futures::future::try_join_all(players).await?;
    if initial_players.is_empty() {
      tracing::debug!("no existing players found");
    }

    Ok(tokio::spawn(async move {
      Self {
        connection,
        changes,
        players: HashMap::from_iter(initial_players),

        tx,
        rx,
        cancel_token,
      }
      .event_loop()
      .await
    }))
  }

  async fn event_loop(&mut self) {
    loop {
      tokio::select! {
        Some(change) = self.changes.next() => if let Err(err) = self.handle_change(change).await {
          tracing::error!("error handling change: {:?}", err);
        },
        Some(command) = self.rx.recv() => {
          tracing::trace!("received command: {:?}", command);
          if let Some(to) = &command.to {
            if let Some(player) = self.players.get_mut(to) {
              if let Err(err) = player.tx.send(command.clone()).await {
                tracing::error!("error sending command to player: {:?}", err);
              }
            } else {
              tracing::warn!("no player found for command: {:?}", command);
            }
          } else {
            for player in self.players.values_mut() {
              if let Err(err) = player.tx.send(command.clone()).await {
                tracing::error!("error sending command to player: {:?}", err);
              }
            }
          }
        },
        _ = self.cancel_token.cancelled() => break,
      }
    }

    tracing::debug!("cancellation received, shutting down player manager thread");
  }

  async fn handle_change(&mut self, change: NameOwnerChanged) -> Result<()> {
    let Ok(args) = change.args() else {
      return Ok(());
    };

    tracing::trace!("new NameOwnerChanged signal with args: {:?}", args);
    if !args.name.starts_with("org.mpris.MediaPlayer2.") || (args.old_owner.is_some() && args.new_owner.is_some()) {
      return Ok(());
    }

    // the player was destroyed - shut down threads
    if args.new_owner.is_none() {
      tracing::debug!("player destroyed: {:?}", args.name);

      if let Some(existing) = self.players.remove(args.name.as_str()) {
        tracing::debug!("cancelling existing player thread at {:?}", args.name);
        existing.cancel_token.cancel();
      }

      return Ok(());
    }

    tracing::debug!("new media player found: {:?}", args.name);
    let cancel_token = self.cancel_token.child_token();
    let (command_tx, command_rx) = tokio::sync::mpsc::channel(16);
    let existing = self.players.insert(
      args.name.to_string(),
      PlayerConnectionHandle {
        _handle: PlayerConnection::spawn(
          &self.connection,
          args.name.to_owned(),
          self.tx.clone(),
          command_rx,
          cancel_token.clone(),
        )
        .await?,
        tx: command_tx,
        cancel_token,
      },
    );

    if let Some(existing) = existing {
      tracing::debug!("cancelling existing player thread at {:?}", args.name);
      existing.cancel_token.cancel();
    }

    Ok(())
  }
}

pub struct LinuxPlayer {
  connection: Connection,
  tx: StateTx,
  command_tx: Option<CommandTx>,

  cancel_token: CancellationToken,
  _player_manager_handle: Option<JoinHandle<()>>,
}

impl Player for LinuxPlayer {
  async fn init(tx: StateTx) -> Result<Self> {
    tracing::debug!("initializing linux media player subsystem");
    let connection = Connection::session().await?;

    Ok(Self {
      connection,
      tx,
      command_tx: None,

      cancel_token: CancellationToken::new(),
      _player_manager_handle: None,
    })
  }

  async fn subscribe(&mut self) -> Result<()> {
    tracing::debug!("subscribing to media player updates");
    self.handle_cancel_token_live();
    let (command_tx, command_rx) = tokio::sync::mpsc::channel(16);

    self.command_tx = Some(command_tx.clone());
    self._player_manager_handle = Some(
      PlayerManager::spawn(
        self.connection.clone(),
        self.tx.clone(),
        command_rx,
        self.cancel_token.child_token(),
      )
      .await?,
    );

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

impl LinuxPlayer {
  fn handle_cancel_token_live(&mut self) {
    if self.cancel_token.is_cancelled() {
      self.cancel_token = CancellationToken::new();
    }
  }
}
