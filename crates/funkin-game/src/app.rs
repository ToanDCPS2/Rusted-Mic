//! Window, event loop and screen flow, driven by `funkin_core::state::GameState`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use funkin_core::scoring::{calculate_rank, tally_completion, ScoringRank, Tallies};
use funkin_core::song::SongRegistry;
use funkin_core::state::{GameState, PlayRequest, SongResult, Transition};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Fullscreen, Window, WindowId};

use crate::assets::Assets;
use crate::audio::Audio;
use crate::discord::{unix_now, Discord, Presence};
use crate::gfx::ttf::TtfFont;
use crate::gfx::{Gfx, WHITE};
use crate::menus::char_select::CharSelect;
use crate::save::Save;
use crate::menus::freeplay::{list_songs, Freeplay, FreeplaySong};
use crate::menus::players::{load_style, Players, DEFAULT_CHARACTER};
use crate::menus::main_menu::MainMenu;
use crate::menus::credits::Credits;
use crate::menus::story::StoryMenu;
use crate::menus::options::Options;
use crate::menus::title::Title;
use crate::menus::{Fonts, MenuAction, Music};
use crate::overlay::{DebugOverlay, Screenshots, SoundTray};
use crate::pause::{PauseAction, PauseMenu};
use crate::play::{lane_direction, NoteSkin, PlayOutcome, PlayPrefs, PlayScene};
use crate::settings::{Control, DebugDisplay, Settings};
use crate::stickers::{resolve_pack, Stickers};

/// What happens under the stickers once they cover the screen.
#[derive(Clone, Copy)]
enum StickerExit {
    /// Pause menu "Exit to Menu".
    Pause,
    /// Leaving the game over screen.
    GameOver,
    /// Leaving the results screen.
    Results,
}

/// `FlxTransitionableState` default transitions (`TransitionData(FADE, BLACK, ...)`).
#[derive(Clone)]
enum Fade {
    /// transIn: 1 s, the black sweeps up to reveal the new state.
    In { start: f32 },
    /// transOut: 0.7 s, the black sweeps down to cover, then `then` runs.
    Out { start: f32, then: MenuAction },
}

const FADE_IN_SECS: f32 = 1.0;
const FADE_OUT_SECS: f32 = 0.7;

struct Loaded {
    /// 1×256 black, alpha 0 → 1 top to bottom (the transition's gradient edge).
    gradient: crate::gfx::TexId,
    gfx: Gfx,
    fonts: Fonts,
    /// The default (`funkin`) note style, also used by the options menu.
    skin: NoteSkin,
    /// Other note styles, loaded the first time a song uses them.
    skins: std::collections::HashMap<String, NoteSkin>,
    tray: SoundTray,
    shots: Screenshots,
    debug: DebugOverlay,
}

pub struct App {
    assets: Assets,
    registry: SongRegistry,
    audio: Audio,
    state: GameState,
    songs: Arc<Vec<FreeplaySong>>,
    /// The character `songs` was listed for.
    songs_char: String,
    players: Players,
    /// The character freeplay shows (`rememberedCharacterId`).
    freeplay_char: String,
    /// Character select, open instead of freeplay.
    char_select: Option<CharSelect>,
    /// Progress (`Save.hx`): characters seen, the char select intro.
    save: Save,
    /// Dev-only `RUSTED_MIC_DEV`: `freeplay` or `charselect` opens that menu at
    /// launch; `tab@<seconds>` presses freeplay's character select key then, and
    /// `back@<seconds>` leaves character select; `down@<seconds>` moves freeplay's selection.
    /// `story` opens story mode, where `down`, `right` and `enter` work; `botplay`
    /// toggles botplay and `enter` also leaves the results screen.
    dev: Vec<(String, f32)>,
    music: Music,
    title: Option<Title>,
    main_menu: Option<MainMenu>,
    freeplay: Option<Freeplay>,
    /// The options menu, open over the main menu state.
    options: Option<Options>,
    /// The credits scroll, open over the main menu state.
    credits: Option<Credits>,
    /// The mod list (Tab on the main menu), over the main menu.
    mods: Option<crate::menus::mods::ModMenu>,
    /// Freeplay's selection, kept across songs.
    remembered: Option<(String, String)>,
    /// The story menu (`StoryMenuState`).
    story: Option<StoryMenu>,
    /// Its selected level and difficulty, kept across songs.
    story_remembered: Option<(String, String)>,
    /// The week being played (`PlayStatePlaylist.campaignScore` and `Highscore.talliesLevel`).
    campaign: Option<(i64, Tallies)>,
    play: Option<PlayScene>,
    /// The results screen (`ResultState`) while in [`GameState::Results`].
    results: Option<crate::results::Results>,
    pause: Option<PauseMenu>,
    /// Deaths on the current song (`PlayState.deathCounter`).
    deaths: u32,
    loaded: Option<Loaded>,
    error: Option<String>,
    start: Option<PlayRequest>,
    /// `--start <seconds>` for the first song, in ms.
    skip_ms: Option<f64>,
    botplay: bool,
    last_frame: Option<Instant>,
    /// When the next frame is due (framerate cap).
    next_frame: Option<Instant>,
    clock: f32,
    /// The title screen's Y-key window wiggle: when it started and from where.
    wiggle: Option<(f32, winit::dpi::PhysicalPosition<i32>)>,
    pads: crate::pad::Pads,
    /// The next `start_song` restarts the song being played.
    restarting: bool,
    /// ...from the pause menu (not a death), so the old notes vwoosh away.
    restart_vwoosh: bool,
    /// The restarted song, drawing its notes leaving (`vwooshNotes`).
    ghost: Option<PlayScene>,
    quit: bool,
    fade: Option<Fade>,
    settings: Settings,
    discord: Discord,
    /// When the current song started (Discord's elapsed time).
    play_started: u64,
    /// The window lost focus with "Pause on Unfocus" on.
    unfocused: bool,
    /// Unpause the audio when focus comes back.
    resume_on_focus: bool,
    shift: bool,
    /// `-` and `=` held: pressing both together toggles botplay.
    combo_keys: [bool; 2],
    /// The volume before the first key of the botplay shortcut changed it.
    volume_before_combo: Option<(i32, bool)>,
    cursor: (f32, f32),
    /// The sticker transition out of a song (`StickerSubState`).
    stickers: Option<(Stickers, StickerExit)>,
    /// The current song's sticker pack id.
    sticker_pack: String,
    /// First-run setup (no assets yet), drawing on its own `Gfx` until done.
    #[cfg(feature = "importer")]
    setup: Option<(crate::setup_screen::SetupScreen, Option<Gfx>)>,
    /// Why setup is needed (shown if it's not simply a first run).
    #[cfg(feature = "importer")]
    setup_reason: Option<String>,
}

/// Open the game window. With no `asset_root`, the first-run setup screen
/// comes first and the game starts once the assets are packed.
/// Launch a fresh copy of the game (same assets folder) for a reboot.
fn restart_process() -> bool {
    let Ok(exe) = std::env::current_exe() else { return false };
    // Keep only what picks the assets; a `play` command or test flags would
    // otherwise start a song again.
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut keep = vec![];
    if let Some(i) = args.iter().position(|a| a == "--assets") {
        keep.extend(args[i..].iter().take(2).cloned());
    }
    // The dev launch script would run again (and restart again).
    match std::process::Command::new(exe).args(keep).env_remove("RUSTED_MIC_DEV").spawn() {
        Ok(_) => true,
        Err(e) => {
            log::warn!("restart: {e}");
            false
        }
    }
}

