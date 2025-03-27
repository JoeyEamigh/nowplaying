import { NowPlaying, type LogMessage, type NowPlayingMessage } from '../';

const logCallback = (event: LogMessage) => {
  console.log('[NowPlaying] new log:', event);
};

const callback = (event: NowPlayingMessage) => {
  console.log('[NowPlaying]', event);
};

const sleep = (seconds: number) => new Promise(resolve => setTimeout(resolve, seconds * 1000));

try {
  const player = new NowPlaying(callback, { logLevelDirective: 'n_nowplaying=trace,nowplaying=trace', logCallback });
  await player.subscribe();

  // sleep for 5 seconds
  await sleep(5);
  console.log('pausing...');
  await player.pause();

  // sleep for 5 seconds
  await sleep(5);
  console.log('resuming...');
  await player.play();

  // sleep for 5 seconds
  await sleep(5);
  console.log('rewinding...');
  await player.previousTrack();

  // sleep forever for the example
  await new Promise(resolve => setTimeout(resolve, 999_999_999));
} catch (error) {
  console.error('Error:', error);
}
