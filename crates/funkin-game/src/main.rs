//! Rusted Mic: the game window and command-line frontend.
//!
//! Until the renderer lands this binary handles first-boot setup and can run
//! charts headlessly through the real gameplay logic, which is useful for
//! checking that every song in your installation loads and scores correctly.
//!
//! ```text
//! RustedMic                         boot: run setup if needed, then open the game window
//! RustedMic play <song> [difficulty] [variation]
//! RustedMic setup [--path DIR] [--reference]
//! RustedMic status
//! RustedMic songs
//! RustedMic simulate <song> [difficulty] [variation]
//! RustedMic reset                   forget the import (forces setup next launch)
//! ```

mod actor;
mod app;
mod assets;
mod audio;
mod gfx;
mod health_icon;
mod logo;
mod menus;
#[cfg(feature = "importer")]
mod setup_screen;
mod discord;
mod overlay;
mod pause;
mod play;
mod settings;
mod spectrum;
mod stickers;
mod world;

use std::path::PathBuf;
use std::process::ExitCode;

use funkin_core::constants::{DEFAULT_DIFFICULTY, DEFAULT_VARIATION, HIT_WINDOW_MS};
use funkin_core::play::{NoteDirection, NoteKindFlags, PlayConfig, PlayEvent, PlaySession};
use funkin_core::scoring::{calculate_rank, tally_completion};
use funkin_core::song::SongRegistry;

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("error: {msg}");
            ExitCode::FAILURE
        }
    }
}

fn flag_value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn positional(args: &[String]) -> Vec<&str> {
    let mut out = Vec::new();
    let mut skip = false;
    for a in args {
        if skip {
            skip = false;
            continue;
        }
        if a == "--path" || a == "--assets" {
            skip = true;
            continue;
        }
        if !a.starts_with("--") {
            out.push(a.as_str());
        }
    }
    out
}

fn run(args: &[String]) -> Result<(), String> {
    let pos = positional(args);
    match pos.first().copied() {
        None => boot(args),
        #[cfg(feature = "importer")]
        Some("setup") => setup::run_setup(
            flag_value(args, "--path").map(PathBuf::from),
            args.iter().any(|a| a == "--reference"),
        )
        .map(|_| ()),
        #[cfg(feature = "importer")]
        Some("reset") => {
            funkin_assets::marker::clear_marker().map_err(|e| e.to_string())?;
            println!("Import forgotten. Setup will run on next launch.");
            Ok(())
        }
        Some("status") => {
            let root = asset_root(args)?;
            println!("Assets: {}", root.display());
            Ok(())
        }
        Some("logo") => {
            // Writes the procedurally painted logo to a PNG (for previewing).
            // `--square` gives a 1024×1024 icon on its backdrop (Discord's Rich Presence art).
            let out = positional(args).into_iter().filter(|a| *a != "--square").nth(1).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("rusted-mic-logo.png"));
            if args.iter().any(|a| a == "--square") {
                const S: u32 = 1024;
                let mut img = image::RgbaImage::from_raw(S, S, logo::render_backdrop(S, S)).ok_or("backdrop")?;
                let logo_img = image::RgbaImage::from_raw(logo::LOGO_W, logo::LOGO_H, logo::render_logo()).ok_or("logo")?;
                let w = 960;
                let h = w * logo::LOGO_H / logo::LOGO_W;
                let logo_img = image::imageops::resize(&logo_img, w, h, image::imageops::FilterType::Lanczos3);
                image::imageops::overlay(&mut img, &logo_img, ((S - w) / 2) as i64, ((S - h) / 2) as i64);
                img.save(&out).map_err(|e| e.to_string())?;
            } else {
                image::save_buffer(&out, &logo::render_logo(), logo::LOGO_W, logo::LOGO_H, image::ColorType::Rgba8).map_err(|e| e.to_string())?;
            }
            println!("wrote {}", out.display());
            Ok(())
        }
        Some("songs") => list_songs(&asset_root(args)?),
        Some("play") => {
            let song = pos
                .get(1)
                .ok_or("usage: RustedMic play <song> [difficulty] [variation]")?;
            let req = funkin_core::state::PlayRequest {
                song_id: song.to_string(),
                difficulty: pos.get(2).copied().unwrap_or(DEFAULT_DIFFICULTY).to_owned(),
                variation: pos.get(3).copied().unwrap_or(DEFAULT_VARIATION).to_owned(),
                playlist: vec![],
                is_story: false,
                level_id: None,
            };
            app::run(
                Some(asset_root(args)?),
                None,
                Some(req),
                args.iter().any(|a| a == "--botplay"),
            )
        }
        Some("simulate") => {
            let song = pos
                .get(1)
                .ok_or("usage: RustedMic simulate <song> [difficulty] [variation]")?;
            let diff = pos.get(2).copied().unwrap_or(DEFAULT_DIFFICULTY);
            let var = pos.get(3).copied();
            simulate(&asset_root(args)?, song, diff, var)
        }
        Some(other) => Err(format!("unknown command '{other}'")),
    }
}

