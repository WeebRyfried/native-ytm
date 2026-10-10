# native-ytm

A lightweight, native YouTube Music client for Windows, built the way
[Spotifast](https://github.com/crmne/spotifast) is: Rust and
[egui](https://github.com/emilk/egui) on Spotifast's own foundation,
[fastframe](https://github.com/crmne/fastframe). There is no browser engine
and no webview. The interface is drawn natively with OpenGL, the catalogue
comes from YouTube Music's JSON API, and the audio is streamed, decoded and
played by a small engine in Rust.

This is a personal experiment, not affiliated with Google or YouTube.

![Search, the queue and the player bar](docs/screenshot.png)

## What it does

- **Home, Explore, search** (with filters and suggestions), **albums,
  artists, playlists**, moods and charts, with infinite scrolling.
- **Playback** straight from YouTube's servers: AAC at about 128 kb/s,
  resampled to whatever rate the output device runs at, with **gapless**
  transitions (the next song is fetched and decoded before this one ends),
  instant seeking, and YouTube's loudness normalisation.
- **Queue**: play next, add to queue, drag to reorder, shuffle, repeat
  (all or one), and **autoplay radio** that keeps going when the queue runs
  out, as YouTube Music does.
- **Lyrics** for the song playing.
- **Signed in** (optional): your playlists in the sidebar, Liked music,
  the Library (playlists, songs, albums, artists, subscriptions, history),
  like and dislike, save albums and playlists, subscribe to artists, save to
  a playlist, create playlists, remove songs from your own playlists.
- **Windows integration**: the media keys and the volume overlay's
  controls (System Media Transport Controls), a tray icon that keeps the
  music playing with the window closed, one running copy (a second launch
  brings the window back), and a dark title bar.
- The queue and position are restored at the next start.

## How it works

| Part | Where | What |
| --- | --- | --- |
| Interface | `src/ui/` | egui views; they emit `Action`s, which `src/app.rs` applies after each frame |
| App state | `src/app.rs`, `src/queue.rs` | navigation, the queue, what plays next, preloading, autoplay |
| Network | `src/backend.rs` | a two-thread tokio runtime; the interface never waits on it |
| API | `src/api/` | InnerTube (`WEB_REMIX`) requests and the parsers that turn renderers into the model |
| Streams | `src/stream.rs` | finds an audio address for a video id (see below) |
| Audio | `src/player/` | HTTP range downloader, symphonia demuxer and AAC decoder, rubato resampler, fastframe-audio output |
| Artwork | `src/images.rs` | fetched and decoded off the interface thread, cached on disk, dropped from memory when unused |

### Getting the audio

YouTube serves direct audio addresses to some of its own clients without
the JavaScript challenge or proof-of-origin token the web player needs.
`src/stream.rs` asks the player endpoint as one of those (visionOS, then
Android VR), takes the AAC stream (itag 140), and checks the last kilobyte
before trusting it, since some clients' addresses stop working after the
first megabyte. This takes about 0.2 s.

YouTube changes which clients work. When none does, the app falls back to
[yt-dlp](https://github.com/yt-dlp/yt-dlp) if it is installed (it is
looked for on `PATH`, or set its path in Settings). yt-dlp needs a
JavaScript runtime for YouTube; if Deno is not installed and Node is, the
app tells yt-dlp to use Node. Keep yt-dlp current (`yt-dlp -U`, or your
package manager): an old one finds addresses that YouTube cuts off after the
first megabyte, which the app detects and reports. To fix the native path, update the client
list in `PlayerClient::defaults` (`src/api/client.rs`) from yt-dlp's
`_base.py`.

## Signing in

Google no longer lets third-party apps sign in to YouTube Music with OAuth
in a way that works for this API, so the app signs in the way
[ytmusicapi](https://github.com/sigma67/ytmusicapi) does: with your
browser's cookie.

1. Open <https://music.youtube.com> in your browser, signed in.
2. Press F12, open **Network**, and filter for `browse` (click around if
   the list is empty).
3. Select a request to `music.youtube.com` and copy the value of the
   `cookie` request header. (*Copy as cURL* or a `cookies.txt` export also
   work.)
4. In the app: **Sign in**, paste, **Sign in**.

The cookie is stored encrypted with Windows DPAPI, readable only by your
Windows account, in `%APPDATA%\native-ytm\config\account.bin`. Signing out
of YouTube in that browser profile ends the app's session too.

## Building

You need Rust 1.98 or newer and the MSVC build tools.

```bash
cargo build --release
```

The executable is `target\release\native-ytm.exe`, one file with
everything inside.

`ytm-probe` is a console tool for when YouTube changes something:

```bash
cargo run --release --bin ytm-probe -- dump search "daft punk" songs
```

Run it without arguments to see its commands (`dump` prints what the
parsers make of live pages, `play` resolves and plays a stream, `icon`
redraws `assets/app.ico`).

To check the interface without clicking through it, the app takes a few
environment variables (any build): `NATIVE_YTM_ROUTE` opens a page or
plays something at start (`search:QUERY`, `browse:ID`, `play:VIDEO_ID`,
`explore`, `library`, `settings`, `queue`, `lyrics`, `signin`, joined with
`|`), `NATIVE_YTM_SHOT=out.png` saves a screenshot after
`NATIVE_YTM_SHOT_DELAY` milliseconds and quits, `NATIVE_YTM_SCROLL` scrolls
the page, `NATIVE_YTM_SEEK` jumps into the song, and `NATIVE_YTM_MUTE`
plays silently.

## Keyboard

| Keys | Does |
| --- | --- |
| Space | Play or pause |
| Ctrl + Right / Left | Next / previous song |
| Shift + Right / Left | Forward / back 10 seconds |
| Ctrl + Up / Down | Volume |
| M | Mute |
| Ctrl + F or / | Search |
| Alt + Left / Right, mouse back / forward | Back / forward |
| Q | Show the queue |
| L | Like the song |
| Ctrl + R or F5 | Reload the page |

## Where things are kept

- Settings, the queue and the sign-in: `%APPDATA%\native-ytm\config\`
- Artwork cache and logs: `%LOCALAPPDATA%\native-ytm\cache\`

## Footprint

Measured on this machine with the release build: a 14 MB executable, the
window up about 0.1 s after launch, about 0.2% of one core when idle and
4% while playing with the window open (0.3% with it closed to the tray), and
about 170 MB of private memory with the window open (87 MB in the tray).
Some 58 MB of that is the system fonts for Chinese, Japanese, Korean and
other scripts, loaded so every title draws.

## Limits

- Audio only, AAC at about 128 kb/s: the decoder has no Opus, and
  Premium's 256 kb/s stream needs the web client's token.
- No video, no podcasts' episode lists beyond what pages show, no
  uploads, no offline downloads.
- YouTube changes its API without notice. The parsers look for item
  renderers wherever they are rather than at fixed paths, which survives
  most changes; stream access is the part most likely to need updating.

## Credits

- [Spotifast](https://github.com/crmne/spotifast) and
  [fastframe](https://github.com/crmne/fastframe) by Carmine Paolino (MIT):
  the design this follows, and the crates for audio output, media controls,
  the tray, fonts, icons and the window shell.
- [ytmusicapi](https://github.com/sigma67/ytmusicapi) and
  [yt-dlp](https://github.com/yt-dlp/yt-dlp): what YouTube Music's API looks
  like, and which clients still get streams.
- Icons from [Lucide](https://lucide.dev) (ISC). Interface font
  [Inter](https://rsms.me/inter/) (SIL OFL 1.1), bundled by fastframe-fonts.
