# nowplaying

A cross-platform Node.js N-API module to get and control the currently playing media across different operating systems.

## Features

- Get currently playing media information (song, album, artist, progress, etc.)
- Control media playback (play, pause, next, previous, etc.)
- Cross-platform support (macOS, Windows, Linux)
- Real-time updates via event callbacks
- Detailed logging capabilities

## Installation

```bash
npm install node-nowplaying
# or
yarn add node-nowplaying
# or
bun add node-nowplaying
```

## Usage

### Basic Example

```typescript
import { NowPlaying } from 'node-nowplaying';

// create a new instance with a callback for media updates
const player = new NowPlaying(event => {
  console.log('[NowPlaying]:', event);
});

// initialize and subscribe to media events
await player.subscribe();

// control playback
await player.play();
await player.pause();
await player.nextTrack();
await player.previousTrack();

// clean up when done
await player.unsubscribe();
```

### With Logging

```typescript
import { NowPlaying, type LogMessage } from 'node-nowplaying';

// custom log handler
const logCallback = (log: LogMessage) => {
  console.log(
    `[${log.timestamp}] [${log.level}] ${log.target}: ${log.message}`,
  );
};

// create with logging options
const player = new NowPlaying(event => console.log('[NowPlaying]:', event), {
  logCallback,
  logLevelDirective: 'nowplaying=debug,n_nowplaying=debug',
});

await player.subscribe();
```

## API Reference

### `NowPlaying` Class

#### Constructor

```typescript
new NowPlaying(
  callback: (event: NowPlayingMessage) => void,
  options?: NowPlayingOptions
)
```

- `callback`: Function called when media state changes
- `options`: Optional configuration object

#### Methods

| Method                                      | Description                                                          |
| ------------------------------------------- | -------------------------------------------------------------------- |
| `subscribe()`                               | Start listening for media events                                     |
| `unsubscribe()`                             | Stop listening for media events                                      |
| `play(to?: string)`                         | Begin playback (optionally on a specific device)                     |
| `pause(to?: string)`                        | Pause playback (optionally on a specific device)                     |
| `playPause(to?: string)`                    | Toggle play/pause (optionally on a specific device)                  |
| `nextTrack(to?: string)`                    | Skip to the next track (optionally on a specific device)             |
| `previousTrack(to?: string)`                | Go to the previous track (optionally on a specific device)           |
| `seekTo(positionMs: number, to?: string)`   | Seek to a position in milliseconds (optionally on a specific device) |
| `setVolume(volume: number, to?: string)`    | Set volume level 0-100 (optionally on a specific device)             |
| `setShuffle(shuffle: boolean, to?: string)` | Set shuffle mode (optionally on a specific device)                   |
| `sendCommand(command: PlayerCommand)`       | Send supported command to the media player                           |

### Types

#### `NowPlayingMessage`

```typescript
interface NowPlayingMessage {
  album?: string;
  artist?: Array<string>;
  playlist?: string;
  playlistId?: string;
  trackName: string;
  shuffleState?: boolean;
  repeatState?: string; // "off", "all", "track"
  isPlaying: boolean;
  canFastForward: boolean;
  canSkip: boolean;
  canLike: boolean;
  canChangeVolume: boolean;
  canSetOutput: boolean;
  trackDuration?: number;
  trackProgress?: number;
  playbackRate?: number;
  volume: number;
  device?: string;
  id?: string;
  deviceId?: string;
  url?: string;
  thumbnail?: string;
}
```

#### `NowPlayingOptions`

```typescript
interface NowPlayingOptions {
  logCallback?: (event: LogMessage) => void;
  logLevelDirective?: string;
}
```

#### `LogMessage`

```typescript
interface LogMessage {
  level: string; // "TRACE", "DEBUG", "INFO", "WARN", "ERROR"
  target: string; // Component that generated the log
  message: string; // Log content
  timestamp: string; // ISO 8601 format timestamp
}
```

## Platform Support

| Platform | Support | Notes                                                           |
| -------- | ------- | --------------------------------------------------------------- |
| macOS    | ✅      | MediaRemote private framework                                   |
| Windows  | ✅      | GlobalSystemMediaTransportControlsSession API                   |
| Linux    | ✅      | MPRIS (Media Player Remote Interfacing Specification) via D-Bus |

## Developing

```bash
git clone https://github.com/JoeyEamigh/nowplaying.git
cd nowplaying

bun install
bun run build

# run the example
cd bindings && bun run example
```