/// Resolve the asset root, refusing to continue if no valid assets are available.
fn asset_root(args: &[String]) -> Result<PathBuf, String> {
    if let Some(p) = flag_value(args, "--assets") {
        let p = PathBuf::from(p);
        if p.is_file() {
            funkin_core::vfs::mount(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        }
        if !funkin_core::vfs::is_dir(&p.join("data").join("songs")) {
            return Err(format!(
                "'{}' is not a game asset folder (no data\\songs).",
                p.display()
            ));
        }
        return Ok(p);
    }
    #[cfg(feature = "importer")]
    {
        match funkin_assets::resolve_asset_root() {
            funkin_assets::AssetStatus::Ready(m) => {
                // Imports from before font extraction existed: pull the fonts out now.
                let fonts = funkin_assets::paths::fonts_dir();
                if m.mode == funkin_assets::ImportMode::Reference && !fonts.join("5by7.ttf").exists() {
                    for exe in funkin_assets::validate::EXECUTABLE_NAMES {
                        let p = m.source_install_dir.join(exe);
                        if p.is_file() {
                            if let Err(e) = funkin_assets::fonts::extract_embedded_fonts(&p, &fonts) {
                                log::warn!("fonts: {e}");
                            }
                            break;
                        }
                    }
                }
                Ok(m.asset_root)
            }
            funkin_assets::AssetStatus::SetupRequired(why) => {
                Err(format!("{why}\nRun `RustedMic setup` to import your installation. Gameplay is unavailable until then."))
            }
        }
    }
    #[cfg(not(feature = "importer"))]
    Err("built without the importer: pass --assets <path to the game's assets folder>".into())
}

fn boot(args: &[String]) -> Result<(), String> {
    let botplay = args.iter().any(|a| a == "--botplay");
    match asset_root(args) {
        Ok(root) => app::run(Some(root), None, None, botplay),
        // No assets yet: the window opens on the first-run setup screen.
        #[cfg(feature = "importer")]
        Err(reason) => app::run(None, Some(reason), None, botplay),
        #[cfg(not(feature = "importer"))]
        Err(reason) => Err(reason),
    }
}

fn list_songs(root: &std::path::Path) -> Result<(), String> {
    let reg = SongRegistry::new(root);
    let ids = reg.list_song_ids();
    println!("{} songs in {}", ids.len(), root.display());
    for id in ids {
        match reg.load_metadata(&id, DEFAULT_VARIATION) {
            Ok(m) => println!(
                "  {id:<20} {:<28} {:<24} [{}] variations: {}",
                m.song_name,
                m.artist,
                m.play_data.difficulties.join(", "),
                if m.play_data.song_variations.is_empty() {
                    "-".to_owned()
                } else {
                    m.play_data.song_variations.join(", ")
                }
            ),
            Err(e) => println!("  {id:<20} <failed to load: {e}>"),
        }
    }
    println!("\nLevels:");
    for l in reg.load_levels() {
        println!("  {:<12} {:<28} {}", l.id, l.name, l.songs.join(", "));
    }
    Ok(())
}

/// Play a chart with perfectly timed inputs through the real gameplay logic.
fn simulate(
    root: &std::path::Path,
    song_id: &str,
    difficulty: &str,
    variation: Option<&str>,
) -> Result<(), String> {
    let reg = SongRegistry::new(root);
    let song = reg.load_song(song_id).map_err(|e| e.to_string())?;
    let var = song
        .variation_for_difficulty(difficulty, variation)
        .ok_or_else(|| format!("'{song_id}' has no '{difficulty}' difficulty"))?;
    let notes = var
        .chart
        .notes(difficulty)
        .ok_or("chart has no notes for that difficulty")?;
    let speed = var.chart.scroll_speed(difficulty);

    let config = PlayConfig {
        scroll_speed: speed,
        ..Default::default()
    };
    let mut session = PlaySession::new(
        config,
        &var.metadata.time_changes,
        notes,
        &var.chart.events,
        |_| NoteKindFlags::default(),
    );

    // Build a perfect input script: press on the strum time, release at the end of the hold.
    let mut script: Vec<(f64, bool, NoteDirection)> = Vec::new();
    for n in session.player.notes.iter() {
        script.push((n.strum_time(), true, n.direction));
        script.push((
            n.strum_time() + n.data.length.max(1000.0 / 60.0),
            false,
            n.direction,
        ));
    }
    script.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));

    let end = notes.iter().map(|n| n.time + n.length).fold(0.0, f64::max) + HIT_WINDOW_MS + 500.0;
    let frame = 1000.0 / 240.0;
    let (mut t, mut si) = (-1000.0, 0);
    let mut events = Vec::new();
    let (mut hits, mut misses) = (0usize, 0usize);
    let started = std::time::Instant::now();
    while t <= end {
        while si < script.len() && script[si].0 <= t {
            let (time, press, dir) = script[si];
            if press {
                session.press(dir, time);
            } else {
                session.release(dir);
            }
            si += 1;
        }
        session.update(t, &mut events);
        for e in events.drain(..) {
            match e {
                PlayEvent::NoteHit {
                    judgement: Some(_), ..
                } => hits += 1,
                PlayEvent::NoteMiss { .. } => misses += 1,
                _ => {}
            }
        }
        t += frame;
    }

    let m = &var.metadata;
    println!(
        "{} by {}: {difficulty} ({} variation), scroll speed {speed}",
        m.song_name, m.artist, m.variation
    );
    println!(
        "  BPM changes: {}, player notes: {}, chart events: {}",
        m.time_changes.len(),
        session.tallies.total_notes,
        session.events.len()
    );
    println!(
        "  hits: {hits}, misses: {misses}, max combo: {}",
        session.tallies.max_combo
    );
    println!(
        "  score: {}  clear: {:.1}%  rank: {:?}",
        session.score(),
        tally_completion(&session.tallies) * 100.0,
        calculate_rank(&session.tallies)
    );
    println!(
        "  instrumental: {}",
        reg.inst_path(song_id, &m.play_data.characters.instrumental)
            .display()
    );
    for v in reg.voice_list(song_id, m) {
        println!("  vocals: {}", v.display());
    }
    println!("  simulated in {:.2?}", started.elapsed());
    Ok(())
}

