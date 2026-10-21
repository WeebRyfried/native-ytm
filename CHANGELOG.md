# Changelog

What changed in each version of Native YTM, newest first. After an update,
the app shows what is new since the version it had; this file is built into
it for that (src/changelog.rs).

## 0.3.14 — 22 October 2026

The next song in an island at the top of the screen, and a new logo.

- **An island for song changes.** The note at the top right is now an island in the middle of the top of the screen, as on an iPhone. With the window minimised or in the taskbar's corner, a black pill comes down from above the screen and springs open to the song ending, its artwork beside its title and artist; the next song slides in as the old one slides away, the island taking its width, and a few seconds later it closes and goes back up. The artwork is the one the media overlay already has, so nothing more is downloaded. It never takes a click or the focus, and stays away from games and videos in full screen.
- **Still next to nothing to run.** A frame of the island takes about a millisecond to put together and show, and frames are made only while it moves (about two seconds of the six it shows): a sixth of a second of work for each song change, and none in between. Off unless turned on: Settings, Window, Show song changes.
- **A new logo.** A music note with a window in its flag, white on a rounded square. In the app it takes the colour of your theme (red for Classic, blue for Midnight, pink for Sakura, green for Matcha) in the sidebar, on the taskbar and in the tray, and changes with it.
- **Just Music.** The name beside the logo is now Music, and a click on either takes you Home.

## 0.3.13 — 21 October 2026

A fraction of the memory, and the pointer costs nothing to move.

- **About 15 MB of memory instead of about 95.** Most of what the window starts with is the graphics driver's, used to start it and then barely again: 15 seconds after the window opens, 3 seconds after it is minimised, and when it closes to the taskbar's corner, that goes back to Windows. Task Manager now shows about 15 MB while playing, 4 MB open and idle. What is still in use comes straight back.
- **Moving the pointer no longer works the graphics card.** On a 144 Hz screen, a moving pointer had the whole window drawn up to 144 times a second whenever anything moved (even the seek bar's creep). Now no frame comes faster than 60 a second, or 30 while the pointer moves over things that stay as they are; animations hold still between their own steps (the seek bar moves in whole pixels, the playing bars ten times a second, the visualiser thirty, the lyrics' sweep a pixel at a time), so the frames between are not drawn; and each movement of the pointer no longer has Windows send a paint. Over empty space while a song plays, the graphics card goes from 9% to 3% and the processor from 21% to 5%.

## 0.3.12 — 21 October 2026

Lighter lyrics, and quicker fades.

- **About 40% less work with the lyrics open.** Each frame now goes to the graphics card in one upload rather than two for every piece of it (which lightens every screen, not just the lyrics), the line being sung is drawn in one piece rather than seven or more, frames come only while the singing moves (fewer on held notes, none between words), and only the song showing has its lyrics kept in memory. With the lyrics beside the queue: 7% of a core rather than 12%.
- **Quicker fades.** A sung line dims in about half the time it did, still by how fast it was sung (between a fifth of a second and a second), and lines carried toward the edges of the view dim in half a second.

## 0.3.11 — 21 October 2026

Fades that really go all the way down smoothly.

- **No more drop at the end of a fade.** A line timed by the word was drawn twice while it faded, dim under bright, and the two see-through layers added up: near the end of its fade it showed at about half again its resting brightness, then fell to it in one frame. Each part of a line is now drawn once, in its one colour, so it fades all the way down evenly. Backing vocals the same.
- **Lines dim toward the edges smoothly too.** As the view moves on, lines carried into the dimmer top and bottom of the lyrics get there over most of a second, instead of as fast as the view's glide.

## 0.3.10 — 21 October 2026

Fades that stay smooth all the way out.

- **No more sudden dimming halfway through a fade.** As a sung line faded, the lyrics moving on to the next line carried it into the dimmer edge at the top of the view, which darkened it all at once on top of the fade. A line still fading now keeps clear of that, and dims only as its own fade has it, at an even pace from start to end.

## 0.3.9 — 21 October 2026

Lyrics that fade out as fast as they were sung.

- **Each line fades at its own pace.** Once sung, a line and its backing vocals dim for about as long as two or three of their words took: a quick line goes out quickly, a slow, drawn-out one ebbs away, gently in either case. Lines timed only by the line go by their length over their words.

## 0.3.8 — 21 October 2026

The next song at the corner of the screen, backing vocals that ebb away, and this list after every update.

- **Song changes at the corner of the screen.** With the window minimised or in the taskbar's corner, a small note at the top right shows the song ending with an arrow, then the one coming in, for a few seconds. It never takes a click or the focus, and stays away from games and videos in full screen. Off unless turned on: Settings, Window, Show song changes.
- **Backing vocals ebb away.** Once sung, they dim slowly from where they were, rather than going out at once, and they stay lit for as long as they are sung after their line.
- **What's new, after an update.** The first time a new version opens, it says what changed since the one you had. It is in Settings, Updates too.

## 0.3.7 — 20 October 2026

Lines sung over one another, lit together.

- **More than one line at a time.** When singers overlap (a round, a call and its answer), each line stays lit and fills word by word while it is sung, as Better Lyrics has it, instead of going dim the moment the other voice comes in.
- The view follows the middle of the lines being sung, and lines brighten and dim by the song's own time, so they hold still while it is paused.

## 0.3.6 — 19 October 2026

Duets set apart: each singer on their own side, in a colour of their own.

