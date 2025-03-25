#![allow(clippy::missing_safety_doc)]

use std::{ffi::c_void, mem::transmute, ptr::NonNull};

use block2::{RcBlock, StackBlock};
use core_foundation::{
  bundle::CFBundle, date::CFAbsoluteTimeGetCurrent, date::CFDateGetAbsoluteTime, string::CFString, url::CFURL,
};
use dispatch2::ffi::{dispatch_queue_global_s, dispatch_queue_global_t};
use objc2::{
  rc::{Id, Retained},
  runtime::NSObject,
};
use objc2_foundation::{NSData, NSDate, NSDictionary, NSNotification, NSNotificationCenter, NSNumber, NSString};
use tokio::{sync::mpsc, task::JoinHandle};
use tokio_util::sync::CancellationToken;

use crate::{Command, CommandData, CommandRx, CommandTx, NowPlaying, Player, Result, StateTx};

type NowPlayingCallback = RcBlock<dyn Fn(*const NSDictionary<NSString, NSObject>)>;

#[allow(improper_ctypes_definitions)]
type MRMediaRemoteSendCommandFunction =
  extern "C" fn(command: u32, userInfo: *const NSDictionary<NSString, NSObject>) -> bool;

#[allow(improper_ctypes_definitions)]
type MRMediaRemoteGetNowPlayingInfoFunction = extern "C" fn(dispatch_queue_global_t, NowPlayingCallback);
type MRMediaRemoteRegisterForNowPlayingNotificationsFunction = extern "C" fn(dispatch_queue_global_t);
type MRMediaRemoteUnregisterForNowPlayingNotificationsFunction = extern "C" fn();

#[allow(improper_ctypes_definitions)]
type GetNowPlayingInfoFnType = extern "C" fn(*mut dispatch_queue_global_s, NowPlayingCallback);
type RegisterNotificationsFnType = extern "C" fn(*mut dispatch_queue_global_s);
type UnregisterNotificationsFnType = extern "C" fn();

#[derive(Clone, Copy)]
struct QueueWrapper(*mut dispatch_queue_global_s);
unsafe impl Send for QueueWrapper {}
unsafe impl Sync for QueueWrapper {}

struct NotificationCenterWrapper {
  center: Retained<NSNotificationCenter>,
}

struct MacPlayerWorker {
  tx: StateTx,
  rx: CommandRx,

  queue: QueueWrapper,
  center_wrapper: NotificationCenterWrapper,

  get_now_playing_info_fn: MRMediaRemoteGetNowPlayingInfoFunction,
  register_notifications_fn: MRMediaRemoteRegisterForNowPlayingNotificationsFunction,
  unregister_notifications_fn: MRMediaRemoteUnregisterForNowPlayingNotificationsFunction,
  send_command_fn: MRMediaRemoteSendCommandFunction,

  cancel_token: CancellationToken,
}

impl NotificationCenterWrapper {
  fn new() -> Self {
    Self {
      center: unsafe { NSNotificationCenter::defaultCenter() },
    }
  }
}

#[allow(dead_code)]
enum MacCommand {
  Play = 0,
  Pause = 1,
  TogglePlayPause = 2,
  NextTrack = 4,
  PreviousTrack = 5,
  SetShuffle = 26,
  SetRepeatMode = 25,
}

#[allow(clippy::too_many_arguments)]
impl MacPlayerWorker {
  async fn spawn(
    tx: StateTx,
    rx: CommandRx,
    queue: QueueWrapper,
    get_now_playing_info_fn: MRMediaRemoteGetNowPlayingInfoFunction,
    register_notifications_fn: MRMediaRemoteRegisterForNowPlayingNotificationsFunction,
    unregister_notifications_fn: MRMediaRemoteUnregisterForNowPlayingNotificationsFunction,
    send_command_fn: MRMediaRemoteSendCommandFunction,
    cancel_token: CancellationToken,
  ) -> Result<JoinHandle<()>> {
    Ok(tokio::spawn(async move {
      Self {
        tx,
        rx,
        queue,
        center_wrapper: NotificationCenterWrapper::new(),
        get_now_playing_info_fn,
        register_notifications_fn,
        unregister_notifications_fn,
        send_command_fn,
        cancel_token,
      }
      .event_loop()
      .await;
    }))
  }

