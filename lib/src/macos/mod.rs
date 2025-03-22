#![allow(clippy::missing_safety_doc)]

use std::ffi::c_void;
use std::mem::transmute;
use std::ptr::NonNull;

use block2::{RcBlock, StackBlock};
use core_foundation::date::CFAbsoluteTimeGetCurrent;
use core_foundation::string::CFString;
use core_foundation::url::CFURL;
use core_foundation::{bundle::CFBundle, date::CFDateGetAbsoluteTime};
use derive_more::Debug;
use dispatch2::ffi::{dispatch_queue_global_s, dispatch_queue_global_t};
use objc2::rc::{Id, Retained};
use objc2::runtime::NSObject;
use objc2_foundation::{NSData, NSDate, NSDictionary, NSNotification, NSNotificationCenter, NSNumber, NSString};
use tokio::sync::mpsc;

type NowPlayingCallback = RcBlock<dyn Fn(*const NSDictionary<NSString, NSObject>)>;

#[allow(improper_ctypes_definitions)]
type MRMediaRemoteGetNowPlayingInfoFunction = extern "C" fn(dispatch_queue_global_t, NowPlayingCallback);

type MRMediaRemoteRegisterForNowPlayingNotificationsFunction = extern "C" fn(dispatch_queue_global_t);
type MRMediaRemoteUnregisterForNowPlayingNotificationsFunction = extern "C" fn();

#[allow(improper_ctypes_definitions)]
type GetNowPlayingInfoFnType = extern "C" fn(*mut dispatch_queue_global_s, NowPlayingCallback);
type RegisterNotificationsFnType = extern "C" fn(*mut dispatch_queue_global_s);
type UnregisterNotificationsFnType = extern "C" fn();

pub struct MacPlayer {
  tx: mpsc::Sender<NowPlayingInfo>,

  queue: *mut dispatch_queue_global_s,
  center: Retained<NSNotificationCenter>,

  get_now_playing_info_fn: MRMediaRemoteGetNowPlayingInfoFunction,
  register_notifications_fn: MRMediaRemoteRegisterForNowPlayingNotificationsFunction,
  unregister_notifications_fn: MRMediaRemoteUnregisterForNowPlayingNotificationsFunction,
}

impl MacPlayer {
  pub fn init(tx: mpsc::Sender<NowPlayingInfo>) -> Self {
    unsafe {
      tracing::debug!("initializing media remote framework");
      let bundle_url = CFURL::from_path("/System/Library/PrivateFrameworks/MediaRemote.framework", true).unwrap();
      let bundle = CFBundle::new(bundle_url).unwrap();

      MacPlayer {
        tx,

        queue: dispatch2::ffi::dispatch_get_global_queue(0, 0),
        center: NSNotificationCenter::defaultCenter(),

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
      }
    }
  }

  pub async fn now_playing(&self) {
    tracing::debug!("fetching now playing song info");
    unsafe {
      self.unsafe_get_now_playing().await;
    }
  }

  async unsafe fn unsafe_get_now_playing(&self) {
    let tx = self.tx.clone();
    let callback_block = create_callback_block(tx);

    let _ = &(self.get_now_playing_info_fn)(self.queue, callback_block);
  }

  pub fn register_notifications(&self) {
    (self.register_notifications_fn)(self.queue);
    tracing::debug!("registered for now playing notifications");

    let center = self.center.clone();

    let queue = self.queue;
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

  pub fn unregister_notifications(&self) {
    (self.unregister_notifications_fn)();
    tracing::debug!("unregistered for now playing notifications");

    // Remove observer for notifications
    unsafe {
      self
        .center
        .removeObserver(&NSString::from_str("MRMediaRemoteNowPlayingInfoDidChangeNotification"));
    }
  }
}

#[derive(Debug)]
pub struct NowPlayingInfo {
  pub song_name: Option<String>,
  pub artist_name: Option<String>,
  pub album_name: Option<String>,
  pub duration: Option<f64>,
  pub artwork_mime_type: Option<String>,
  pub artwork_identifier: Option<String>,
  #[debug(skip)]
  pub artwork_data: Option<Vec<u8>>,
  pub artwork_data_height: Option<u32>,
  pub artwork_data_width: Option<u32>,
  pub content_item_identifier: Option<String>,
  pub is_playing: bool,
  pub song_progress: u32,
}

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
    self
      .objectForKey(&*NSString::from_str(key))
      .map(|value| Id::cast::<NSString>(value.to_owned()).to_string())
  }

  unsafe fn get_f64_for_key(&self, key: &str) -> Option<f64> {
    self
      .objectForKey(&*NSString::from_str(key))
      .map(|value| Id::cast::<NSNumber>(value.to_owned()).as_f64())
  }

  unsafe fn get_bool_for_key(&self, key: &str) -> Option<bool> {
    self
      .objectForKey(&*NSString::from_str(key))
      .map(|value| Id::cast::<NSNumber>(value.to_owned()).as_bool())
  }

  unsafe fn get_absolute_date_time_for_key(&self, key: &str) -> Option<f64> {
    self.objectForKey(&*NSString::from_str(key)).map(|value| {
      CFDateGetAbsoluteTime(transmute::<Retained<NSDate>, *const core_foundation::date::__CFDate>(
        Id::cast::<NSDate>(value.to_owned()),
      ))
    })
  }

  unsafe fn get_data_for_key(&self, key: &str) -> Option<Vec<u8>> {
    self.objectForKey(&*NSString::from_str(key)).map(|value| {
      let data: &NSData = &Id::cast(value.to_owned());
      data.bytes().to_vec()
    })
  }

  unsafe fn get_u32_for_key(&self, key: &str) -> Option<u32> {
    self
      .objectForKey(&*NSString::from_str(key))
      .map(|value| Id::cast::<NSNumber>(value.to_owned()).as_u32())
  }
}

#[allow(clippy::type_complexity)]
fn create_callback_block(tx: mpsc::Sender<NowPlayingInfo>) -> NowPlayingCallback {
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
      let artwork_identifier = info_dictionary.get_string_for_key("kMRMediaRemoteNowPlayingInfoArtworkIdentifier");
      let artwork_data = info_dictionary.get_data_for_key("kMRMediaRemoteNowPlayingInfoArtworkData");
      let artwork_data_height = info_dictionary.get_u32_for_key("kMRMediaRemoteNowPlayingInfoArtworkDataHeight");
      let artwork_data_width = info_dictionary.get_u32_for_key("kMRMediaRemoteNowPlayingInfoArtworkDataWidth");
      let content_item_identifier =
        info_dictionary.get_string_for_key("kMRMediaRemoteNowPlayingInfoContentItemIdentifier");

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

      tx.blocking_send(NowPlayingInfo {
        song_name,
        artist_name,
        album_name,
        duration,
        artwork_mime_type,
        artwork_identifier,
        artwork_data,
        artwork_data_height,
        artwork_data_width,
        content_item_identifier,
        is_playing,
        song_progress,
      })
      .unwrap_or_default();
    }
  })
  .copy()
}