#[cfg(feature = "importer")]
mod setup {
    use std::io::{BufRead, Write};
    use std::path::PathBuf;

    use funkin_assets::{
        import_installation, scan_common_locations, validate_installation, ImportMode,
        InstallMarker,
    };

    fn prompt(q: &str) -> String {
        print!("{q}");
        let _ = std::io::stdout().flush();
        let mut s = String::new();
        let _ = std::io::stdin().lock().read_line(&mut s);
        // PowerShell pipes may prepend a UTF-8 BOM.
        s.trim()
            .trim_start_matches('\u{feff}')
            .trim()
            .trim_matches('"')
            .to_owned()
    }

    #[cfg(feature = "dialog")]
    fn pick_folder() -> Option<PathBuf> {
        rfd::FileDialog::new()
            .set_title("Select your Friday Night Funkin' installation folder (contains Funkin.exe)")
            .pick_folder()
    }
    #[cfg(not(feature = "dialog"))]
    fn pick_folder() -> Option<PathBuf> {
        None
    }

    /// Interactive first-boot setup. Refuses to finish without a valid installation.
    pub fn run_setup(path: Option<PathBuf>, reference: bool) -> Result<InstallMarker, String> {
        let source = match path {
            Some(p) => p,
            None => {
                println!("Scanning for an installation (Steam, itch, Program Files, Desktop, Downloads)...");
                let found = scan_common_locations();
                for (i, f) in found.iter().enumerate() {
                    println!(
                        "  [{}] {} ({} songs)",
                        i + 1,
                        f.install_dir.display(),
                        f.song_count
                    );
                }
                if found.is_empty() {
                    println!("  none found.");
                }
                let hint = if cfg!(feature = "dialog") {
                    ", 'b' to browse"
                } else {
                    ""
                };
                let answer = prompt(&format!(
                    "Enter a number, a folder path{hint}, or press Enter to cancel: "
                ));
                if answer.is_empty() {
                    return Err(refusal());
                } else if answer.eq_ignore_ascii_case("b") {
                    pick_folder().ok_or_else(refusal)?
                } else if let Some(f) = answer
                    .parse::<usize>()
                    .ok()
                    .and_then(|n| found.get(n.wrapping_sub(1)))
                {
                    f.install_dir.clone()
                } else {
                    PathBuf::from(answer)
                }
            }
        };

        let inst = validate_installation(&source).map_err(|e| format!("{e}\n\n{}", refusal()))?;
        println!(
            "Found a valid installation: {} ({} songs)",
            inst.install_dir.display(),
            inst.song_count
        );

        let mode = if reference {
            ImportMode::Reference
        } else {
            ImportMode::Pack
        };
        let mut last_pct = u64::MAX;
        let marker = import_installation(&inst.install_dir, mode, |p| {
            let pct = p.files_done * 100 / p.files_total.max(1);
            if pct != last_pct {
                last_pct = pct;
                print!(
                    "\rImporting... {pct:>3}% ({}/{} files, {} MB)",
                    p.files_done,
                    p.files_total,
                    p.bytes_done / 1_048_576
                );
                let _ = std::io::stdout().flush();
            }
        })
        .map_err(|e| e.to_string())?;
        println!(
            "\nDone. Assets available at {}",
            marker.asset_root.display()
        );
        Ok(marker)
    }

    fn refusal() -> String {
        "Setup did not complete. This port contains no game assets and cannot be played without \
         a legally obtained official copy of Friday Night Funkin'. Run `RustedMic setup` once you have one."
            .into()
    }
}
