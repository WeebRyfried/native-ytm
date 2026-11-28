# Changelog

What changed in each version of Native YTM, newest first. After an update,
the app shows what is new since the version it had; this file is built into
it for that (src/changelog.rs).

## 0.4.9 — 28 November 2026

Less work while hidden behind another window, and full screen that fills the screen.

- **Nothing drawn behind the window you're using.** With the window you're using wholly over the player (a maximised browser, say), or with the player on another virtual desktop, it stops drawing, as it does when minimised, and starts again within a quarter of a second once it shows. The music and everything else carry on. Only the window in front is looked at, not every window in between, so this is a simpler check than 0.4.7's, with nothing else of 0.4.7 coming back.
- **Full screen from a maximised window.** Pressing full screen while the window was maximised hid the title bar but left the window its maximised size. It now fills the screen, and goes back to maximised when you leave.
- **No light corners in full screen.** The window's rounded corners stayed rounded in full screen, and what was behind showed through at the corners. They are square there now.

## 0.4.8 — 28 November 2026

Back to how 0.4.6 drew the window.

- **0.4.7's drawing changes are undone.** Painting only what changed, the new frame pacing and skipping a covered window brought glitches on some laptops without using less of the graphics card, so the window is drawn as it was in 0.4.6 again.

## 0.4.7 — 28 November 2026

Less work for the graphics card, above all while lyrics sweep.

- **Only what changed is painted again.** While lyrics sweep word by word, each frame used to paint the whole window again; now it paints the line being sung and the seek bar, and copies the rest over as it was. Measured on a desktop with Intel graphics: about 15–25% less GPU time a frame there, and about a third less CPU (from about 12% of a core to 8–9% with the lyrics open). On integrated graphics, where filling the window is what costs, the saving should be bigger.
- **Evener frames on fast screens.** On a 144 Hz screen frames now come every third of the screen's, 48 a second, evenly, rather than two or three apart in turn: smoother, and a fifth fewer for the app and for Windows to put on screen. At 60 Hz nothing changes.
- **Nothing drawn while the window is hidden behind others.** With another window wholly over it (a maximised browser, say), the app stops drawing, as it does when minimised, and starts again within a fraction of a second once it shows: there, from about 2% of the GPU and 12% of a core to none and under 1%. A window you are using is always drawn.

## 0.4.6 — 27 November 2026

Words that light as they're sung, when the lyrics say only when each starts.

- **No more words lit whole at once.** Some of LRCLIB's lyrics timed by the word give each word only the moment it starts, not when it ends. Those were read as words taking no time at all, so each lit up in full the moment it began and the sweep snapped from word to word while the singer was still on it. Such a word now lasts until the next begins (the last, until its line ends), so the sweep moves through it as it's sung. This holds for any source timed that way, and for lyrics already kept on this PC, without looking them up again.

## 0.4.5 — 27 November 2026

Lyrics that sweep smoothly from word to word.

- **No more snapping between words.** Lyrics timed by the word lit each word at its own pace, so the sweep changed speed all at once from one word to the next, and a quick word (or one timed as taking no time at all, as some on LRCLIB and other sources are) jumped. The sweep now speeds up and slows down over a fifth of a second instead, gliding from word to word, and still keeps to the words' times. Seeking and the lyrics' earlier and later buttons still take effect at once.

## 0.4.4 — 26 November 2026

Premium quality again for the songs YouTube holds back a moment.

- **Premium quality that comes.** YouTube now gives some songs' streams a few seconds before they can be played, and says how long; asked for sooner, it turns them away. The app asked at once, was turned away, and said yt-dlp needed updating, so those songs stayed at the usual quality. It now waits as long as YouTube asks (a few seconds, while the song plays on as it was) and then switches.
- **Clearer words for the switch.** While it happens, the bitrate's note says "Transitioning to Premium quality…"; once done, "Switched to Premium quality seamlessly".

## 0.4.3 — 26 November 2026

LRCLIB's lyrics word by word, seeking in the middle of a crossfade, and Last.fm's stats when Last.fm is down.

