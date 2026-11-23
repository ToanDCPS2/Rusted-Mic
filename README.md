# Rusted Mic

An unofficial Rust port of [Friday Night Funkin'](https://github.com/FunkinCrew/Funkin)
by The Funkin' Crew, translated from the game's open-source Haxe code (Apache-2.0).
It is a native Windows program (wgpu + cpal) that plays the real game's songs and
charts with the original judgement, scoring, menus and options.

> **You need your own copy of Friday Night Funkin'.** This repository and its
> releases contain **no game assets**: no art, music, sounds, charts, fonts or
> videos. On first launch Rusted Mic asks for the folder of your installation
> (itch.io or a self-built copy, version 0.8.x) and packs its `assets`
> folder into `assets.pak` next to the executable. Without it the game will not
> start. Get the game on Steam or
> [itch.io](https://ninja-muffin24.itch.io/funkin), or build it from the
> [official source](https://github.com/FunkinCrew/Funkin).

- **Platform:** Windows 11, x86_64 (Windows 10 should work but is untested). The core
  crates are platform-neutral, but only Windows is built and tested.
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

Roughly **50% of the player-facing game** is translated, judged by feature
rather than line count. The original has about 122,000 lines of Haxe; this port
is about 18,000 lines of Rust. The original's editors (chart editor, animation
editor and others, ~29,000 lines), mobile code, mod scripting and the Newgrounds
integration are out of scope and count as 0%.

| Area | Ported | Notes |
|---|---|---|
| Chart loading, conductor, BPM / time-signature changes | ~95% | All 2.x song data, variations and difficulties |
| Judgement, scoring, health, combo, ranks | ~95% | PBOT1 curve and hit windows, hold bonuses and drop penalties |
| Notes, holds, strumlines, note styles (incl. pixel) | ~85% | No note splashes |
| HUD: health bar, icons, score, popups, countdown | ~80% | |
| Stages and characters | ~60% | Drawn and animated, including Animate atlases and Weekend 1's rain shader; most stage and character scripts are not ported |
| Song events (camera focus, zoom, bops, animations) | ~70% | |
| Pause menu | ~85% | Resume, restart, change difficulty, practice mode, offset hotkeys |
| Options (preferences, controls, lag adjustment, clear save) | ~90% | Plus Middlescroll, Hide Opponent Notes, Ghost Tapping and Psych Engine's time bar and health bar (all off by default). No gamepads. |
| Title screen, main menu | ~90% | Merch opens the store page |
| Freeplay | ~40% | Song list, difficulties, previews and DJ; no album art, letter sort, favourites or saved scores |
| Transitions (fade, sticker transition) | ~70% | |
| Game over screen | ~80% | Death animations, camera, music, retry and Tankman's quotes; Pico's can explosion death (2hot) and the 1-in-4096 fakeout are not ported |
| Results screen | ~5% | Placeholder with the tallies and rank |
| Credits | ~70% | Reads `data/credits.json` if present; the base game builds its credits into the exe, so only the founders are listed otherwise |
| Story mode, character select | 0% | |
| Dialogue, cutscenes, videos | 0% | |
| Sound tray, debug display, screenshots, Discord presence | ~80% | |

## Known issues

- **Scripted behaviour is missing:** Nene's knife states, the limo dancers,
  Philly's train and window lights, Week 7's Tankmen and similar stage effects.
  Dialogue (Week 6), cutscenes and videos are not played.
- **Not ported:** note splashes, gamepad support, story mode, character select,
  the real results screen, freeplay album art, letter sort,
  favourites and saved scores, `packer` atlases, note-kind scripts and GF's
  combo-drop animations.
- **Recently added and only lightly tested:** note styles (Week 6's pixel
  arrows) and the strumline/note/hold placement, the sticker transition when leaving
  a song, health icons, the rating and combo popups, the fix for notes freezing
  at the start of a second song, Daddy Dearest / Mommy Mearest idle
  animations, the Weekend 1 rain, the credits and the game over screens.
- The pause music keeps playing when the window loses focus.
- Animated (XML) health icons and mid-song icon changes are not supported.
- The score text uses the VCR TrueType font. The newest upstream code uses a
  bitmap version that the 0.8.6 assets don't include.
- Discord Rich Presence needs your own Discord application id (see below).
- Windows only.

## Download and run

1. Download `RustedMic.exe` from the latest release and put it in its own folder.
2. Run it. The setup screen scans for your Friday Night Funkin' installation, or
   lets you browse to it. It packs the assets into `assets.pak` (about 800 MB)
   next to the exe. This happens once.
3. Play. Controls: WASD or arrow keys by default (rebind them under Options →
   Controls).

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
`target\x86_64-pc-windows-msvc\release\RustedMic.exe` runs without the VC++ redistributable.

## Using it

```powershell
RustedMic                             # open the game (the setup screen comes first if needed)
RustedMic play bopeebo hard           # jump straight into a song (add --botplay to watch)
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
and **health bar** (in the characters' colours). All of these are off by default. Everything is saved to `%LOCALAPPDATA%\Rusted Mic\settings.json`.

Default hotkeys (all rebindable): F11 fullscreen, F3 screenshot (saved to
`screenshots\` next to the exe; click the preview to open the folder), `-`/`=`/`0`
volume down/up/mute, F6 cycles the debug display, P/Enter/Esc pause, R resets.

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
