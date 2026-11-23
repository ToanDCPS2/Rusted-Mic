//! Window, event loop and screen flow, driven by `funkin_core::state::GameState`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use funkin_core::scoring::{calculate_rank, tally_completion, ScoringRank};
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
use crate::menus::freeplay::{list_songs, Freeplay, FreeplaySong};
use crate::menus::main_menu::MainMenu;
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
    skin: NoteSkin,
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
    music: Music,
    title: Option<Title>,
    main_menu: Option<MainMenu>,
    freeplay: Option<Freeplay>,
    /// The options menu, open over the main menu state.
    options: Option<Options>,
    /// Freeplay's selection, kept across songs.
    remembered: Option<(String, String)>,
    play: Option<PlayScene>,
    pause: Option<PauseMenu>,
    /// Deaths on the current song (`PlayState.deathCounter`).
    deaths: u32,
    loaded: Option<Loaded>,
    error: Option<String>,
    start: Option<PlayRequest>,
    botplay: bool,
    last_frame: Option<Instant>,
    /// When the next frame is due (framerate cap).
    next_frame: Option<Instant>,
    clock: f32,
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
pub fn run(
    asset_root: Option<std::path::PathBuf>,
    setup_reason: Option<String>,
    start: Option<PlayRequest>,
    botplay: bool,
) -> Result<(), String> {
    #[cfg(not(feature = "importer"))]
    let _ = &setup_reason;
    let needs_setup = asset_root.is_none();
    let asset_root = asset_root.unwrap_or_default();
    let registry = SongRegistry::new(&asset_root);
    let songs = Arc::new(if needs_setup { Vec::new() } else { list_songs(&registry) });
    let settings = Settings::load();
    let audio = Audio::new()?;
    audio.set_master(settings.master_gain());
    let discord = Discord::new(&settings.discord_app_id);
    let mut app = App {
        #[cfg(feature = "importer")]
        setup: None,
        #[cfg(feature = "importer")]
        setup_reason: needs_setup.then(|| setup_reason.unwrap_or_default()),
        assets: Assets::new(asset_root),
        registry,
        audio,
        state: GameState::Title,
        songs,
        music: Music::default(),
        title: None,
        main_menu: None,
        freeplay: None,
        options: None,
        remembered: None,
        play: None,
        pause: None,
        deaths: 0,
        loaded: None,
        error: None,
        start,
        botplay,
        last_frame: None,
        next_frame: None,
        clock: 0.0,
        quit: false,
        fade: None,
        settings,
        discord,
        play_started: 0,
        unfocused: false,
        resume_on_focus: false,
        shift: false,
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
    let skin = NoteSkin::load(&mut gfx, assets)?;
    let ramp: Vec<u8> = (0..256u32).flat_map(|i| [0, 0, 0, i as u8]).collect();
    let gradient = gfx.upload_rgba(1, 256, &ramp);
    let tray = SoundTray::new(&mut gfx, assets);
    let shots = Screenshots::new(assets);
    Ok(Loaded { gradient, gfx, fonts, skin, tray, shots, debug: DebugOverlay::new() })
}

impl App {
    /// Assets are ready (at startup, or after setup): load the game and show the title.
    fn enter_game(&mut self, gfx: Gfx, event_loop: &ActiveEventLoop) {
        match load_game(gfx, &self.assets) {
            Ok(mut l) => {
                l.gfx.set_vsync(self.settings.vsync);
                if self.settings.auto_fullscreen {
                    l.gfx.window().set_fullscreen(Some(Fullscreen::Borderless(None)));
                }
                self.loaded = Some(l);
                if let Some(req) = self.start.take() {
                    self.start_song(req, false);
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
                self.songs = Arc::new(list_songs(&self.registry));
                self.assets = Assets::new(root);
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
            strumline_bg: s.strumline_background as f32 / 100.0,
            zoom_camera: s.zoom_camera,
            subtitles: s.subtitles,
        }
    }

    fn start_song(&mut self, req: PlayRequest, practice: bool) {
        let prefs = self.play_prefs();
        let offset = self.settings.global_offset as f64;
        if self.loaded.is_none() {
            return;
        }
        self.music.stop(&self.audio);
        self.close_pause();
        self.audio.set_paused(false);
        // Restarting the same song keeps the death counter.
        if self.play.as_ref().map(|p| &p.request.song_id) != Some(&req.song_id) && !matches!(self.state, GameState::GameOver(_)) {
            self.deaths = 0;
        }
        let l = self.loaded.as_mut().unwrap();
        if let Some(old) = self.play.take() {
            l.gfx.release_textures(old.tex_mark);
        }
        let scene = PlayScene::new(&mut l.gfx, &self.assets, &self.registry, &self.audio, req.clone(), offset, self.botplay, prefs, practice);
        // Whatever loads after this (glyphs, previews) outlives the song.
        let scope = l.gfx.end_scope();
        match scene {
            Ok(scene) => {
                l.gfx.window().set_title(&format!("Rusted Mic - {}", scene.title));
                self.sticker_pack = resolve_pack(&self.assets, scene.sticker_pack.as_deref(), &scene.player);
                self.play = Some(scene);
                self.freeplay = None;
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
        self.audio.set_paused(true);
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
        ));
    }

    /// The game-over quote (`bf.getDeathQuote`): Tankman taunts you, censored
    /// unless Naughtyness is on.
    fn death_quote(&self, opponent: &str, variation: &str) {
        if !opponent.starts_with("tankman") {
            return;
        }
        let censored: &[u32] = if self.settings.naughtyness { &[] } else { &[1, 3, 8, 13, 17, 21] };
        let pool: Vec<u32> = (1..=25).filter(|n| !censored.contains(n)).collect();
        let n = pool[((crate::menus::random() * pool.len() as f32) as usize).min(pool.len() - 1)];
        let dir = if variation == "pico" { "jeffGameover-pico" } else { "jeffGameover" };
        if let Some(s) = self.assets.sound(&format!("week7:{dir}/jeffGameover-{n}")) {
            self.audio.play_sfx(s, 1.0);
        }
    }

    fn open_title(&mut self, skip_intro: bool) {
        let Some(l) = self.loaded.as_mut() else { return };
        match Title::new(&mut l.gfx, &self.assets) {
            Ok(mut t) => {
                if skip_intro {
                    t.skip_intro_now();
                }
                t.start_music(&self.audio, &self.assets, &mut self.music);
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
        let Some(l) = self.loaded.as_mut() else { return };
        if self.main_menu.is_none() {
            if let Ok(mut m) = MainMenu::new(&mut l.gfx, &self.assets, 1) {
                m.covered = true;
                m.flashing_lights = self.settings.flashing_lights;
                self.main_menu = Some(m);
            }
        }
        let remembered = self.remembered.as_ref().map(|(s, d)| (s.as_str(), d.as_str()));
        match Freeplay::new(&mut l.gfx, &self.assets, self.songs.clone(), remembered) {
            Ok(mut f) => {
                f.flashing_lights = self.settings.flashing_lights;
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
            MenuAction::CloseFreeplay => self.close_freeplay(),
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
                self.start_song(req, false);
            }
            MenuAction::Quit => self.quit = true,
        }
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
            StickerExit::GameOver => {
                self.state.apply(Transition::Back);
            }
            StickerExit::Results => {
                self.state.apply(Transition::Confirm);
            }
        }
        self.ensure_freeplay();
    }

    /// Back in freeplay after a song.
    fn ensure_freeplay(&mut self) {
        if matches!(self.state, GameState::Freeplay) && self.freeplay.is_none() {
            self.open_freeplay();
        }
    }

    /// Keys that work everywhere: fullscreen, screenshot, volume, debug display.
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

    fn on_key(&mut self, ev: KeyEvent, at: Instant) {
        let PhysicalKey::Code(code) = ev.physical_key else {
            return;
        };
        let pressed = ev.state == ElementState::Pressed;
        if matches!(code, KeyCode::ShiftLeft | KeyCode::ShiftRight) {
            self.shift = pressed;
        }
        let menu = self.settings.controls.menu_key(code);

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
        if !capturing && !ev.repeat && self.global_key(code) {
            return;
        }
        // Nothing underneath takes input while the stickers go on.
        if self.stickers.as_ref().is_some_and(|(s, _)| s.blocking()) {
            return;
        }

        if let (GameState::Play(_), Some(scene)) = (&self.state, self.play.as_mut()) {
            if let Some(p) = &mut self.pause {
                let is_pause = self.settings.controls.is(Control::Pause, code) && !ev.repeat;
                match p.key(&self.audio, menu, is_pause, self.shift) {
                    PauseAction::None => {}
                    PauseAction::Offset(ms) => {
                        self.settings.global_offset = ms;
                        scene.set_global_offset(ms as f64);
                        self.settings_changed();
                    }
                    PauseAction::Resume => {
                        scene.set_practice(p.practice());
                        self.close_pause();
                        self.audio.set_paused(false);
                    }
                    PauseAction::Restart => {
                        let (req, practice) = (scene.request.clone(), p.practice());
                        self.start_song(req, practice);
                    }
                    PauseAction::ChangeDifficulty(d) => {
                        let mut req = scene.request.clone();
                        req.difficulty = d;
                        let practice = p.practice();
                        self.start_song(req, practice);
                    }
                    PauseAction::Exit => self.exit_with_stickers(StickerExit::Pause),
                }
                return;
            }
            if let Some(lane) = self.settings.controls.note_lane(code) {
                if !ev.repeat {
                    scene.key_down(&self.audio, lane_direction(lane), at);
                }
                return;
            }
            if ev.repeat {
                return;
            }
            if self.settings.controls.is(Control::Pause, code) {
                self.open_pause();
            } else if self.settings.controls.is(Control::Reset, code) {
                scene.reset_death();
            }
            return;
        }

        if let Some(o) = &mut self.options {
            let a = o.key_down(code, menu, ev.repeat, &mut self.settings, &self.audio, &mut self.music, &self.assets);
            if std::mem::take(&mut o.dirty) {
                self.settings_changed();
            }
            self.act(a);
            return;
        }

        let Some(code) = menu else { return };
        let action = match &self.state {
            GameState::Title => match &mut self.title {
                Some(t) if !ev.repeat => t.key(code, &self.audio),
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
            GameState::GameOver(req) => {
                match code {
                    KeyCode::Enter => {
                        let req = req.clone();
                        self.state.apply(Transition::Retry);
                        self.start_song(req, false);
                    }
                    KeyCode::Escape => self.exit_with_stickers(StickerExit::GameOver),
                    _ => {}
                }
                MenuAction::None
            }
            GameState::Results(_) => {
                if matches!(code, KeyCode::Enter | KeyCode::Escape) {
                    self.exit_with_stickers(StickerExit::Results);
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
        self.update_presence();

        // Gameplay update first, so input → judgement → draw happens in one frame.
        if let (GameState::Play(_), Some(scene), None, false) = (&self.state, self.play.as_mut(), &self.pause, frozen) {
            let skin = &self.loaded.as_ref().unwrap().skin;
            match scene.update(&self.audio, skin) {
                PlayOutcome::Running => {}
                PlayOutcome::Died => {
                    let (opp, var) = (scene.opponent.clone(), scene.request.variation.clone());
                    self.deaths += 1;
                    self.leave_play(Transition::Died);
                    self.death_quote(&opp, &var);
                }
                PlayOutcome::Finished => {
                    let s = &scene.session;
                    let result = SongResult {
                        request: scene.request.clone(),
                        score: s.score(),
                        tallies: s.tallies,
                        rank: calculate_rank(&s.tallies),
                    };
                    self.leave_play(Transition::SongFinished(result));
                    if let GameState::Play(next) = self.state.clone() {
                        // Story playlists continue automatically.
                        self.start_song(next, false);
                    }
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
                if matches!(then, MenuAction::ToMainMenu) && self.options.is_some() {
                    // Leaving options: the main menu comes back interactive.
                    self.options = None;
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

        // Menu updates.
        if !frozen {
            self.music.update(&self.audio);
        }
        let music_ms = self.music.time_ms(&self.audio);
        let action = if frozen {
            None
        } else if let Some(o) = &mut self.options {
            let a = o.update(dt, &mut self.settings, &self.audio, &mut self.music, &self.assets);
            if std::mem::take(&mut o.dirty) {
                self.settings_changed();
            }
            Some(a)
        } else {
            match &self.state {
                GameState::Title => self.title.as_mut().map(|t| t.update(dt, music_ms)),
                GameState::MainMenu => self.main_menu.as_mut().map(|m| m.update(dt)),
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
        } else {
            match &self.state {
                GameState::Play(_) => {
                    if let Some(scene) = &self.play {
                        scene.draw(gfx, &l.skin, font, fonts.vcr.as_ref());
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
                GameState::MainMenu | GameState::Freeplay => {
                    match &self.main_menu {
                        Some(m) => m.draw(gfx),
                        None => gfx.begin([0.0, 0.0, 0.0, 1.0]),
                    }
                    if let (GameState::Freeplay, Some(f)) = (&self.state, &self.freeplay) {
                        f.draw(gfx, fonts);
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
                self.audio.set_paused(false);
            }
            self.last_frame = None;
        }
    }
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
            }
            WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left, .. } => {
                // Clicking the screenshot preview opens the screenshots folder.
                let (x, y) = self.cursor;
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