- **LRCLIB, word by word.** LRCLIB now times some songs by the word, and the app reads that: those songs sweep word by word, as with the other sources, where before they lit a line at a time. When LRCLIB has a song twice, timed by the line and by the word, the one timed by the word is taken. Lyrics kept from before are looked for again once, so songs already played pick it up.
- **One LRCLIB in the sources' menu.** A source with lyrics timed by the word no longer also offers the same lyrics timed only by the line, and a newer answer from a source takes the place of what it gave before.
- **Seeking during a crossfade.** In the last seconds of a song, once the next had been lined up to fade in, a seek (or Previous, back to the start) was lost and the next song took over all the same. The song playing now goes back to where you asked, and the next waits its turn again. Changing what comes next in those seconds now changes it too, rather than the old next song coming in anyway.
- **Song changes whenever the window isn't in front.** With Show song changes on, the island now also comes down when the window is open but behind another window or on another screen; only the window in front (the one you're using) keeps it away.
- **Last.fm's stats when Last.fm is down.** Last.fm's servers are failing to give out listening data for every account at the moment (error 8, "Operation failed"), which left the stats page with only that error. Last.fm's stats are now kept as they last came: they show at once while the latest are asked for, and stay, said to be from then, when Last.fm can't give them, with Try again. Without any kept yet, the page says the trouble is on Last.fm's side and offers Try again or this app's own stats. Scrobbles were never lost: they still go through, and wait their turn if Last.fm turns them away.
- The stats page no longer copies its whole list of artists, songs and albums each time it is drawn.

## 0.4.2 — 25 November 2026

Syncing lyrics by listening, taken out again.

- **No more Sync to the song.** Timing lyrics by sending the song to Fish Audio didn't come out well enough, so the buttons are gone, and so are the lyrics it saved: songs show their own lyrics again. The title bar's fix and the rest of 0.4.1 stay.

## 0.4.1 — 25 November 2026

Lyrics synced by listening to the song, and a title bar that blends in on every theme.

- **Sync to the song.** Lyrics that come as plain text now have a **Sync to the song** button: Fish Audio's speech recognition listens to the song and each word takes the time it was sung at, so they sweep word by word like the rest. Lyrics timed only by the line can be synced word by word from the sources' menu. Tried on a song LRCLIB has timed, the lines landed within about half a second of LRCLIB's on average.
- **Lyrics for songs that have none.** Where no source has the song, **Write them down from the song** turns what is heard into timed lyrics of its own (it may mishear).
- **What it is doing, as it goes:** four small steps under the button (fetch, prepare, listen, sync) tick off as each is done, with a line saying what is happening; if it stops, it says why, with Try again.
- **Only on a click,** for the song showing, one song at a time, so the service is never asked on its own. A song takes ten to twenty seconds, and what is found is kept with the song's other lyrics, there next time and offline. Settings, Advanced takes a Fish Audio key of your own.
- **The title bar blends in on every theme.** It was the sidebar's colour, darker than the page beneath it, so its lower half read as a black band; it is the page's colour now. With the full-window player open, the artwork's colours carry on up through it, as they do in the Artwork theme.
- **Cancelling a sign-in from the browser** now really cancels it: an answer that arrives after Cancel is let go rather than signing you in.
- An unused library is no longer built in.

## 0.4.0 — 25 November 2026

Sign in from your browser, a title bar of its own, and a theme from the artwork.

- **Sign in from your browser.** No more copying a cookie out of the developer tools: Sign in takes your YouTube Music sign-in straight from your browser, through yt-dlp. On the first start the app asks whether to do this by itself, now and whenever you're signed out. Firefox works; Chrome and Edge lock their sign-in away from other apps, so for those a cookie can still be pasted.
- **yt-dlp in a click.** Without yt-dlp, a small card in the corner offers to install it (the × puts it away for good). Settings, yt-dlp shows its version, checks for a newer one and updates it, and keeps it up to date by itself, checked once a day.
- **A title bar of its own.** Windows' title bar is gone: the app's own carries the logo, the name and the version in the middle, the theme's accent glowing along its top and fading into the window beneath, with its own minimise, maximise and close. The sidebar no longer repeats them, and starts with Home. Drag it to move the window, double-click it to maximise, and the window's edges still resize it.
- **The Artwork theme.** A fifth theme: the song's artwork, blurred and darkened behind the whole window, as the full-window player has it, fading to each new song's. The title bar, sidebar, side panel and player bar sit on it without seams, and the accent, buttons and lists take the artwork's colour. It costs one more layer of a tiny image; with the full-window player open, only the strips it leaves are drawn. The logo, the taskbar and the tray follow too.
- **When Premium quality doesn't come.** A small warning beside the bitrate says why the better stream couldn't be had; a click tries again (or switches to it at once, when it was found but couldn't take over mid-song).
- **Lyrics, again.** Lyrics that couldn't be found can be looked for again, from the sources' menu or the Try again button.
- **The timing control fades.** The lyrics' earlier and later buttons now come and go with the pointer, as the sources' pill does, rather than staying on screen when moved.
- **A song queued twice** lights up only where it is playing, not at both places in the queue.
- **Shorter settings.** Every setting says what it does in a few words.

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