  async fn event_loop(&mut self) {
    tracing::debug!("Starting mac player event loop");
    self.register_notifications();
    self.get_now_playing().await;

    loop {
      tokio::select! {
        Some(command) = self.rx.recv() => self.handle_command(command),
        _ = self.cancel_token.cancelled() => {
          tracing::debug!("cancellation received, shutting down mac player worker");
          break;
        }
      }
    }

    self.unregister_notifications();
  }

  fn handle_command(&self, command: Command) {
    tracing::debug!("Received command: {:?}", command);

    match command.data {
      CommandData::Play => {
        tracing::debug!("sending play command");
        self.send_command(MacCommand::Play as u32)
      }
      CommandData::Pause => {
        tracing::debug!("sending pause command");
        self.send_command(MacCommand::Pause as u32)
      }
      CommandData::PlayPause => {
        tracing::debug!("sending play/pause command");
        self.send_command(MacCommand::TogglePlayPause as u32)
      }
      CommandData::NextTrack => {
        tracing::debug!("sending next track command");
        self.send_command(MacCommand::NextTrack as u32)
      }
      CommandData::PreviousTrack => {
        tracing::debug!("sending previous track command");
        self.send_command(MacCommand::PreviousTrack as u32)
      }
      CommandData::SeekTo(_) => {
        tracing::warn!("seek command not supported on macOS");
      }
      CommandData::SetVolume(_) => {
        tracing::warn!("set volume command not supported on macOS");
      }
      CommandData::SetShuffle(_) => {
        // this should totally be supported but i'm lazy
        tracing::warn!("set shuffle command not supported on macOS");
      }
    }
  }

  fn send_command(&self, command: u32) {
    // for most commands userInfo is nil
    (self.send_command_fn)(command, std::ptr::null());
  }

  fn register_notifications(&self) {
    (self.register_notifications_fn)(self.queue.0);
    tracing::debug!("registered for now playing notifications");

    let center = self.center_wrapper.center.clone();

    let queue = self.queue.0;
    let get_now_playing_info_fn = self.get_now_playing_info_fn;
    let tx = self.tx.clone();
    let observer = StackBlock::new(move |_notification: NonNull<NSNotification>| {
      tracing::debug!("new notification received!");
      let tx = tx.clone();
      let callback_block = create_callback_block(tx);

      (get_now_playing_info_fn)(queue, callback_block);
    })
    .copy();

    unsafe {
      center.addObserverForName_object_queue_usingBlock(
        Some(&*NSString::from_str(
          "kMRMediaRemoteNowPlayingInfoDidChangeNotification",
        )),
        None,
        None,
        &observer,
      );

      center.addObserverForName_object_queue_usingBlock(
        Some(&*NSString::from_str(
          "kMRMediaRemoteNowPlayingApplicationDidChangeNotification",
        )),
        None,
        None,
        &observer,
      );
    }
  }

  fn unregister_notifications(&self) {
    (self.unregister_notifications_fn)();
    tracing::debug!("unregistered for now playing notifications");

    unsafe {
      self
        .center_wrapper
        .center
        .removeObserver(&NSString::from_str("kMRMediaRemoteNowPlayingInfoDidChangeNotification"));

      self.center_wrapper.center.removeObserver(&NSString::from_str(
        "kMRMediaRemoteNowPlayingApplicationDidChangeNotification",
      ));
    }
  }

  async fn get_now_playing(&self) {
    tracing::debug!("fetching now playing song info");
    let tx = self.tx.clone();
    let callback_block = create_callback_block(tx);

    (self.get_now_playing_info_fn)(self.queue.0, callback_block);
  }
}

pub struct MacPlayer {
  state_tx: StateTx,
  command_tx: Option<CommandTx>,

  queue: QueueWrapper,

  get_now_playing_info_fn: MRMediaRemoteGetNowPlayingInfoFunction,
  register_notifications_fn: MRMediaRemoteRegisterForNowPlayingNotificationsFunction,
  unregister_notifications_fn: MRMediaRemoteUnregisterForNowPlayingNotificationsFunction,
  send_command_fn: MRMediaRemoteSendCommandFunction,

  cancel_token: CancellationToken,
  _worker_handle: Option<JoinHandle<()>>,
}

