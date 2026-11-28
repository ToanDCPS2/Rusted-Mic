# Rusted Mic

An unofficial Rust port of [Friday Night Funkin'](https://github.com/FunkinCrew/Funkin)
by The Funkin' Crew, translated from the game's open-source Haxe code (Apache-2.0).
It is a native program for Windows and Linux (wgpu + cpal) that plays the real game's songs and
charts with the original judgement, scoring, menus and options.

> **You need your own copy of Friday Night Funkin'.** This repository and its
> releases contain **no game assets**: no art, music, sounds, charts, fonts or
> videos. On first launch Rusted Mic asks for the folder of your installation
> (itch.io or a self-built copy, version 0.8.x) and packs its `assets`
> folder into `assets.pak` next to the executable. Without it the game will not
> start. Get the game on Steam or
> [itch.io](https://ninja-muffin24.itch.io/funkin), or build it from the
> [official source](https://github.com/FunkinCrew/Funkin).

- **Platform:** Windows 11, x86_64 (Windows 10 should work but is untested), and
  Linux x86_64 (built on Ubuntu 26.04, needs glibc 2.43+; lightly tested). Everything uses portable crates
  or has a Linux code path.
- **Version:** 0.3.0, a pre-release. See *What's new in 0.3.0* below.
- **Status:** early and incomplete. See *How much is ported* and *Known issues*.

## Credits

- **Friday Night Funkin'** is made by [The Funkin' Crew](https://github.com/FunkinCrew):
  ninjamuffin99 (programming), PhantomArcade (animation), evilsk8r (art),
  Kawai Sprite (music) and the many contributors to the
  [Funkin' repository](https://github.com/FunkinCrew/Funkin/graphs/contributors).
  The game logic here is a translation of their code. All credit for the game's
  design belongs to them.
- Ported from upstream commit `94ee0f2a9386d80e445be39ee2ed4b4a630cfbac`
  (the 0.8.7 source). It is tested against the 0.8.6 release's assets.
- Rusted Mic is not affiliated with or endorsed by The Funkin' Crew Inc.
  Please support the original game.

## How much is ported

Roughly **70% of the player-facing game** is translated, judged by feature
rather than line count. The original has about 122,000 lines of Haxe; this port
is about 39,000 lines of Rust. The original's editors (chart editor, animation
editor and others, ~29,000 lines), mobile code and the Newgrounds integration
are out of scope and count as 0%. The game's own `.hxc` scripts, and V-Slice
mods' scripts, are not ported: they run unchanged through the built-in
interpreter (see *Mods*).

| Area | Ported | Notes |
|---|---|---|
| Chart loading, conductor, BPM / time-signature changes | ~95% | All 2.x song data, variations and difficulties |
| Judgement, scoring, health, combo, ranks | ~95% | PBOT1 curve and hit windows, hold bonuses and drop penalties |
| Notes, holds, strumlines, note styles (incl. pixel) | ~90% | Note splashes and hold covers from the note style |
| HUD: health bar, icons, score, popups, countdown | ~80% | |
| Stages and characters | ~75% | Drawn and animated, including Animate atlases. Stage and character scripts run through the `.hxc` interpreter (script-fetched characters, sprite groups, skewed sprites, backdrops, trails, `FlxText` and lip-sync sprites inside atlases included); some engine APIs they call are still missing |
| Song events (camera focus, zoom, bops, animations) | ~70% | |
| Pause menu | ~85% | Resume, restart, change difficulty, practice mode, offset hotkeys |
| Options (preferences, controls, lag adjustment, clear save) | ~90% | Plus Middlescroll, Hide Opponent Notes, Ghost Tapping and Psych Engine's time bar and health bar (all off by default), and Note Splashes and Antialiasing toggles (on by default). No gamepads. |
| Title screen, main menu | ~90% | Merch opens the store page |
| Freeplay | ~65% | Song list, difficulties, previews and DJ, for Boyfriend and Pico (with Pico's DJ, card and style), album art and difficulty stars, saved scores and clear percentages, the "Press [ TAB ]" hint and the slide to and from character select; no letter sort or favourites |
| Character select | ~85% | Opened with Tab in freeplay: the grid, locks, both characters and their partners, Nene's speaker visualizer, cursors and sounds. The first visit plays the lights video, then each newly unlocked character's lock opens (remembered in `save.json`). No nametag mosaic or confirm pitch bend; Pico counts as unlocked without beating Weekend 1, since there are no saved scores |
| Transitions (fade, sticker transition) | ~70% | |
| Game over screen | ~80% | Death animations, camera, music, retry and Tankman's quotes; Pico's can explosion death (2hot) and the 1-in-4096 fakeout are not ported |
| Results screen | ~80% | The full results screen: score, tallies and clear percentage counting up, the rank and its music, and the player character's rank animations (scripted ones included); new highscores are saved |
| Credits | ~90% | The base game builds its credits into `Funkin.exe`; setup recovers them from your own copy into `data/credits.json` |
| Mods (V-Slice / Polymod) | ~60% | Mods in the `mods` folder are layered over the assets in a load order you set in the mods menu, and their `.hxc` scripts run through the interpreter. Many common engine APIs are bound; mods that call ones that aren't yet log a warning for that call and carry on |
| Story mode | ~60% | The story menu, and each week played through in order with its cutscenes |
| Dialogue, cutscenes, videos | ~75% | Dialogue, scripted in-engine cutscenes (Spaghetti's intro included, skippable with the cutscene key) and every base-game video (H.264, HEVC and VP9, in MP4 or MKV) play. Spaghetti's audio, which the base game keeps inside `Funkin.exe`, is extracted from your own copy on first launch |
| Script shaders | ~70% | The game's GLSL (and mods') is translated at runtime: `FlxRuntimeShader` and script classes extending it, `AdjustColorShader`, `DropShadowShader`, `RuntimeRainShader`, `SserafimShader`, `GaussianBlurShader`, `WiggleEffectRuntime`, on sprites (`sprite.shader`) and as camera filters (`FlxG.camera.filters = [new ShaderFilter(shader)]`). `BlurFilter`, `TextureSwap`, `OverlayBlend` and filters on cameras other than camGame are not supported yet |
| Sound tray, debug display, screenshots, Discord presence | ~80% | |

## Known issues

- **Not ported:** gamepad support, freeplay letter sort and favourites, Pico's
  can explosion death (2hot) and the 1-in-4096 game over fakeout, character
  select's nametag mosaic and confirm pitch bend, Pico's card confirm
  animation, the `BlurFilter`, `TextureSwap` and `OverlayBlend` shaders, and
  shader filters on cameras other than camGame.
- **Scripts and mods:** some engine APIs are still missing, so a few stage
  effects and mod features don't happen (each logs a warning). Starting a
  song part-way in (`--start`) can confuse scripts that expect to have seen
  the beginning; play from the start when testing cutscenes.
- **Recently added and only lightly tested:** the Linux build, note splashes,
  animated health icons, the mod load order and restart, the cutscene pause
  menu, and the Antialiasing option.
- Character select treats Pico as unlocked even before Weekend 1 is beaten.
- The pause music keeps playing when the window loses focus.
- The score text uses the VCR TrueType font. The newest upstream code uses a
  bitmap version that the 0.8.6 assets don't include.
- Discord Rich Presence needs your own Discord application id (see below).
- Linux: built on Ubuntu 26.04 (WSL) and only lightly tested.

## What's new in 0.3.0

A pre-release. Everything below is new since 0.2.0.

### Scripting and mods
- **An `.hxc` interpreter** (`crates/funkin-script`): a lexer, parser and
  tree-walking evaluator for HScript as Polymod accepts it, with `#if` defines,
  string interpolation, enums, closures, `switch` patterns and scripted classes
  that extend engine classes (`Stage`, `Song`, `BaseCharacter`, `FunkinSprite`,
  `Module`, `DialogueBox`, ...). The base game's stage, song, character,
  note-kind, module and player scripts now run as written instead of being
  hand-ported, and so do mods'.
- **A Haxe/Flixel/V-Slice API for scripts**: `PlayState.instance`, stages and
  their props, characters, sprites and sprite groups, `FlxText`, `FlxTween`
  and `FlxEase`, `FlxTimer`, sounds and music, cameras (including extra
  `FlxCamera`s, shakes, flashes and fades), `FlxBar`, strumlines, notes and
  holds (positions, alpha, scale, cameras), the health bar and icons, the
  conductor, song events, `Countdown.skipCountdown`, Polymod reflection
  (`scriptCall` / `scriptGet` / `scriptSet`), colour transforms and more.
  APIs that aren't bound yet resolve to inert stand-ins and log one warning,
  so a script keeps running.
- **Mods folder**: V-Slice / Polymod mods (folders with `_polymod_meta.json`)
  in `mods/` next to the executable are layered over the game's assets:
  their files replace or add to the base game's, and their scripts load with
  the game's own. Psych Engine packs are listed but can't load.
- **Mods menu** (Tab on the main menu): every mod with its icon and metadata
  (title, author, version, API version, license, contributors, description).
  Space or the switch turns a mod on or off. **Load order**: `[` / `]`
  (or Page Up / Page Down, or the buttons in the details panel) move a mod
  earlier or later; later mods override earlier ones. Each tile shows its
  place (#1, #2, ...). Both are saved in `mods.json` in the data folder.
  **Leaving the menu with a different set of mods, or a different order,
  restarts the game** so everything loads with the new set.
- Tested with large script-driven mods (QT, the Silly Billy V-Slice port and
  others): custom health bars and icons, extra cameras, moving and fading
  strumlines, mid-song character changes, video cutscenes and lyrics.

### Story mode, cutscenes and video
- **Story mode**: the story menu (level titles, props bopping to the music,
  difficulty picker, track list), each week played through in order, and the
  week's score saved.
- **Dialogue**: Week 6's dialogue boxes, portraits and typing text
  (`Conversation`, `DialogueBox`, `Speaker`), scriptable as in the original.
- **Scripted cutscenes** in the engine (Spaghetti's intro, Weekend 1, Week 7
  and others), skippable with the cutscene key.
- **Video cutscenes**: every base-game video plays, all decoders built in
  (H.264 via OpenH264, HEVC via libde265, VP9 via rusty_vp9; MP4 and MKV;
  AAC and Vorbis audio). Scripts get `FunkinVideoSprite` / `FlxVideoSprite`
  with their signals, pausing, volume and seeking.
- **Pausing a video** opens the cutscene pause menu (Resume, Skip Cutscene,
  Restart Cutscene, Exit to Menu), as in V-Slice. The video's sound pauses
  with it, a restarted video no longer leaves the old copy's sound playing,
  and a song held by a cutscene stays held when you resume.
- Spaghetti's songs, which the base game keeps inside `Funkin.exe`, are
  recovered from your own copy on first launch.

### Gameplay and HUD
- **Note splashes** on sick hits (botplay included), from the note style's
  own splash data: random variants, slightly varied speed, at most six at a
  time, and the pixel style's screen blend.
- **Hold covers** while a sustain is held.
- **Animated health icons**: icons with an `icon-<id>.xml` play their idle,
  winning and losing animations (and the transitions between them, when
  present). Mid-song icon changes (`SetHealthIcon`) work.
- Characters' `idleSuffix`, `onAnimationFinished`, and a movable
  `cameraFocusPoint`, so character scripts can swap idles and shift the camera
  (Silly Billy's shrink scene).
- Sprites placed inside an Adobe Animate atlas symbol (lip sync in Spaghetti)
  are positioned exactly as flixel-animate does, including mirrored symbols.
- GF's combo reactions, Pico's game over deaths from the playable characters'
  scripts, sticker transitions, camera fades, `FlxTiledSprite` and backdrops.

### Menus
- **Character select** (Tab in freeplay): the grid, locks and unlock
  animations, both characters and their partners, Nene's speaker visualizer,
  the first-visit lights video. Playing as **Pico** in freeplay, with his DJ,
  card and note style.
- **Freeplay**: album art and difficulty stars, saved scores and clear
  percentages.
- **Results screen**: ported in full (see the table above).
- **Credits**: recovered from your `Funkin.exe` by setup.

### Graphics
- **Script shaders**: the game's and mods' GLSL is translated at runtime for
  sprites and camera filters (see the table above). New blend mode: screen.
- **Antialiasing** option: off draws everything nearest-neighbour, menus
  included, and applies at once.

### Options
- **Note Splashes** (on by default) and **Antialiasing** (on by default).
- Middlescroll, Hide Opponent Notes, Ghost Tapping and the Psych Engine time
  bar and health bar (from 0.2.0) are unchanged.

### Platforms and builds
- **Linux** builds (x86_64), from the same code: cpal/ALSA audio, winit with
  X11 and Wayland, GTK folder picker for setup.
- The game's data folder is `~/.local/share/Rusted Mic` on Linux.
- Releases contain an empty `mods` folder with a `README.txt` explaining how
  to install mods.

### Fixes
- Pausing and skipping the Weekend 1 intro video: no leftover sound after a
  restart, no black screen after a skip, and Pico no longer misses notes in
  the in-level cutscene after a skip.
- Strumline positions are published to scripts before `onCountdownStart`, so
  scripts that remember them get real values.
- Many smaller script-compatibility fixes found while running mods.

## Download and run

1. Download the archive for your system from the latest release and unpack it
   into its own folder:
   - Windows: `RustedMic-0.3.0-windows-x64.zip` (`RustedMic.exe`).
   - Linux: `RustedMic-0.3.0-linux-x64.tar.gz` (`RustedMic`). It needs ALSA
     and GTK 3 at run time (present on most desktops), X11 or Wayland, and
     glibc 2.43 or newer (Ubuntu 26.04 or a similarly recent distro). On an
     older distro, build from source (see *Building (Linux)*).
2. Run it. The setup screen scans for your Friday Night Funkin' installation, or
   lets you browse to it. It packs the assets into `assets.pak` (about 800 MB)
   next to the executable. This happens once.
3. Play. Controls: WASD or arrow keys by default (rebind them under Options →
   Controls).
4. Optional: put mods in the `mods` folder (see `mods/README.txt` and *Mods*
   below).

## Mods

Rusted Mic loads V-Slice (Polymod) mods, the kind made for the official game
0.5 and later. Each mod is a folder with a `_polymod_meta.json` inside; put it
in the `mods` folder next to the executable, so it looks like
`mods/<Mod Name>/_polymod_meta.json`. Don't put the mod's zip there unpacked
one level too deep (`mods/<Mod Name>/<Mod Name>/...`).

- Mods are on when first found. Open the mods menu with **Tab on the main
  menu** to see them, turn them on or off (**Space**), and set their **load
  order** (**`[`** / **`]`**). When two mods change the same file, the later one
  in the order wins.
- Leaving the mods menu after changing the mod set or order **restarts the
  game**, so all songs, characters, stages and scripts load with it.
- Mod scripts (`.hxc`) run through the built-in interpreter, unchanged. When a
  mod uses an engine feature Rusted Mic doesn't have yet, that call does
  nothing and a warning goes to the log (run the game from a terminal with
  `RUST_LOG=warn` to see it); the rest of the mod keeps working.
- Psych Engine mods (`pack.json`) are shown in the menu but can't be loaded.
- `RUSTED_MIC_MODS=FolderA,FolderB` loads only those folders and ignores the
  menu's settings (`none` loads no mods). Meant for testing.

## Layout

```
crates/
  funkin-core/     engine-independent logic (no I/O except reading game files)
    vfs.rs         asset pack format + virtual file layer (pack first, then disk)
    constants.rs   ← util/Constants.hx
    conductor.rs   ← Conductor.hx (BPM / time-signature changes, step/beat/measure)
    scoring.rs     ← play/scoring/Scoring.hx (PBOT1, Week7, Legacy), ranks, tallies
    song/          ← data/song/SongData.hx, SongRegistry.hx, play/song/Song.hx (voice resolution)
    play/          ← PlayState.hx input queue + processNotes, Strumline.hx, GRhythmUtil.hx
    stage.rs       ← data/stage/StageData.hx, data/character/CharacterData.hx
    camera.rs      ← PlayState camera (follow, zoom, bops) + FlxEase
    state.rs       ← state flow: Setup → Title → Menus → Play → GameOver/Results
  funkin-assets/   first-boot importer (optional, removable)
    detect.rs      scan Steam libraries (registry + libraryfolders.vdf), itch, Program Files, ...
    validate.rs    check exe, assets/ layout, parsable 2.x song data, audio presence
    import.rs      pack assets/ (+ the exe's embedded fonts) → assets.pak next to the exe
    marker.rs      finds and mounts assets.pak on boot; install.json for older/reference imports
  funkin-game/     the `RustedMic` binary (package `rusted-mic`)
    gfx/           wgpu sprite batcher, Sparrow + Adobe Animate atlas loaders, alphabet-atlas text
    actor.rs       ← FlxSprite/Bopper: animations, offsets, hitbox placement, dancing
    world.rs       ← Stage.hx, BaseCharacter.hx, camera song events
    audio.rs       cpal mixer + OGG decoding, sample-accurate song clock
    video.rs       MP4/MKV cutscenes: OpenH264, libde265 and rusty_vp9 video; Symphonia AAC, lewton Vorbis
  de265-sys/       libde265 1.0.16, vendored and compiled with `cc` (no CMake)
    save.rs        ← Save.hx: progress kept in save.json
    play.rs        gameplay screen: strumlines, notes, holds, countdown, popups, health bar
    app.rs         winit event loop; Setup → Title → Menus → Play → Results / Game Over
    setup_screen.rs  first-run screen: pick the game folder, pack with a progress bar
    logo.rs        the Rusted Mic logo, painted in code (no image files)
```

## Building (Windows 11)

Prerequisites: [rustup](https://rustup.rs) and the *Visual Studio Build Tools*
with the "Desktop development with C++" workload (provides the MSVC linker).

```powershell
cargo build --release
cargo test
```

The C runtime is linked statically (`.cargo/config.toml`), so the resulting
`target\release\RustedMic.exe` runs without the VC++ redistributable.

## Building (Linux)

Needs a C/C++ compiler (OpenH264 and libde265 are compiled from source), `pkg-config`, and the
ALSA and GTK 3 development packages (for audio and the folder picker), e.g. on
Debian/Ubuntu `sudo apt install build-essential pkg-config libasound2-dev libgtk-3-dev`.

```sh
cargo build --release
```

The binary is `target/release/RustedMic`.

## Using it

```powershell
RustedMic                             # open the game (the setup screen comes first if needed)
RustedMic play bopeebo hard           # jump straight into a song (add --botplay to watch)
RustedMic play 2hot hard --story      # play as story mode: cutscenes, then the rest of the song's week
RustedMic play 2hot hard --start 110  # start 110 s in (no countdown or opening cutscene)
RustedMic setup                       # command-line setup: scan for an installation, or browse
RustedMic setup --path "D:\Games\Funkin" --reference   # read in place, no copy
RustedMic songs                       # list songs and story levels
RustedMic simulate bopeebo hard       # run a chart through the real judgement logic
RustedMic reset                       # forget the import
RustedMic logo out.png                # write the logo to a PNG
RustedMic logo --square icon.png      # 1024x1024 logo for Discord's Rich Presence art
```

### Settings

Options (main menu → Options) has Preferences, Controls, Lag Adjustment and
Clear Save Data, as in the original, plus new preferences: **Middlescroll**
(your strumline centred, the opponent's split to the sides), **Hide Opponent
Notes**, **Ghost Tapping** (key presses with no note aren't penalized), and
Psych Engine's **time bar** (which also moves the arrows to Psych's position)
and **health bar** (in the characters' colours). All of these are off by default.
**Note Splashes** (splashes on sick hits) and **Antialiasing** (smooth
graphics; off draws everything nearest-neighbour) are on by default. Everything is saved to `settings.json` in the data folder: `%LOCALAPPDATA%\Rusted Mic` on Windows, `~/.local/share/Rusted Mic` on Linux. Progress (scores, characters seen in character select) goes to `save.json` next to it, and the mods menu's choices to `mods.json`.

Default hotkeys (all rebindable): F11 fullscreen, F3 screenshot (saved to
`screenshots\` next to the exe; click the preview to open the folder), `-`/`=`/`0`
volume down/up/mute, F6 cycles the debug display, P/Enter/Esc pause, R resets.
Pressing `-` and `=` together toggles botplay (for the current song and the
next ones; the `--botplay` command-line flag still starts with it on).

### Discord Rich Presence

Discord shows an application's name and art, so presence needs your own
Discord application:
1. In the Discord Developer Portal, create an application named **Rusted Mic**.
2. Under Rich Presence → Art Assets, upload `RustedMic logo --square icon.png`'s
   output with the name `logo`.
3. Put the application id in `settings.json` as `"discord_app_id": "<id>"` (or
   set `RUSTED_MIC_DISCORD_APP_ID`).

With Discord running, your profile then shows "Playing Rusted Mic" with the
logo, the song and difficulty, and the elapsed time. Turn it off with the
Discord RPC preference.

### The asset pack

Setup reads every file in the game's `assets` folder, plus the fonts embedded in
`Funkin.exe`, into a single `assets.pak` next to `RustedMic.exe`. Text files
(JSON, XML, scripts) are zstd-compressed; audio, images and video are already
compressed and are stored untouched, so nothing loses quality. Reading from one
file is at least as fast as loose files (no per-file open or virus-scan cost).
Delete `assets.pak` to run setup again. The pack holds the game's assets, so
keep it out of version control and don't share it.

The data directory can be overridden with `RUSTED_MIC_DATA_DIR`.

### Removing the importer

Build with `cargo build --no-default-features`. The `funkin-assets` crate is then
not compiled at all, and you pass `--assets <game>\assets` yourself.

## Accuracy notes

Ported faithfully: PBOT1 scoring curve and windows, health values, combo rules,
hold note bonus and drop penalties, ghost-miss penalties (ghost tapping is
**off** by default, matching desktop builds), rank thresholds, time-change math, and voice
file resolution, including its quirks.

Deliberate differences, all aimed at determinism (see `play/mod.rs`):
- Inputs carry their own song timestamp instead of "frame time minus measured latency".
- Hit windows are refreshed before the input queue runs, so judgement doesn't depend on frame rate.

Controls: WASD or arrow keys by default; rebind them under Options → Controls.

See *Known issues* above for what is not yet ported.

## Legal

The code is a derivative of Funkin' source code, used under the Apache License
2.0 (see `LICENSE-APACHE` and `NOTICE`). "Friday Night Funkin'" is a trademark of
The Funkin' Crew Inc. This project is unaffiliated. The game's assets are under a
separate license and are never redistributed by this project.

Video decoding uses [OpenH264](https://github.com/cisco/openh264) (BSD-2-Clause),
compiled from source. Cisco's H.264 patent licence covers only its own prebuilt
binaries, so builds of this project don't carry it.
HEVC cutscenes are decoded by [libde265](https://github.com/strukturag/libde265)
(LGPL-3.0-or-later), whose source is included in `crates/de265-sys/libde265` and
linked statically. Under the LGPL you may rebuild the game against a modified
libde265 from that folder. VP9 cutscenes use [rusty_vp9](https://crates.io/crates/rusty_vp9)
(Apache-2.0). All decoders are built into the game, so no codec downloads are needed.