- **Singers set apart.** When the lyrics name more than one singer, the first sings on the left, the others on the right, and lines sung together sit in the middle. With one singer, every line stays on the left.
- **A colour for each singer**, from your theme: the first in white, the second in the theme's colour, a third in its glow.

## 0.3.5 — 19 October 2026

Lyrics that are never lost, backing vocals in brackets, and translations borrowed from another source.

- **Lyrics found are never lost.** A later look is put together with what was kept and takes nothing away, even when every source fails.
- **LRCLIB, busy or down**, is asked again, and searched by title and artist together when its fields find nothing.
- **Backing vocals written in brackets** sit small under their line and fill as they are sung.
- **Translations borrowed** from another source of the same song, else from Unison, Better Lyrics' community service.

## 0.3.4 — 19 October 2026

Lyrics word by word, from every source Better Lyrics uses.

- **Lyrics timed by the word**, each word lighting up as it is sung, readings over Japanese lyrics with them.
- **Every source the Better Lyrics extension uses**, best first: its own, BiniLyrics, QQ Music, Musixmatch, KuGou and Unison, then LRCLIB and YouTube Music.
- **Backing vocals, duets and translations** under their lines, and another source a click away in the lyrics' corner.

## 0.3.3 — 19 October 2026

The artwork as a deck of cards.

- **A deck of artwork in the full-window player.** The next song's cover peeks out behind this one's in its last seconds, then comes to the front in time with the crossfade.

## 0.3.2 — 19 October 2026

A livelier visualiser, and a choice of what it follows.

- **The visualiser reacts to the music**, catching every hit as well as how loud each part is.
- **Reacts to**: everything, the drums, or the vocals (Settings, Visualiser).

## 0.3.1 — 19 October 2026

Themes, a visualiser, and Quick picks near the top of Home.

- **Four themes**: Classic, Midnight, Sakura and Matcha (Settings, Theme).
- **A visualiser** rising from the player bar, in four styles: Bars, Wave, Mirror and Aurora.
- **A soft glow** follows the pointer over songs, cards and menus.

## 0.3.0 — 16 October 2026

Covers on Discord for your own music, Premium quality after a quick start, and Play all on Home.

- **Discord shows the cover** of songs on this PC, found online once.
- **Premium quality** takes over from a song started at once by pointing at it.
- **Play all** on Quick picks and Your daily discover.

## 0.2.9 — 15 October 2026

Quicker everywhere, your listening stats, album pages for your own music, and folders that keep up.

- **Faster**: artwork in a tenth of a second, Home at once, lyrics ready when the song starts, songs that start the moment you click.
- **Your stats**, from Last.fm or the app's own count.
- **Album pages** for the music on this PC, and folders that update themselves.
- Lyrics moved half a second at a time, playlists renamed and deleted, recent searches, and starting with Windows.

## 0.2.8 — 15 October 2026

Start a radio from your own songs, artist descriptions up top, and a clearer storage bar.

- **Start radio** from a song on this PC.
- **Artist descriptions** under the banner, and every kind of storage visible in the storage bar.

## 0.2.7 — 15 October 2026

Sharp artwork in the full-window player, and lyrics at once after a restart.

- **Sharp artwork**, even when the largest size is slow to come.
- **Lyrics at once after a restart**, while LRCLIB is asked again in the background.

## 0.2.6 — 15 October 2026

Synced lyrics are back.

- **Synced lyrics when LRCLIB is busy**: the app waits as LRCLIB asks and tries again, and keeps only real answers.

## 0.2.5 — 15 October 2026

A smart crossfade, lyrics kept for offline, where your storage goes, and a symbol for every format.

- **Smart crossfade**: songs fade into one another where they go quiet.
- **Lyrics kept for offline**, and **where your storage goes**, in Settings.
- **Symbols** for FLAC, MP3, WAV, AIFF, ALAC and Vorbis, and Last.fm on artist pages.

## 0.2.4 — 14 October 2026

Artist pages from Last.fm, artwork for your own songs, and search by how Japanese titles read.

- **Artist pages** for the artists of your own songs, with Last.fm's story, tags and listeners.
- **Artwork for your own songs**, from Last.fm or YouTube Music.
- **Search by how it reads**: "torikago" finds トリカゴ.
- **Radio after your songs** when they run out.

## 0.2.3 — 14 October 2026

Your own music beside YouTube Music, a full-screen player, and updates in one click.

- **Music on this PC**: your folders of MP3, FLAC, M4A, Ogg, Opus, WAV or AIFF, in the same queue.
- **Full screen** (F11), the release year in the player, and **updates in one click**.

## 0.2.2 — 11 October 2026

An equalizer, crossfade, and readings over Japanese lyrics.

- **Equalizer**: ten bands, with presets or a curve of your own.
- **Crossfade** and **gapless playback**, as switches.
- **Readings over Japanese lyrics**: romaji, or furigana over the kanji.

## 0.2.1 — 11 October 2026

A fix for Japanese, Chinese and Korean songs.

- **Last.fm scrobbles** use the title YouTube Music's player shows, and **titles in Latin letters** everywhere a song is named.

## 0.2.0 — 11 October 2026

Downloads and an offline mode.

- **Downloads** of songs, albums and playlists, and an **offline mode** that carries on with them.
- **Last.fm play counts** in the player bar, and your plays in your YouTube Music history.

## 0.1.0 — 11 October 2026

YouTube Music as a real Windows app.

- **Premium quality**, songs kept on disk, synced lyrics, and a full-window player.
- Media keys, the tray, Discord presence and Last.fm scrobbling, light on the machine.