impl MacPlayer {
  pub async fn init(tx: StateTx) -> Result<Self> {
    unsafe {
      tracing::debug!("initializing media remote framework");
      let bundle_url = CFURL::from_path("/System/Library/PrivateFrameworks/MediaRemote.framework", true).unwrap();
      let bundle = CFBundle::new(bundle_url).unwrap();

      Ok(MacPlayer {
        state_tx: tx,
        command_tx: None,

        queue: QueueWrapper(dispatch2::ffi::dispatch_get_global_queue(0, 0)),

        get_now_playing_info_fn: transmute::<*const c_void, GetNowPlayingInfoFnType>(
          bundle.function_pointer_for_name(CFString::from_static_string("MRMediaRemoteGetNowPlayingInfo")),
        ),
        register_notifications_fn: transmute::<*const c_void, RegisterNotificationsFnType>(
          bundle.function_pointer_for_name(CFString::from_static_string(
            "MRMediaRemoteRegisterForNowPlayingNotifications",
          )),
        ),
        unregister_notifications_fn: transmute::<*const c_void, UnregisterNotificationsFnType>(
          bundle.function_pointer_for_name(CFString::from_static_string(
            "MRMediaRemoteUnregisterForNowPlayingNotifications",
          )),
        ),
        send_command_fn: transmute::<*const c_void, MRMediaRemoteSendCommandFunction>(
          bundle.function_pointer_for_name(CFString::from_static_string("MRMediaRemoteSendCommand")),
        ),

        cancel_token: CancellationToken::new(),
        _worker_handle: None,
      })
    }
  }
}

impl Player for MacPlayer {
  async fn init(tx: StateTx) -> Result<Self> {
    Self::init(tx).await
  }

  async fn subscribe(&mut self) -> Result<()> {
    tracing::debug!("subscribing to media player updates");

    if self.cancel_token.is_cancelled() {
      self.cancel_token = CancellationToken::new();
    }

    let (command_tx, command_rx) = mpsc::channel(16);
    self.command_tx = Some(command_tx);

    self._worker_handle = Some(
      MacPlayerWorker::spawn(
        self.state_tx.clone(),
        command_rx,
        self.queue,
        self.get_now_playing_info_fn,
        self.register_notifications_fn,
        self.unregister_notifications_fn,
        self.send_command_fn,
        self.cancel_token.child_token(),
      )
      .await?,
    );

    Ok(())
  }

  async fn unsubscribe(&mut self) -> Result<()> {
    tracing::debug!("unsubscribing from media player updates");
    self.cancel_token.cancel();
    self.command_tx = None;

    Ok(())
  }

  async fn send_command(&mut self, command: Command) -> Result<()> {
    if let Some(tx) = &self.command_tx {
      tx.send(command).await?;
    } else {
      tracing::warn!("command channel not initialized - please subscribe first");
    }

    Ok(())
  }
}

#[allow(dead_code)]
pub trait NSDictionaryExtensions {
  unsafe fn get_string_for_key(&self, key: &str) -> Option<String>;
  unsafe fn get_f64_for_key(&self, key: &str) -> Option<f64>;
  unsafe fn get_bool_for_key(&self, key: &str) -> Option<bool>;
  unsafe fn get_absolute_date_time_for_key(&self, key: &str) -> Option<f64>;
  unsafe fn get_data_for_key(&self, key: &str) -> Option<Vec<u8>>;
  unsafe fn get_u32_for_key(&self, key: &str) -> Option<u32>;
}

impl NSDictionaryExtensions for NSDictionary<NSString, NSObject> {
  unsafe fn get_string_for_key(&self, key: &str) -> Option<String> {
    unsafe {
      self
        .objectForKey(&*NSString::from_str(key))
        .map(|value| Id::cast::<NSString>(value.to_owned()).to_string())
    }
  }

  unsafe fn get_f64_for_key(&self, key: &str) -> Option<f64> {
    unsafe {
      self
        .objectForKey(&*NSString::from_str(key))
        .map(|value| Id::cast::<NSNumber>(value.to_owned()).as_f64())
    }
  }

  unsafe fn get_bool_for_key(&self, key: &str) -> Option<bool> {
    unsafe {
      self
        .objectForKey(&*NSString::from_str(key))
        .map(|value| Id::cast::<NSNumber>(value.to_owned()).as_bool())
    }
  }

  unsafe fn get_absolute_date_time_for_key(&self, key: &str) -> Option<f64> {
    unsafe {
      self.objectForKey(&*NSString::from_str(key)).map(|value| {
        CFDateGetAbsoluteTime(transmute::<Retained<NSDate>, *const core_foundation::date::__CFDate>(
          Id::cast::<NSDate>(value.to_owned()),
        ))
      })
    }
  }

