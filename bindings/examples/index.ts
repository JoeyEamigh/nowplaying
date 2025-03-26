import { NowPlaying, type LogMessage, type NowPlayingMessage } from '../';

const logCallback = (event: LogMessage) => {
  console.log('Flash event:', event);
};

const callback = (event: NowPlayingMessage) => {
  console.log('Flash event:', event);
};

try {
  const player = new NowPlaying(callback, { logLevelDirective: 'nowplaying=trace', logCallback });
  await player.subscribe();

  // sleep forever for the example
  await new Promise(resolve => setTimeout(resolve, 999_999_999));
} catch (error) {
  console.error('Error:', error);
}