pub fn run(
    asset_root: Option<std::path::PathBuf>,
    setup_reason: Option<String>,
    start: Option<PlayRequest>,
    botplay: bool,
    skip_ms: f64,
) -> Result<(), String> {
    #[cfg(not(feature = "importer"))]
    let _ = &setup_reason;
    let needs_setup = asset_root.is_none();
    let asset_root = asset_root.unwrap_or_default();
    let registry = SongRegistry::new(&asset_root);
    let assets = Assets::new(asset_root);
    let players = Players::load(&assets);
    let songs = Arc::new(if needs_setup { Vec::new() } else { list_songs(&registry, &players, DEFAULT_CHARACTER) });
    let settings = Settings::load();
    let audio = Audio::new()?;
    audio.set_master(settings.master_gain());
    let discord = Discord::new(&settings.discord_app_id);
    let mut app = App {
        #[cfg(feature = "importer")]
        setup: None,
        #[cfg(feature = "importer")]
        setup_reason: needs_setup.then(|| setup_reason.unwrap_or_default()),
        assets,
        registry,
        audio,
        state: GameState::Title,
        songs,
        songs_char: DEFAULT_CHARACTER.into(),
        players,
        freeplay_char: DEFAULT_CHARACTER.into(),
        char_select: None,
        save: Save::load(),
        dev: std::env::var("RUSTED_MIC_DEV")
            .map(|v| {
                v.split(',')
                    .filter(|c| !c.is_empty())
                    .map(|c| match c.split_once('@') {
                        Some((k, t)) => (k.to_owned(), t.parse().unwrap_or(0.0)),
                        None => (c.to_owned(), 0.0),
                    })
                    .collect()
            })
            .unwrap_or_default(),
        music: Music::default(),
        title: None,
        main_menu: None,
        freeplay: None,
        options: None,
        credits: None,
        mods: None,
        remembered: None,
        story: None,
        story_remembered: None,
        campaign: None,
        play: None,
        results: None,
        pause: None,
        deaths: 0,
        loaded: None,
        error: None,
        start,
        skip_ms: (skip_ms > 0.0).then_some(skip_ms),
        botplay,
        last_frame: None,
        next_frame: None,
        clock: 0.0,
        wiggle: None,
        pads: crate::pad::Pads::new(),
        restarting: false,
        restart_vwoosh: false,
        ghost: None,
        quit: false,
        fade: None,
        settings,
        discord,
        play_started: 0,
        unfocused: false,
        resume_on_focus: false,
        shift: false,
        combo_keys: [false, false],
        volume_before_combo: None,
        cursor: (0.0, 0.0),
        stickers: None,
        sticker_pack: "default".into(),
    };
    let event_loop = EventLoop::new().map_err(|e| e.to_string())?;
    event_loop.set_control_flow(ControlFlow::Poll);
    event_loop.run_app(&mut app).map_err(|e| e.to_string())?;
    app.discord.set(None);
    match app.error {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// Everything the game needs on top of the window: fonts, note skin, ...
fn load_game(mut gfx: Gfx, assets: &Assets) -> Result<Loaded, String> {
    let fonts = Fonts {
        bold: assets.font(&mut gfx, "bold", true)?,
        default: assets.font(&mut gfx, "default", false)?,
        clear: assets.font(&mut gfx, "freeplay-clear", false)?,
        vcr: assets.ttf_path("VCR_OSD_Mono.ttf").and_then(|p| TtfFont::load(&p).map_err(|e| log::warn!("{e}")).ok()),
        five_by_seven: assets.ttf_path("5by7.ttf").and_then(|p| TtfFont::load(&p).map_err(|e| log::warn!("{e}")).ok()),
        five_by_seven_bold: assets.ttf_path("5by7_Bold.ttf").and_then(|p| TtfFont::load(&p).map_err(|e| log::warn!("{e}")).ok()),
    };
    let skin = NoteSkin::load(&mut gfx, assets, "funkin")?;
    let ramp: Vec<u8> = (0..256u32).flat_map(|i| [0, 0, 0, i as u8]).collect();
    let gradient = gfx.upload_rgba(1, 256, &ramp);
    let tray = SoundTray::new(&mut gfx, assets);
    let shots = Screenshots::new(assets);
    Ok(Loaded { gradient, gfx, fonts, skin, skins: Default::default(), tray, shots, debug: DebugOverlay::new() })
}

impl App {
    /// Assets are ready (at startup, or after setup): load the game and show the title.
    fn enter_game(&mut self, gfx: Gfx, event_loop: &ActiveEventLoop) {
        match load_game(gfx, &self.assets) {
            Ok(mut l) => {
                l.gfx.set_vsync(self.settings.vsync);
                l.gfx.antialiasing = self.settings.antialiasing;
                if self.settings.auto_fullscreen {
                    l.gfx.window().set_fullscreen(Some(Fullscreen::Borderless(None)));
                }
                self.loaded = Some(l);
                if let Some(mut req) = self.start.take() {
                    // `play <song> --story`: carry on through the rest of the song's week.
                    if req.is_story && req.playlist.is_empty() && req.level_id.is_none() {
                        if let Some(level) = self.registry.load_levels().into_iter().find(|l| l.songs.contains(&req.song_id)) {
                            let at = level.songs.iter().position(|s| *s == req.song_id).unwrap();
                            req.playlist = level.songs[at + 1..].to_vec();
                            req.level_id = Some(level.id);
                        }
                    }
                    if let Ok(spec) = std::env::var("RUSTED_MIC_DEBUG_RESULTS") {
                        self.debug_results(req, &spec);
                    } else {
                        self.start_song(req, false);
                    }
                } else {
                    self.open_title(false);
                }
            }
            Err(e) => {
                self.error = Some(e);
                event_loop.exit();
            }
        }
    }

    /// Run the setup screen for one frame. Returns true while setup is active.
    #[cfg(feature = "importer")]
    fn setup_frame(&mut self, event_loop: &ActiveEventLoop) -> bool {
        let Some((screen, gfx)) = self.setup.as_mut() else { return false };
        let now = Instant::now();
        let dt = self.last_frame.map(|t| (now - t).as_secs_f32()).unwrap_or(0.0).min(0.1);
        self.last_frame = Some(now);
        let action = screen.update(dt);
        if let Some(g) = gfx.as_mut() {
            screen.draw(g);
            g.present();
        }
        self.setup_action(action, event_loop);
        true
    }

    #[cfg(feature = "importer")]
    fn setup_action(&mut self, action: crate::setup_screen::SetupAction, event_loop: &ActiveEventLoop) {
        use crate::setup_screen::SetupAction;
        match action {
            SetupAction::None => {}
            SetupAction::Quit => event_loop.exit(),
            SetupAction::Ready(root) => {
                let Some((_, Some(gfx))) = self.setup.take() else { return };
                self.registry = SongRegistry::new(&root);
                self.assets = Assets::new(root);
                self.players = Players::load(&self.assets);
                self.songs = Arc::new(list_songs(&self.registry, &self.players, DEFAULT_CHARACTER));
                self.songs_char = DEFAULT_CHARACTER.into();
                self.last_frame = None;
                self.enter_game(gfx, event_loop);
            }
        }
    }

    /// Settings changed in the options menu (or by a hotkey): save and apply.
    fn settings_changed(&mut self) {
        self.settings.save();
        self.audio.set_master(self.settings.master_gain());
        if let Some(l) = self.loaded.as_mut() {
            l.gfx.set_vsync(self.settings.vsync);
            l.gfx.antialiasing = self.settings.antialiasing;
        }
        if let Some(m) = &mut self.main_menu {
            m.flashing_lights = self.settings.flashing_lights;
        }
        if !self.settings.discord_rpc {
            self.discord.set(None);
        }
    }

    fn play_prefs(&self) -> PlayPrefs {
        let s = &self.settings;
        PlayPrefs {
            downscroll: s.downscroll,
            middlescroll: s.middlescroll,
            hide_opponent: s.hide_opponent_notes,
            ghost_tapping: s.ghost_tapping,
            psych_time_bar: s.psych_time_bar,
            psych_health_bar: s.psych_health_bar,
            strumline_bg: s.strumline_background as f32 / 100.0,
            note_splashes: s.note_splashes,
            zoom_camera: s.zoom_camera,
            subtitles: s.subtitles,
            naughty: s.naughtyness,
            skip_ms: 0.0,
            advance_key: s.controls.keys(Control::CutsceneAdvance).first().map_or("?", |&k| crate::settings::key_label(k)),
            restart: false,
        }
    }

    fn start_song(&mut self, req: PlayRequest, practice: bool) {
        let mut prefs = self.play_prefs();
        let offset = self.settings.global_offset as f64;
        if self.loaded.is_none() {
            return;
        }
        // `--start`: only the first song launched skips ahead.
        prefs.skip_ms = self.skip_ms.take().unwrap_or(0.0);
        prefs.restart = std::mem::take(&mut self.restarting);
        let vwoosh = std::mem::take(&mut self.restart_vwoosh) && prefs.restart;
        self.music.stop(&self.audio);
        self.close_pause();
        self.audio.set_paused(false);
        // Restarting the same song keeps the death counter.
        if self.play.as_ref().map(|p| &p.request.song_id) != Some(&req.song_id) && !matches!(self.state, GameState::GameOver(_)) {
            self.deaths = 0;
        }
        let l = self.loaded.as_mut().unwrap();
        self.ghost = None;
        if let Some(mut old) = self.play.take() {
            // The notes are drawn from the note skins, which stay loaded.
            l.gfx.release_textures(old.tex_mark);
            if vwoosh {
                old.into_ghost();
                self.ghost = Some(old);
            }
        }
        let scene = PlayScene::new(&mut l.gfx, &self.assets, &self.registry, &self.audio, req.clone(), offset, self.botplay, prefs, practice);
        // Whatever loads after this (glyphs, previews) outlives the song.
        let scope = l.gfx.end_scope();
        match scene {
            Ok(scene) => {
                l.gfx.window().set_title(&format!("Rusted Mic - {}", scene.title));
                self.sticker_pack = resolve_pack(&self.assets, scene.sticker_pack.as_deref(), &scene.player);
                // Load the song's note style once (outside the song's texture scope, so it stays).
                let style = scene.note_style.clone();
                if style != l.skin.id && !l.skins.contains_key(&style) {
                    match NoteSkin::load(&mut l.gfx, &self.assets, &style) {
                        Ok(s) => {
                            l.skins.insert(style, s);
                        }
                        Err(e) => log::warn!("note style {style}: {e}"),
                    }
                }
                self.play = Some(scene);
                self.freeplay = None;
                self.story = None;
                self.play_started = unix_now();
                if !matches!(self.state, GameState::Play(_)) {
                    if !matches!(self.state, GameState::Freeplay | GameState::StoryMenu) {
                        self.state = GameState::Freeplay;
                    }
                    self.state.apply(Transition::StartSong(req));
                } else {
                    self.state = GameState::Play(req);
                }
            }
            Err(e) => {
                l.gfx.release_textures(scope);
                log::error!("could not start {}: {e}", req.song_id);
            }
        }
    }

    fn leave_play(&mut self, t: Transition) {
        self.close_pause();
        self.audio.set_paused(false);
        self.audio.stop();
        if let (Some(scene), Some(l)) = (self.play.take(), self.loaded.as_mut()) {
            l.gfx.release_textures(scene.tex_mark);
        }
        self.state.apply(t);
        if let Some(l) = &self.loaded {
            l.gfx.window().set_title("Rusted Mic");
        }
    }

    /// Leaving the pause menu: the song plays on, unless a script's cutscene holds it
    /// (a cancelled countdown keeps the song's audio paused).
    fn unpause_song(&self) {
        let held = self.play.as_ref().is_some_and(|p| p.song_held());
        self.audio.set_paused(held);
    }

    fn close_pause(&mut self) {
        if let Some(mut p) = self.pause.take() {
            p.close(&self.audio);
            self.settings.save();
        }
    }

    fn open_pause(&mut self) {
        if self.pause.is_some() {
            return;
        }
        let Some(scene) = &self.play else { return };
        if !scene.may_pause() {
            return;
        }
        self.audio.set_paused(true);
        let cutscene = scene.video_playing();
        if let Some(scene) = self.play.as_mut() {
            scene.pause_video(&self.audio, true);
        }
        let Some(scene) = &self.play else { return };
        self.pause = Some(PauseMenu::new(
            &self.audio,
            &self.assets,
            &scene.song_name,
            &scene.artist,
            scene.charter.as_deref(),
            &scene.request.difficulty,
            &scene.request.variation,
            scene.difficulties.clone(),
            self.deaths,
            scene.practice(),
            self.settings.global_offset,
            cutscene,
        ));
    }

    fn open_title(&mut self, skip_intro: bool) {
        let Some(l) = self.loaded.as_mut() else { return };
        match Title::new(&mut l.gfx, &self.assets) {
            Ok(mut t) => {
                if skip_intro {
                    t.skip_intro_now();
                }
                if !t.delayed() {
                    t.start_music(&self.audio, &self.assets, &mut self.music);
                }
                self.title = Some(t);
            }
            Err(e) => {
                log::error!("title: {e}");
                self.open_main_menu();
            }
        }
        self.state = GameState::Title;
    }

    fn open_main_menu(&mut self) {
        let Some(l) = self.loaded.as_mut() else { return };
        let selected = self.main_menu.as_ref().map(|m| m.selected).unwrap_or(0);
        match MainMenu::new(&mut l.gfx, &self.assets, selected) {
            Ok(mut m) => {
                m.flashing_lights = self.settings.flashing_lights;
                self.main_menu = Some(m);
            }
            Err(e) => log::error!("main menu: {e}"),
        }
        self.title = None;
        self.state = GameState::MainMenu;
        if !self.music.playing() && self.music.play_menu(&self.audio, &self.assets, "freakyMenu", false) {
            self.music.fade(0.0, 1.0, 4.0);
        }
    }

    /// Open freeplay (over the main menu).
    fn open_freeplay(&mut self) {
        self.open_freeplay_from(false);
    }

    /// Open freeplay; `from_char_select` slides it in from character select.
    fn open_freeplay_from(&mut self, from_char_select: bool) {
        if self.players.get(&self.freeplay_char).is_none() {
            self.freeplay_char = DEFAULT_CHARACTER.into();
        }
        if self.songs_char != self.freeplay_char {
            self.songs = Arc::new(list_songs(&self.registry, &self.players, &self.freeplay_char));
            self.songs_char = self.freeplay_char.clone();
            self.remembered = None;
        }
        let player = self.players.get(&self.freeplay_char).cloned().unwrap_or_default();
        let style = load_style(&self.assets, player.freeplay_style.as_deref().unwrap_or(DEFAULT_CHARACTER));
        let can_char_select = self.players.unlocked_count() > 1;
        let Some(l) = self.loaded.as_mut() else { return };
        if self.main_menu.is_none() {
            if let Ok(mut m) = MainMenu::new(&mut l.gfx, &self.assets, 1) {
                m.covered = true;
                m.flashing_lights = self.settings.flashing_lights;
                self.main_menu = Some(m);
            }
        }
        let remembered = self.remembered.as_ref().map(|(s, d)| (s.as_str(), d.as_str()));
        match Freeplay::new(&mut l.gfx, &self.assets, self.songs.clone(), remembered, &player, &style, can_char_select, from_char_select) {
            Ok(mut f) => {
                f.flashing_lights = self.settings.flashing_lights;
                f.scores = self.freeplay_scores();
                self.freeplay = Some(f);
                self.state = GameState::Freeplay;
            }
            Err(e) => {
                log::error!("freeplay: {e}");
                if let Some(m) = &mut self.main_menu {
                    m.uncover();
                }
            }
        }
    }

    /// Steps the `RUSTED_MIC_DEV` launch script.
    fn run_dev(&mut self) {
        if self.dev.is_empty() || self.loaded.is_none() || self.fade.is_some() {
            return;
        }
        let (cmd, at) = self.dev[0].clone();
        if self.clock < at {
            return;
        }
        self.dev.remove(0);
        if let Some(m) = &mut self.mods {
            let code = match cmd.as_str() {
                "right" => Some(KeyCode::ArrowRight),
                "left" => Some(KeyCode::ArrowLeft),
                "down" => Some(KeyCode::ArrowDown),
                "enter" => Some(KeyCode::Enter),
                "back" => Some(KeyCode::Escape),
                "space" => Some(KeyCode::Space),
                "earlier" => Some(KeyCode::BracketLeft),
                "later" => Some(KeyCode::BracketRight),
                "tab" => Some(KeyCode::Tab),
                _ => None,
            };
            if let Some(c) = code {
                let a = m.key(c, Some(c), &self.audio);
                self.act(a);
                return;
            }
        }
        match cmd.as_str() {
            // `press:NAME` / `release:NAME`: a key (save id, e.g. `LEFT`) or gamepad input (`A`, `DPAD_UP`)
            // through the normal input path. Pad ids win where they share a name.
            "fakepad" => self.pads.fake = true,
            c if c.starts_with("press:") || c.starts_with("release:") => {
                let (kind, name) = c.split_once(':').unwrap_or_default();
                let pad = name.strip_prefix("pad:");
                let code = match pad {
                    Some(n) => crate::pad::pad_from_id(n),
                    None => crate::settings::key_from_id(name),
                };
                match code {
                    Some(code) => self.on_input(code, kind == "press", false, Instant::now()),
                    None => log::warn!("RUSTED_MIC_DEV: unknown key '{name}'"),
                }
            }
            // A tap of Space for scripts (`FlxG.keys`), e.g. QT's dodge.
            "space" => {
                if let Some(s) = self.play.as_mut() {
                    s.script_key(32, true);
                    s.script_key(32, false);
                }
            }
            "freeplay" => {
                self.title = None;
                self.open_freeplay();
            }
            "mods" => {
                self.title = None;
                self.open_main_menu();
                self.act(MenuAction::OpenMods);
            }
            "credits" => {
                self.title = None;
                self.open_main_menu();
                if let Some(m) = &mut self.main_menu {
                    m.covered = true;
                }
                self.act(MenuAction::OpenCreditsNow);
            }
            "story" => {
                self.title = None;
                self.open_main_menu();
                if let Some(m) = &mut self.main_menu {
                    m.covered = true;
                }
                self.act(MenuAction::OpenStoryNow);
            }
            "right" | "enter" if self.story.is_some() => {
                let code = if cmd == "right" { KeyCode::ArrowRight } else { KeyCode::Enter };
                if let (Some(s), Some(l)) = (&mut self.story, self.loaded.as_mut()) {
                    let a = s.key(&mut l.gfx, code, true, &self.audio);
                    s.key(&mut l.gfx, code, false, &self.audio);
                    self.act(a);
                }
            }
            "botplay" => self.toggle_botplay(),
            "enter" if matches!(self.state, GameState::Results(_)) => self.results_accept(),
            "enter" if self.play.as_ref().is_some_and(|p| p.in_conversation()) => {
                if let Some(p) = &mut self.play {
                    p.advance_conversation();
                }
            }
            "charselect" => {
                self.title = None;
                self.act(MenuAction::OpenCharSelect(DEFAULT_CHARACTER.into()));
            }
            "tab" => {
                if let Some(f) = &mut self.freeplay {
                    let a = f.key(KeyCode::F16, &self.audio, &mut self.music);
                    self.act(a);
                }
            }
            "down" | "up" if self.story.is_some() => {
                let code = if cmd == "up" { KeyCode::ArrowUp } else { KeyCode::ArrowDown };
                if let (Some(s), Some(l)) = (&mut self.story, self.loaded.as_mut()) {
                    let a = s.key(&mut l.gfx, code, true, &self.audio);
                    s.key(&mut l.gfx, code, false, &self.audio);
                    self.act(a);
                }
            }
            "down" => {
                if let Some(f) = &mut self.freeplay {
                    let a = f.key(KeyCode::ArrowDown, &self.audio, &mut self.music);
                    self.act(a);
                }
            }
            "back" => {
                if let Some(c) = &mut self.char_select {
                    let a = c.key(&self.audio, &mut self.music, Some(KeyCode::Escape), true);
                    self.act(a);
                }
            }
            other => log::warn!("RUSTED_MIC_DEV: unknown step '{other}'"),
        }
    }

    fn close_freeplay(&mut self) {
        if let Some(f) = self.freeplay.take() {
            self.remembered = f.remembered().or(self.remembered.take());
        }
        if let Some(m) = &mut self.main_menu {
            m.uncover();
        }
        self.state = GameState::MainMenu;
        if self.music.play_menu(&self.audio, &self.assets, "freakyMenu", false) {
            self.music.fade(0.0, 1.0, 4.0);
        }
    }

    fn act(&mut self, a: MenuAction) {
        match a {
            MenuAction::None => {}
            MenuAction::ToMainMenu => {
                self.open_main_menu();
                self.fade = Some(Fade::In { start: self.clock });
            }
            MenuAction::ToTitle => {
                if !matches!(self.fade, Some(Fade::Out { .. })) {
                    self.fade = Some(Fade::Out { start: self.clock, then: MenuAction::ToTitleNow });
                }
            }
            MenuAction::ToTitleNow => {
                self.main_menu = None;
                self.open_title(true);
            }
            MenuAction::OpenFreeplay => self.open_freeplay(),
            MenuAction::OpenStory => {
                if !matches!(self.fade, Some(Fade::Out { .. })) {
                    self.fade = Some(Fade::Out { start: self.clock, then: MenuAction::OpenStoryNow });
                }
            }
            MenuAction::OpenStoryNow => {
                self.open_story();
                self.fade = Some(Fade::In { start: self.clock });
            }
            MenuAction::CloseStory => {
                if !matches!(self.fade, Some(Fade::Out { .. })) {
                    self.fade = Some(Fade::Out { start: self.clock, then: MenuAction::ToMainMenu });
                }
            }
            MenuAction::CloseFreeplay => self.close_freeplay(),
            MenuAction::OpenCharSelect(c) => {
                if let Some(f) = self.freeplay.take() {
                    self.remembered = f.remembered().or(self.remembered.take());
                }
                let Some(l) = self.loaded.as_mut() else { return };
                self.char_select = Some(CharSelect::new(&mut l.gfx, &self.assets, &self.audio, &mut self.music, &self.players, &self.save, &c));
            }
            MenuAction::CloseCharSelect(c) => {
                self.char_select = None;
                self.music.stop(&self.audio);
                self.freeplay_char = c;
                self.open_freeplay_from(true);
            }
            MenuAction::OpenOptions => {
                if !matches!(self.fade, Some(Fade::Out { .. })) {
                    self.fade = Some(Fade::Out { start: self.clock, then: MenuAction::OpenOptionsNow });
                }
            }
            MenuAction::OpenOptionsNow => {
                let Some(l) = self.loaded.as_mut() else { return };
                match Options::new(&mut l.gfx, &self.assets, &self.settings) {
                    Ok(o) => self.options = Some(o),
                    Err(e) => {
                        log::error!("options: {e}");
                        if let Some(m) = &mut self.main_menu {
                            m.uncover();
                        }
                    }
                }
                self.fade = Some(Fade::In { start: self.clock });
            }
            MenuAction::OpenCredits => {
                if !matches!(self.fade, Some(Fade::Out { .. })) {
                    self.fade = Some(Fade::Out { start: self.clock, then: MenuAction::OpenCreditsNow });
                }
            }
            MenuAction::OpenCreditsNow => {
                let Some(l) = self.loaded.as_mut() else { return };
                self.credits = Some(Credits::new(&mut l.gfx, &self.assets, l.fonts.vcr.as_ref()));
                Credits::start_music(&self.audio, &self.assets, &mut self.music);
                self.fade = Some(Fade::In { start: self.clock });
            }
            MenuAction::OpenMods => {
                if let (None, Some(l)) = (&self.mods, self.loaded.as_mut()) {
                    self.mods = Some(crate::menus::mods::ModMenu::new(&mut l.gfx, &self.assets));
                }
            }
            MenuAction::CloseMods => {
                if let (Some(m), Some(l)) = (self.mods.take(), self.loaded.as_mut()) {
                    l.gfx.release_textures(m.tex_mark);
                    if m.needs_restart() {
                        // A different mod set: start the game over so every
                        // asset, script and registry loads with it.
                        let mods = crate::menus::mods::enabled_mod_dirs();
                        log::info!("mods now: {:?}; restarting", mods.iter().filter_map(|m| m.file_name()).collect::<Vec<_>>());
                        if restart_process() {
                            self.quit = true;
                        } else {
                            // Couldn't relaunch: re-layer in place (menus and songs opened from now on see it).
                            funkin_core::vfs::set_mods(self.assets.root(), mods);
                        }
                    }
                }
            }
            MenuAction::CloseCredits => {
                if !matches!(self.fade, Some(Fade::Out { .. })) {
                    self.fade = Some(Fade::Out { start: self.clock, then: MenuAction::ToMainMenu });
                }
            }
            MenuAction::CloseOptions => {
                if !matches!(self.fade, Some(Fade::Out { .. })) {
                    self.fade = Some(Fade::Out { start: self.clock, then: MenuAction::ToMainMenu });
                }
            }
            MenuAction::Restart => {
                // Save data cleared: back to the very start (`InitState`).
                self.settings_changed();
                self.options = None;
                self.main_menu = None;
                self.remembered = None;
                self.music.stop(&self.audio);
                self.open_title(false);
            }
            MenuAction::Play(req) => {
                if let Some(f) = &self.freeplay {
                    self.remembered = f.remembered();
                }
                if let Some(s) = &self.story {
                    self.story_remembered = Some(s.remembered());
                }
                // A new week: `campaignScore = 0`, `talliesLevel = new Tallies()`.
                self.campaign = req.is_story.then(|| (0, Tallies::default()));
                self.start_song(req, false);
            }
            MenuAction::Quit => self.quit = true,
        }
    }

    /// Open the results screen for a finished song or week.
    fn open_results(&mut self, r: funkin_core::state::SongResult, (title, player, valid): (String, String, bool)) {
        let Some(l) = self.loaded.as_mut() else { return };
        let req = &r.request;
        let rank = funkin_core::scoring::calculate_rank(&r.tallies);
        let (title, prev_rank, new_highscore) = if req.is_story {
            let level = req.level_id.as_deref().unwrap_or("");
            let name = self.registry.load_levels().into_iter().find(|lv| lv.id == level).map(|lv| lv.name).filter(|n| !n.is_empty()).unwrap_or(title);
            (name, None, false)
        } else {
            let key = format!("{}:{}:{}", req.song_id, req.difficulty, req.variation);
            let prev = self.save.song_scores.get(&key).cloned();
            let new_high = valid && prev.as_ref().is_none_or(|p| r.score > p.score);
            if valid {
                let e = self.save.song_scores.entry(key).or_default();
                if r.score > e.score || e.completion == 0.0 {
                    e.completion = funkin_core::scoring::tally_completion(&r.tallies);
                }
                e.score = e.score.max(r.score);
                if rank > e.rank {
                    e.rank = rank;
                }
                self.save.save();
            }
            (title, prev.and_then(|p| p.rank), new_high)
        };
        let params = crate::results::ResultsParams {
            story: req.is_story,
            title,
            character_id: player,
            difficulty: req.difficulty.clone(),
            score: r.score,
            tallies: r.tallies,
            new_highscore,
            prev_rank,
            score_valid: valid,
        };
        self.music.stop(&self.audio);
        self.results = Some(crate::results::Results::new(&mut l.gfx, &self.assets, &self.audio, params, self.settings.naughtyness));
    }

    /// Dev: `RUSTED_MIC_DEBUG_RESULTS=rank[,character]` opens the results
    /// screen for the requested song with made-up tallies for that rank.
    fn debug_results(&mut self, req: PlayRequest, spec: &str) {
        let mut parts = spec.split(',');
        let rank = parts.next().unwrap_or("").trim().to_ascii_lowercase();
        let player = parts.next().map(|c| c.trim().to_owned()).unwrap_or_else(|| "bf".into());
        // [sick, good, bad, shit, missed] out of 100 notes.
        let [sick, good, bad, shit, missed] = match rank.as_str() {
            "gold" | "perfect_gold" => [100, 0, 0, 0, 0],
            "perfect" => [90, 10, 0, 0, 0],
            "excellent" => [80, 12, 8, 0, 0],
            "great" => [70, 15, 15, 0, 0],
            "good" => [50, 20, 25, 0, 5],
            _ => [20, 10, 30, 10, 30],
        };
        let tallies = Tallies { sick, good, bad, shit, missed, combo: 0, max_combo: sick + good, total_notes_hit: 100 - missed, total_notes: 100 };
        let score = sick as i64 * 500 + good as i64 * 200 + bad as i64 * 100 + shit as i64 * 50 - missed as i64 * 10;
        let result = SongResult { request: req, score, tallies, rank: calculate_rank(&tallies) };
        self.state = GameState::Results(result.clone());
        self.open_results(result, ("Debug Song by Nobody".into(), player, true));
    }

    /// Enter on the results screen.
    fn results_accept(&mut self) {
        match self.results.as_mut().map(|r| r.accept(&mut self.music)) {
            Some(Some(crate::results::ResultsExit::Stickers)) | None => self.exit_with_stickers(StickerExit::Results),
            _ => {}
        }
    }

    /// Drop the results screen and its textures.
    fn close_results(&mut self) {
        if let (Some(r), Some(l)) = (self.results.take(), self.loaded.as_mut()) {
            l.gfx.release_textures(r.tex_mark);
        }
        self.music.stop(&self.audio);
    }

    /// Leave a song (or its game over / results) under the sticker transition.
    fn exit_with_stickers(&mut self, exit: StickerExit) {
        if self.stickers.is_some() {
            return;
        }
        let started = self.loaded.as_mut().and_then(|l| Stickers::new(&mut l.gfx, &self.assets, &self.sticker_pack));
        match started {
            Some(s) => self.stickers = Some((s, exit)),
            None => self.sticker_switch(exit),
        }
    }

    /// The stickers cover the screen: switch to the menu underneath.
    fn sticker_switch(&mut self, exit: StickerExit) {
        self.deaths = 0;
        match exit {
            StickerExit::Pause => self.leave_play(Transition::Back),
            StickerExit::GameOver => self.leave_play(Transition::Back),
            StickerExit::Results => {
                self.close_results();
                self.state.apply(Transition::Confirm);
            }
        }
        self.ensure_freeplay();
    }

    /// Back in freeplay (or the story menu) after a song.
    /// Saved bests for the freeplay score panel: `song:difficulty:variation` → (score, completion).
    fn freeplay_scores(&self) -> std::collections::HashMap<String, (i64, f64)> {
        self.save.song_scores.iter().map(|(k, v)| (k.clone(), (v.score, v.completion))).collect()
    }

    fn ensure_freeplay(&mut self) {
        let scores = self.freeplay_scores();
        if let Some(f) = &mut self.freeplay {
            f.scores = scores;
        }
        if matches!(self.state, GameState::Freeplay) && self.freeplay.is_none() {
            self.open_freeplay();
        }
        if matches!(self.state, GameState::StoryMenu) && self.story.is_none() {
            self.open_story();
        }
    }

    /// Open the story menu (`StoryMenuState`), over the main menu state.
    fn open_story(&mut self) {
        let Some(l) = self.loaded.as_mut() else { return };
        match StoryMenu::new(&mut l.gfx, &self.assets, &self.registry, self.story_remembered.clone(), self.save.level_scores.clone()) {
            Ok(s) => {
                self.story = Some(s);
                self.state = GameState::StoryMenu;
                // playMenuMusic: freakyMenu, carrying on if it's already playing.
                if self.music.id != "freakyMenu" && self.music.play_menu(&self.audio, &self.assets, "freakyMenu", false) {
                    self.music.fade(0.0, 1.0, 4.0);
                }
            }
            Err(e) => {
                log::error!("story menu: {e}");
                self.state = GameState::MainMenu;
                if let Some(m) = &mut self.main_menu {
                    m.uncover();
                }
            }
        }
    }

    /// Keys that work everywhere: fullscreen, screenshot, volume, debug display.
    /// Botplay on or off, for the current song and the ones after it.
    fn toggle_botplay(&mut self) {
        self.botplay = !self.botplay;
        log::info!("botplay {}", if self.botplay { "on" } else { "off" });
        if let Some(scene) = &mut self.play {
            scene.set_botplay(self.botplay);
        }
        // Outside a song there's no "Bot Play Enabled" text, so give a sound.
        if self.play.is_none() {
            let key = if self.botplay { "default:confirmMenu" } else { "default:cancelMenu" };
            if let Some(s) = self.assets.sound(key) {
                self.audio.play_sfx(s, 0.7);
            }
        }
    }

    fn global_key(&mut self, code: KeyCode) -> bool {
        let c = &self.settings.controls;
        let Some(l) = self.loaded.as_mut() else { return false };
        if c.is(Control::WindowFullscreen, code) {
            let w = l.gfx.window();
            w.set_fullscreen(if w.fullscreen().is_some() { None } else { Some(Fullscreen::Borderless(None)) });
            return true;
        }
        if c.is(Control::WindowScreenshot, code) {
            l.shots.request(&mut l.gfx, &self.settings);
            return true;
        }
        let vol = if c.is(Control::VolumeUp, code) {
            Some(1)
        } else if c.is(Control::VolumeDown, code) {
            Some(-1)
        } else if c.is(Control::VolumeMute, code) {
            Some(0)
        } else {
            None
        };
        if let Some(d) = vol {
            if d == 0 {
                self.settings.muted = !self.settings.muted;
            } else {
                self.settings.muted = false;
                self.settings.volume = (self.settings.volume + d).clamp(0, 10);
            }
            l.tray.show(&self.audio, &self.settings, d > 0);
            self.settings_changed();
            return true;
        }
        if c.is(Control::DebugDisplay, code) {
            self.settings.debug_display = match self.settings.debug_display {
                DebugDisplay::Off => DebugDisplay::Simple,
                DebugDisplay::Simple => DebugDisplay::Advanced,
                DebugDisplay::Advanced => DebugDisplay::Off,
            };
            self.settings_changed();
            return true;
        }
        false
    }

    /// `FlxTween.tween(window, {x: x + 300}, 1.4, PINGPONG, startDelay 0.35)` and
    /// `{y: y + 100}, 0.7, PINGPONG`, both quadInOut, until the title screen closes.
    fn update_wiggle(&mut self) {
        let Some((start, base)) = self.wiggle else { return };
        if !matches!(self.state, GameState::Title) {
            self.wiggle = None;
            return;
        }
        let Some(l) = self.loaded.as_ref() else { return };
        fn ping_pong(t: f32, dur: f32) -> f32 {
            let u = (t / dur).max(0.0);
            let leg = u.floor() as i64;
            let f = u - leg as f32;
            let f = if leg % 2 == 0 { f } else { 1.0 - f };
            // quadInOut
            if f <= 0.5 { f * f * 2.0 } else { 1.0 - (f - 1.0) * (f - 1.0) * 2.0 }
        }
        let t = self.clock - start;
        let dx = if t < 0.35 { 0.0 } else { 300.0 * ping_pong(t - 0.35, 1.4) };
        let dy = 100.0 * ping_pong(t, 0.7);
        let p = winit::dpi::PhysicalPosition::new(base.x + dx.round() as i32, base.y + dy.round() as i32);
        l.gfx.window().set_outer_position(p);
    }

    fn on_key(&mut self, ev: KeyEvent, at: Instant) {
        let PhysicalKey::Code(code) = ev.physical_key else {
            return;
        };
        // These codes stand for gamepad inputs (`pad.rs`), not keys.
        if crate::pad::is_pad(code) {
            return;
        }
        self.on_input(code, ev.state == ElementState::Pressed, ev.repeat, at);
    }

    /// Gamepad inputs since the last frame, fed in like keys.
    fn poll_pads(&mut self) {
        for (code, pressed, at) in self.pads.poll() {
            self.on_input(code, pressed, false, at);
        }
    }

    /// A key (or gamepad input) going down or up. `repeat`: the OS key repeat.
    fn on_input(&mut self, code: KeyCode, pressed: bool, repeat: bool, at: Instant) {
        // `-` + `=` together: toggle botplay (the keys are also the volume keys,
        // so the first key's volume step is undone).
        let combo = match code {
            KeyCode::Minus => Some(0),
            KeyCode::Equal => Some(1),
            _ => None,
        };
        if let Some(i) = combo {
            if pressed && !repeat && self.combo_keys[1 - i] {
                self.combo_keys[i] = true;
                if let Some((v, m)) = self.volume_before_combo.take() {
                    self.settings.volume = v;
                    self.settings.muted = m;
                    self.settings_changed();
                }
                self.toggle_botplay();
                return;
            }
            if pressed && !self.combo_keys[i] {
                self.volume_before_combo = Some((self.settings.volume, self.settings.muted));
            }
            self.combo_keys[i] = pressed;
        }
        if matches!(code, KeyCode::ShiftLeft | KeyCode::ShiftRight) {
            self.shift = pressed;
        }
        let menu = self.settings.controls.menu_key(code);
        if let (GameState::Title, Some(t)) = (&self.state, self.title.as_mut()) {
            let c = &self.settings.controls;
            // `FlxG.keys.pressed.ANY`: the keyboard only, volume keys aside.
            let is_volume = crate::pad::is_pad(code) || c.is(Control::VolumeUp, code) || c.is(Control::VolumeDown, code) || c.is(Control::VolumeMute, code);
            t.raw_key(code, pressed, is_volume);
            if !pressed {
                if let Some(m) = menu {
                    t.key_up(m);
                }
            }
            // `FlxG.keys.justPressed.Y`: the window wiggles.
            if pressed && !repeat && code == KeyCode::KeyY {
                if let Some(l) = self.loaded.as_ref() {
                    let w = l.gfx.window();
                    if w.fullscreen().is_none() {
                        if let Ok(p) = w.outer_position() {
                            // Earlier tweens stop where they are; these start from there.
                            self.wiggle = Some((self.clock, p));
                        }
                    }
                }
            }
        }
        if let (GameState::Play(_), Some(scene), Some(k)) = (&self.state, self.play.as_mut(), crate::scripting::flx_key_code(code)) {
            scene.script_key(k, pressed);
        }

        // Releases: gameplay and the options menu care about them.
        if !pressed {
            if let (GameState::Play(_), Some(scene), None) = (&self.state, self.play.as_mut(), &self.pause) {
                if let Some(lane) = self.settings.controls.note_lane(code) {
                    scene.key_up(lane_direction(lane));
                }
            }
            if let Some(p) = &mut self.pause {
                p.key_up(menu, self.shift);
            }
            if let Some(c) = &mut self.credits {
                let is_pause = self.settings.controls.is(Control::Pause, code);
                c.key(code, menu, is_pause, false);
            }
            if let Some(c) = &mut self.char_select {
                c.key(&self.audio, &mut self.music, menu, false);
            }
            if let (Some(s), Some(m), Some(l)) = (&mut self.story, menu, self.loaded.as_mut()) {
                s.key(&mut l.gfx, m, false, &self.audio);
            }
            if let Some(o) = &mut self.options {
                o.key_up(code, menu, &mut self.settings, &self.audio);
                if std::mem::take(&mut o.dirty) {
                    self.settings_changed();
                }
            }
            return;
        }
        if matches!(self.fade, Some(Fade::Out { .. })) {
            return;
        }
        // Global hotkeys (not while the controls menu is listening for a key).
        let capturing = self.options.as_ref().is_some_and(|o| o.capturing());
        if !capturing && !repeat && self.global_key(code) {
            return;
        }
        // Nothing underneath takes input while the stickers go on.
        if self.stickers.as_ref().is_some_and(|(s, _)| s.blocking()) {
            return;
        }

        if let (GameState::Play(_), Some(scene)) = (&self.state, self.play.as_mut()) {
            if let Some(p) = &mut self.pause {
                let is_pause = self.settings.controls.is(Control::Pause, code) && !repeat;
                match p.key(&self.audio, menu, is_pause, self.shift) {
                    PauseAction::None => {}
                    PauseAction::Offset(ms) => {
                        self.settings.global_offset = ms;
                        scene.set_global_offset(ms as f64);
                        self.settings_changed();
                    }
                    PauseAction::Resume => {
                        scene.set_practice(p.practice());
                        scene.pause_video(&self.audio, false);
                        self.close_pause();
                        self.unpause_song();
                    }
                    PauseAction::SkipCutscene => {
                        scene.skip_video();
                        self.close_pause();
                        self.unpause_song();
                    }
                    PauseAction::RestartCutscene => {
                        scene.restart_video();
                        self.close_pause();
                        self.unpause_song();
                    }
                    PauseAction::Restart => {
                        let (req, practice) = (scene.request.clone(), p.practice());
                        self.restarting = true;
                        self.restart_vwoosh = true;
                        self.start_song(req, practice);
                    }
                    PauseAction::ChangeDifficulty(d) => {
                        let mut req = scene.request.clone();
                        req.difficulty = d;
                        let practice = p.practice();
                        self.restarting = true;
                        self.restart_vwoosh = true;
                        self.start_song(req, practice);
                    }
                    PauseAction::Exit => self.exit_with_stickers(StickerExit::Pause),
                }
                return;
            }
            if !repeat && self.settings.controls.is(Control::CutsceneAdvance, code) {
                scene.cutscene_advance();
            }
            if scene.in_conversation() && self.settings.controls.is(Control::Accept, code) {
                if !repeat {
                    scene.advance_conversation();
                }
                return;
            }
            if let Some(lane) = self.settings.controls.note_lane(code) {
                if !repeat {
                    scene.key_down(&self.audio, lane_direction(lane), at);
                }
                return;
            }
            if repeat {
                return;
            }
            if self.settings.controls.is(Control::Pause, code) {
                self.open_pause();
            } else if self.settings.controls.is(Control::Reset, code) {
                scene.reset_death();
            } else if code == KeyCode::Digit9 {
                scene.toggle_old_icon();
            }
            return;
        }

        if let Some(c) = &mut self.credits {
            let is_pause = self.settings.controls.is(Control::Pause, code);
            let a = c.key(code, menu, is_pause, true);
            self.act(a);
            return;
        }
        if let Some(m) = &mut self.mods {
            let a = m.key(code, menu, &self.audio);
            self.act(a);
            return;
        }
        // Tab on the main menu: the mod list.
        if code == KeyCode::Tab && !repeat && matches!(self.state, GameState::MainMenu) && self.fade.is_none() && self.main_menu.as_ref().is_some_and(|m| m.idle()) {
            self.act(MenuAction::OpenMods);
            return;
        }
        if let Some(c) = &mut self.char_select {
            if !repeat {
                let a = c.key(&self.audio, &mut self.music, menu, true);
                self.act(a);
            }
            return;
        }
        if let Some(o) = &mut self.options {
            let a = o.key_down(code, menu, repeat, &mut self.settings, &self.audio, &mut self.music, &self.assets);
            if std::mem::take(&mut o.dirty) {
                self.settings_changed();
            }
            self.act(a);
            return;
        }

        let Some(code) = menu else { return };
        let action = match &self.state {
            GameState::Title => match &mut self.title {
                Some(t) if !repeat => t.key(code, &self.audio),
                _ => MenuAction::None,
            },
            GameState::MainMenu => match &mut self.main_menu {
                Some(m) => m.key(code, &self.audio),
                None => MenuAction::None,
            },
            GameState::Freeplay => match &mut self.freeplay {
                Some(f) => f.key(code, &self.audio, &mut self.music),
                None => MenuAction::None,
            },
            GameState::StoryMenu => match (&mut self.story, self.loaded.as_mut()) {
                (Some(s), Some(l)) if !repeat => s.key(&mut l.gfx, code, true, &self.audio),
                _ => MenuAction::None,
            },
            GameState::GameOver(req) => {
                let req = req.clone();
                let scene = self.play.as_mut().filter(|s| s.has_death());
                match (code, scene) {
                    (KeyCode::Enter, Some(scene)) => scene.death_confirm(&self.audio, &self.assets),
                    (KeyCode::Enter, None) => {
                        self.state.apply(Transition::Retry);
                        self.restarting = true;
                        self.start_song(req, false);
                    }
                    (KeyCode::Escape, Some(scene)) => {
                        if scene.death_can_go_back() {
                            scene.death_stop(&self.audio);
                            self.exit_with_stickers(StickerExit::GameOver);
                        }
                    }
                    (KeyCode::Escape, None) => self.exit_with_stickers(StickerExit::GameOver),
                    _ => {}
                }
                MenuAction::None
            }
            GameState::Results(_) => {
                if matches!(code, KeyCode::Enter | KeyCode::Escape) {
                    self.results_accept();
                }
                MenuAction::None
            }
            _ => MenuAction::None,
        };
        self.act(action);
    }

    /// What Discord shows (`DiscordClient.setPresence`).
    fn update_presence(&mut self) {
        if !self.settings.discord_rpc {
            return;
        }
        let p = match (&self.state, &self.play) {
            (GameState::Play(_), Some(scene)) => Presence {
                details: format!("{} [{}]", scene.song_name, title_case(&scene.request.difficulty)),
                state: Some(if self.pause.is_some() { "Paused".to_owned() } else { scene.artist.clone() }),
                start: self.pause.is_none().then_some(self.play_started),
            },
            (GameState::GameOver(req), _) => Presence { details: "Game Over".into(), state: Some(req.song_id.replace('-', " ")), start: None },
            (GameState::Freeplay, _) => Presence { details: "In the Menus".into(), state: Some("Freeplay".into()), start: None },
            _ if self.options.is_some() => Presence { details: "In the Menus".into(), state: Some("Options".into()), start: None },
            _ if self.credits.is_some() => Presence { details: "In the Menus".into(), state: Some("Credits".into()), start: None },
            _ if self.mods.is_some() => Presence { details: "In the Menus".into(), state: Some("Mods".into()), start: None },
            _ => Presence { details: "In the Menus".into(), state: None, start: None },
        };
        self.discord.set(Some(p));
    }

    fn frame(&mut self) {
        let now = Instant::now();
        let dt = self.last_frame.map(|t| (now - t).as_secs_f32()).unwrap_or(0.0).min(0.1);
        self.last_frame = Some(now);
        // Pause on Unfocus: everything stands still until focus returns.
        let frozen = self.unfocused && self.settings.auto_pause;
        let dt = if frozen { 0.0 } else { dt };
        self.clock += dt;
        self.update_wiggle();
        self.poll_pads();
        if let Some(g) = &mut self.ghost {
            if !g.ghost_tick(dt) || !matches!(self.state, GameState::Play(_)) {
                self.ghost = None;
            }
        }
        self.update_presence();

        // Gameplay update first, so input → judgement → draw happens in one frame.
        if let (GameState::Play(_), Some(scene), None, false) = (&self.state, self.play.as_mut(), &self.pause, frozen) {
            let l = self.loaded.as_mut().unwrap();
            let skin = if scene.note_style == l.skin.id { &l.skin } else { l.skins.get(&scene.note_style).unwrap_or(&l.skin) };
            match scene.update(&mut l.gfx, &self.audio, skin) {
                PlayOutcome::Running => {}
                PlayOutcome::Died => {
                    // PlayState: the song stops and GameOverSubState opens over the stage.
                    self.deaths += 1;
                    self.close_pause();
                    self.audio.set_paused(false);
                    self.audio.stop();
                    self.state.apply(Transition::Died);
                    let naughty = self.settings.naughtyness;
                    let shown = match (self.play.as_mut(), self.loaded.as_mut()) {
                        (Some(scene), Some(l)) => scene.start_death(&mut l.gfx, &self.assets, naughty),
                        _ => false,
                    };
                    if !shown {
                        // No stage to die on: the plain game over screen.
                        if let (Some(scene), Some(l)) = (self.play.take(), self.loaded.as_mut()) {
                            l.gfx.release_textures(scene.tex_mark);
                        }
                    }
                    if let Some(l) = &self.loaded {
                        l.gfx.window().set_title("Rusted Mic");
                    }
                }
                PlayOutcome::Finished => {
                    let s = &scene.session;
                    // ResultsStateParams from PlayState.endSong.
                    let info = (format!("{} by {}", scene.song_name, scene.artist), scene.player.clone(), !(s.config.botplay || scene.practice()));
                    let result = SongResult {
                        request: scene.request.clone(),
                        score: s.score(),
                        tallies: s.tallies,
                        rank: calculate_rank(&s.tallies),
                    };
                    if result.request.is_story {
                        let (score, t) = self.campaign.get_or_insert((0, Tallies::default()));
                        *score += result.score;
                        add_tallies(t, &result.tallies);
                    }
                    self.leave_play(Transition::SongFinished(result));
                    if let GameState::Play(next) = self.state.clone() {
                        // Story playlists continue automatically.
                        self.start_song(next, false);
                    } else if let GameState::Results(r) = &mut self.state {
                        // The week is over: its results cover every song, and the best score is kept.
                        if let (true, Some((score, t))) = (r.request.is_story, self.campaign.take()) {
                            r.score = score;
                            r.tallies = t;
                            r.rank = calculate_rank(&t);
                            let key = format!("{}:{}", r.request.level_id.as_deref().unwrap_or(""), r.request.difficulty);
                            if self.save.level_scores.get(&key).is_none_or(|&best| score > best) {
                                self.save.level_scores.insert(key, score);
                                self.save.save();
                            }
                        }
                    }
                    if let GameState::Results(r) = &self.state {
                        let r = r.clone();
                        self.open_results(r, info);
                    }
                }
            }
        }
        // The results screen.
        if let (GameState::Results(_), Some(r), false, Some(l)) = (&self.state, self.results.as_mut(), frozen, self.loaded.as_mut()) {
            if let Some(crate::results::ResultsExit::RankUp) = r.update(&mut l.gfx, &self.assets, &self.audio, &mut self.music, dt as f64) {
                // Faded to black: straight to freeplay.
                self.close_results();
                self.state.apply(Transition::Confirm);
                self.ensure_freeplay();
            }
        }
        // The game over screen.
        if let (GameState::GameOver(req), Some(scene), false) = (&self.state, self.play.as_mut(), frozen) {
            if let crate::game_over::DeathAction::Retry { pixel } = scene.death_update(dt, &self.audio, &self.assets) {
                let (req, practice) = (req.clone(), scene.practice());
                self.state.apply(Transition::Retry);
                self.restarting = true;
                self.start_song(req, practice);
                if let Some(s) = self.play.as_mut() {
                    s.fade_in(pixel);
                }
            }
        }
        let covering = self.stickers.as_ref().is_some_and(|(s, _)| s.blocking());
        if let (Some(p), false) = (&mut self.pause, covering) {
            if let PauseAction::Offset(ms) = p.update(dt) {
                self.settings.global_offset = ms;
                if let Some(scene) = &mut self.play {
                    scene.set_global_offset(ms as f64);
                }
                // Saved when the pause menu closes, not every frame of a held key.
            }
        }

        // The sticker transition.
        if let Some((s, exit)) = &mut self.stickers {
            let exit = *exit;
            if s.update(dt, &self.audio) {
                self.sticker_switch(exit);
                if let Some((s, _)) = &mut self.stickers {
                    s.switched();
                }
            } else if s.done() {
                self.stickers = None;
            }
        }

        // Transitions.
        match self.fade.clone() {
            Some(Fade::In { start }) if self.clock - start >= FADE_IN_SECS => self.fade = None,
            Some(Fade::Out { start, then }) if self.clock - start >= FADE_OUT_SECS => {
                self.fade = None;
                if matches!(then, MenuAction::ToMainMenu) && (self.options.is_some() || self.credits.is_some() || self.story.is_some()) {
                    // Leaving options, credits or story mode: the main menu comes back interactive.
                    self.options = None;
                    self.credits = None;
                    if let Some(s) = self.story.take() {
                        self.story_remembered = Some(s.remembered());
                        self.state = GameState::MainMenu;
                        if self.main_menu.is_none() {
                            self.open_main_menu();
                        }
                    }
                    if let Some(m) = &mut self.main_menu {
                        m.uncover();
                    }
                    self.fade = Some(Fade::In { start: self.clock });
                    if self.music.id != "freakyMenu" && self.music.play_menu(&self.audio, &self.assets, "freakyMenu", true) {
                        self.music.fade(0.0, 1.0, 0.5);
                    }
                } else {
                    self.act(then);
                }
            }
            _ => {}
        }

        self.run_dev();

        // Menu updates.
        if !frozen {
            self.music.update(&self.audio);
        }
        let music_ms = self.music.time_ms(&self.audio);
        let action = if frozen {
            None
        } else if let Some(o) = &mut self.options {
            o.gamepad_present = self.pads.connected();
            let a = o.update(dt, &mut self.settings, &self.audio, &mut self.music, &self.assets);
            if std::mem::take(&mut o.dirty) {
                self.settings_changed();
            }
            Some(a)
        } else if let Some(c) = &mut self.credits {
            Some(c.update(dt, self.shift))
        } else if let Some(m) = &mut self.mods {
            if let Some(mm) = &mut self.main_menu {
                mm.update(dt);
            }
            Some(m.update(dt))
        } else if let Some(c) = &mut self.char_select {
            let gfx = self.loaded.as_mut().map(|l| &mut l.gfx);
            let a = c.update(dt, gfx, &self.audio, &mut self.music);
            if let Some(save) = c.take_save() {
                save.save();
                self.save = save;
            }
            Some(a)
        } else {
            match &self.state {
                GameState::Title => match (self.title.as_mut(), self.loaded.as_mut()) {
                    (Some(t), Some(l)) => Some(t.update(dt, music_ms, &mut l.gfx, &self.audio, &self.assets, &mut self.music)),
                    _ => None,
                },
                GameState::MainMenu => self.main_menu.as_mut().map(|m| m.update(dt)),
                GameState::StoryMenu => self.story.as_mut().map(|s| s.update(dt, music_ms)),
                GameState::Freeplay => {
                    if let Some(m) = &mut self.main_menu {
                        m.update(dt);
                    }
                    self.freeplay.as_mut().map(|f| f.update(dt, &self.audio, &self.assets, &mut self.music))
                }
                _ => None,
            }
        };
        if let Some(a) = action {
            self.act(a);
        }

        let Some(l) = self.loaded.as_mut() else {
            return;
        };
        l.debug.frame(dt.max(1e-4));
        l.tray.update(&l.gfx, dt, &self.settings);
        l.shots.update(&l.gfx, dt);
        let (gfx, fonts) = (&mut l.gfx, &l.fonts);
        fonts.set_time(self.clock);
        let font = &fonts.bold;
        if let Some(o) = &self.options {
            o.draw(gfx, fonts, &l.skin, &self.settings);
        } else if let Some(c) = &self.credits {
            c.draw(gfx, fonts.vcr.as_ref());
        } else if let Some(c) = &self.char_select {
            c.draw(gfx);
        } else {
            match &self.state {
                GameState::Play(_) => {
                    if let Some(scene) = &self.play {
                        let skin = if scene.note_style == l.skin.id { &l.skin } else { l.skins.get(&scene.note_style).unwrap_or(&l.skin) };
                        scene.draw(gfx, skin, font, fonts.vcr.as_ref());
                        if let Some(g) = &self.ghost {
                            let skin = if g.note_style == l.skin.id { &l.skin } else { l.skins.get(&g.note_style).unwrap_or(&l.skin) };
                            g.draw_ghost(gfx, skin);
                        }
                        scene.draw_subtitles(gfx, fonts.vcr.as_ref());
                    }
                    if let Some(p) = &self.pause {
                        p.draw(gfx, fonts);
                    }
                }
                GameState::Title => match &self.title {
                    Some(t) => t.draw(gfx, fonts),
                    None => gfx.begin([0.0, 0.0, 0.0, 1.0]),
                },
                GameState::StoryMenu => match &self.story {
                    Some(s) => s.draw(gfx, fonts.vcr.as_ref()),
                    None => gfx.begin([0.0, 0.0, 0.0, 1.0]),
                },
                GameState::MainMenu | GameState::Freeplay => {
                    match &self.main_menu {
                        Some(m) => m.draw(gfx, fonts.vcr.as_ref()),
                        None => gfx.begin([0.0, 0.0, 0.0, 1.0]),
                    }
                    if let (GameState::MainMenu, Some(m)) = (&self.state, &self.mods) {
                        m.draw(gfx, fonts);
                    }
                    if let (GameState::Freeplay, Some(f)) = (&self.state, &self.freeplay) {
                        f.draw(gfx, fonts);
                    }
                }
                GameState::GameOver(_) if self.play.as_ref().is_some_and(|s| s.has_death()) => {
                    if let Some(scene) = &self.play {
                        scene.draw_death(gfx);
                    }
                }
                GameState::GameOver(req) => {
                    gfx.begin([0.05, 0.0, 0.0, 1.0]);
                    font.draw_centered(gfx, "GAME OVER", 640.0, 250.0, 1.2, WHITE);
                    font.draw_centered(
                        gfx,
                        &req.song_id.replace('-', " "),
                        640.0,
                        380.0,
                        0.6,
                        [1.0, 1.0, 1.0, 0.7],
                    );
                    font.draw_centered(gfx, "ENTER RETRY   ESC BACK", 640.0, 560.0, 0.4, WHITE);
                }
                GameState::Results(_) if self.results.is_some() => {
                    if let Some(r) = &self.results {
                        r.draw(gfx);
                    }
                }
                GameState::Results(r) => {
                    gfx.begin([0.95, 0.75, 0.25, 1.0]);
                    let t = &r.tallies;
                    let rank = match r.rank {
                        Some(ScoringRank::PerfectGold) => "PERFECT!!",
                        Some(ScoringRank::Perfect) => "PERFECT",
                        Some(ScoringRank::Excellent) => "EXCELLENT",
                        Some(ScoringRank::Great) => "GREAT",
                        Some(ScoringRank::Good) => "GOOD",
                        Some(ScoringRank::Shit) | None => "LOSS",
                    };
                    font.draw(gfx, "RESULTS", 60.0, 40.0, 1.0, WHITE);
                    let lines = [
                        format!("TOTAL NOTES {}", t.total_notes),
                        format!("MAX COMBO {}", t.max_combo),
                        format!("SICK {}", t.sick),
                        format!("GOOD {}", t.good),
                        format!("BAD {}", t.bad),
                        format!("SHIT {}", t.shit),
                        format!("MISSED {}", t.missed),
                    ];
                    for (i, s) in lines.iter().enumerate() {
                        font.draw(gfx, s, 80.0, 160.0 + i as f32 * 58.0, 0.55, WHITE);
                    }
                    font.draw(gfx, &format!("SCORE {}", r.score), 700.0, 200.0, 0.7, WHITE);
                    font.draw(
                        gfx,
                        &format!("CLEAR {:.0}%", tally_completion(t) * 100.0),
                        700.0,
                        300.0,
                        0.7,
                        WHITE,
                    );
                    font.draw(gfx, rank, 700.0, 420.0, 1.0, WHITE);
                    font.draw(gfx, "ENTER CONTINUE", 700.0, 640.0, 0.4, WHITE);
                }
                _ => gfx.begin([0.0, 0.0, 0.0, 1.0]),
            }
        }
        if let Some((s, _)) = &self.stickers {
            s.draw(gfx);
        }
        if let Some(f) = &self.fade {
            draw_fade(gfx, l.gradient, f, self.clock);
        }
        // Overlays (left out of screenshots, like the original's stage sprites).
        if !l.shots.pending {
            l.tray.draw(gfx, &self.settings);
            l.debug.draw(gfx, self.settings.debug_display, self.settings.debug_display_bg as f32 / 100.0);
            l.shots.draw(gfx);
        }
        gfx.present();
        l.shots.after_present(gfx, &self.audio, &self.settings);
    }

    /// Focus lost/regained, for "Pause on Unfocus".
    fn focus_changed(&mut self, focused: bool) {
        if !self.settings.auto_pause {
            self.unfocused = false;
            return;
        }
        if !focused {
            self.unfocused = true;
            if matches!(self.state, GameState::Play(_)) && self.play.is_some() {
                // PlayState.onFocusLost opens the pause menu.
                self.open_pause();
            } else if !self.audio.paused() {
                self.audio.set_paused(true);
                self.resume_on_focus = true;
            }
            self.audio.set_master(0.0);
        } else {
            self.unfocused = false;
            self.audio.set_master(self.settings.master_gain());
            if std::mem::take(&mut self.resume_on_focus) && self.pause.is_none() {
                self.unpause_song();
            }
            self.last_frame = None;
        }
    }
}

/// `Tallies.combine`: a week's tallies across its songs.
fn add_tallies(a: &mut Tallies, b: &Tallies) {
    a.sick += b.sick;
    a.good += b.good;
    a.bad += b.bad;
    a.shit += b.shit;
    a.missed += b.missed;
    a.combo = b.combo;
    a.max_combo = a.max_combo.max(b.max_combo);
    a.total_notes_hit += b.total_notes_hit;
    a.total_notes += b.total_notes;
}

fn title_case(s: &str) -> String {
    s.replace('-', " ")
        .split(' ')
        .map(|w| {
            let mut c = w.chars();
            c.next().map(|f| f.to_uppercase().chain(c).collect::<String>()).unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.loaded.is_some() {
            return;
        }
        #[cfg(feature = "importer")]
        if self.setup.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("Rusted Mic")
            .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 720.0));
        let gfx = match event_loop.create_window(attrs).map_err(|e| e.to_string()).and_then(|w| Gfx::new(Arc::new(w))) {
            Ok(g) => g,
            Err(e) => {
                self.error = Some(e);
                event_loop.exit();
                return;
            }
        };
        #[cfg(feature = "importer")]
        if let Some(reason) = self.setup_reason.take() {
            let mut gfx = gfx;
            let screen = crate::setup_screen::SetupScreen::new(&mut gfx, Some(reason));
            self.setup = Some((screen, Some(gfx)));
            return;
        }
        self.enter_game(gfx, event_loop);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        #[cfg(feature = "importer")]
        if let Some((screen, gfx)) = self.setup.as_mut() {
            let action = match event {
                WindowEvent::CloseRequested => crate::setup_screen::SetupAction::Quit,
                WindowEvent::Resized(size) => {
                    if let Some(g) = gfx.as_mut() {
                        g.resize(size.width, size.height);
                    }
                    crate::setup_screen::SetupAction::None
                }
                WindowEvent::CursorMoved { position, .. } => {
                    if let Some(g) = gfx.as_ref() {
                        let (x, y) = g.to_canvas(position.x as f32, position.y as f32);
                        screen.mouse_moved(x, y);
                    }
                    crate::setup_screen::SetupAction::None
                }
                WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left, .. } => screen.click(),
                WindowEvent::KeyboardInput { event: KeyEvent { physical_key: PhysicalKey::Code(code), state: ElementState::Pressed, .. }, .. } => screen.key(code),
                WindowEvent::RedrawRequested => {
                    self.setup_frame(event_loop);
                    crate::setup_screen::SetupAction::None
                }
                _ => crate::setup_screen::SetupAction::None,
            };
            self.setup_action(action, event_loop);
            return;
        }
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(l) = self.loaded.as_mut() {
                    l.gfx.resize(size.width, size.height);
                }
            }
            WindowEvent::Focused(f) => self.focus_changed(f),
            WindowEvent::CursorMoved { position, .. } => {
                if let Some(l) = &self.loaded {
                    self.cursor = l.gfx.to_canvas(position.x as f32, position.y as f32);
                }
                if let Some(m) = &mut self.mods {
                    m.mouse_moved(self.cursor.0, self.cursor.1);
                }
            }
            WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left, .. } => {
                // Clicking the screenshot preview opens the screenshots folder.
                let (x, y) = self.cursor;
                if let Some(m) = &mut self.mods {
                    m.click(x, y, &self.audio);
                }
                if let Some(r) = self.loaded.as_ref().and_then(|l| l.shots.preview_rect) {
                    if x >= r[0] && y >= r[1] && x <= r[0] + r[2] && y <= r[1] + r[3] {
                        let _ = std::process::Command::new("explorer").arg(crate::overlay::screenshot_dir()).spawn();
                    }
                }
            }
            WindowEvent::KeyboardInput { event, .. } => self.on_key(event, Instant::now()),
            WindowEvent::RedrawRequested => self.frame(),
            _ => {}
        }
        if self.quit {
            event_loop.exit();
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        #[cfg(feature = "importer")]
        if let Some((_, Some(g))) = &self.setup {
            g.window().request_redraw();
        }
        let Some(l) = &self.loaded else { return };
        // FPS cap (`Preferences.framerate`) unless Unlocked Framerate is on.
        if self.settings.unlocked_framerate {
            event_loop.set_control_flow(ControlFlow::Poll);
            l.gfx.window().request_redraw();
            return;
        }
        let period = Duration::from_secs_f64(1.0 / self.settings.framerate.clamp(30, 500) as f64);
        let now = Instant::now();
        let due = self.next_frame.unwrap_or(now);
        if now >= due {
            // Keep a steady cadence, but don't try to catch up after a stall.
            let next = due + period;
            self.next_frame = Some(if next < now { now + period } else { next });
            event_loop.set_control_flow(ControlFlow::Poll);
            l.gfx.window().request_redraw();
        } else {
            event_loop.set_control_flow(ControlFlow::WaitUntil(due));
        }
    }
}

/// A screen-high black block with a screen-high gradient edge below it, swept
/// vertically (flixel-addons `TransitionFade` with a y direction): down to cover
/// on the way out, up to reveal on the way in.
fn draw_fade(gfx: &mut Gfx, gradient: crate::gfx::TexId, f: &Fade, clock: f32) {
    use crate::gfx::{VIEW_H, VIEW_W};
    let h = VIEW_H;
    let top = match f {
        Fade::Out { start, .. } => -2.0 * h + 2.0 * h * ((clock - start) / FADE_OUT_SECS).clamp(0.0, 1.0),
        Fade::In { start } => -2.0 * h * ((clock - start) / FADE_IN_SECS).clamp(0.0, 1.0),
    };
    gfx.rect(0.0, top, VIEW_W, h, [0.0, 0.0, 0.0, 1.0]);
    let dst = [[0.0, top + h], [VIEW_W, top + h], [VIEW_W, top + 2.0 * h], [0.0, top + 2.0 * h]];
    gfx.quad_uv(gradient, [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]], dst, [1.0; 4]);
}