  unsafe fn get_data_for_key(&self, key: &str) -> Option<Vec<u8>> {
    unsafe {
      self.objectForKey(&*NSString::from_str(key)).map(|value| {
        let data: &NSData = &Id::cast(value.to_owned());
        data.bytes().to_vec()
      })
    }
  }

  unsafe fn get_u32_for_key(&self, key: &str) -> Option<u32> {
    unsafe {
      self
        .objectForKey(&*NSString::from_str(key))
        .map(|value| Id::cast::<NSNumber>(value.to_owned()).as_u32())
    }
  }
}

fn create_callback_block(tx: StateTx) -> NowPlayingCallback {
  block2::StackBlock::new(move |raw_info_dictionary: *const NSDictionary<NSString, NSObject>| {
    tracing::debug!("new now playing notification received");
    let info_dictionary = match unsafe { raw_info_dictionary.as_ref() } {
      Some(x) => x,
      None => {
        tracing::debug!("no info dictionary found");
        return;
      }
    };
    tracing::trace!("now playing: {:?}", info_dictionary);

    unsafe {
      let song_name = info_dictionary.get_string_for_key("kMRMediaRemoteNowPlayingInfoTitle");
      let artist_name = info_dictionary.get_string_for_key("kMRMediaRemoteNowPlayingInfoArtist");
      let album_name = info_dictionary.get_string_for_key("kMRMediaRemoteNowPlayingInfoAlbum");
      let duration = info_dictionary.get_f64_for_key("kMRMediaRemoteNowPlayingInfoDuration");
      let artwork_mime_type = info_dictionary.get_string_for_key("kMRMediaRemoteNowPlayingInfoArtworkMIMEType");
      let artwork_data = info_dictionary.get_data_for_key("kMRMediaRemoteNowPlayingInfoArtworkData");
      let shuffle_mode = info_dictionary.get_u32_for_key("kMRMediaRemoteNowPlayingInfoShuffleMode");
      let repeat_mode = info_dictionary.get_u32_for_key("kMRMediaRemoteNowPlayingInfoRepeatMode");
      let app_name = info_dictionary.get_string_for_key("kMRMediaRemoteNowPlayingApplicationDisplayNameUserInfoKey");

      let playback_rate = info_dictionary
        .get_f64_for_key("kMRMediaRemoteNowPlayingInfoPlaybackRate")
        .unwrap_or_default();
      let is_playing = playback_rate > 0f64;

      let song_progress_until_previous_pause: f64 = info_dictionary
        .get_f64_for_key("kMRMediaRemoteNowPlayingInfoElapsedTime")
        .unwrap_or_default();
      let song_progress = match is_playing {
        true => {
          let last_play_event_timestamp: f64 = info_dictionary
            .get_absolute_date_time_for_key("kMRMediaRemoteNowPlayingInfoTimestamp")
            .unwrap_or_default();
          let time_interval_since_last_play_event: f64 = CFAbsoluteTimeGetCurrent() - last_play_event_timestamp;

          (time_interval_since_last_play_event + song_progress_until_previous_pause) * 1000.0
        }
        false => song_progress_until_previous_pause * 1000.0,
      } as u32;

      // Create base64 thumbnail from artwork data if available
      let thumbnail = if let Some(artwork_data) = artwork_data.as_ref() {
        use base64::prelude::*;

        artwork_mime_type
          .as_ref()
          .map(|mime_type| format!("data:{};base64,{}", mime_type, BASE64_STANDARD.encode(artwork_data)))
      } else {
        None
      };

      let repeat_state = repeat_mode.map(|mode| match mode {
        0 => "off".to_string(),
        1 => "track".to_string(),
        2 => "all".to_string(),
        _ => "off".to_string(),
      });

      let now_playing = NowPlaying {
        track_name: song_name.unwrap_or_default(),
        album: album_name,
        artist: artist_name.map(|name| vec![name]),
        is_playing,
        track_duration: duration.map(|d| (d * 1000.0) as u32),
        track_progress: Some(song_progress),
        thumbnail,
        id: None,
        can_skip: true,
        can_fast_forward: false,
        can_change_volume: false,
        can_like: false,
        can_set_output: false,
        device_id: app_name,
        playback_rate: Some(playback_rate),
        volume: 0, // Not provided by MediaRemote, default to 0
        shuffle_state: shuffle_mode.map(|mode| mode != 0),
        repeat_state,
        ..Default::default()
      };

      let _ = tx.try_send(now_playing);
    }
  })
  .copy()
}
