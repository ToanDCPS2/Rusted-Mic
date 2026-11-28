//! Which parts of the HUD a level's scripts take over, found by reading the
//! scripts before any of them runs. The Psych Engine health bar and time bar
//! options redraw and move the HUD; when a script draws its own health bar or
//! moves the strumlines, the option would fight it, so it is turned off for
//! that level.
//!
//! This is a reading of the source, not a run: a script counts when it names
//! the health bar or icons, or names a strumline and writes a position. A
//! module (which is loaded for every song) that checks the song id only counts
//! when one of the ids it checks is this song.

use std::sync::OnceLock;

use regex::Regex;

/// What a level's scripts take over.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct HudTakeover {
    pub health_bar: bool,
    pub strumlines: bool,
}

impl HudTakeover {
    pub fn any(self) -> bool {
        self.health_bar || self.strumlines
    }
}

fn re(cell: &'static OnceLock<Regex>, pat: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pat).expect("hud_scan regex"))
}

/// The source without comments or string contents (string literals become
/// `""`), so names inside them don't count.
fn code_only(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                    if b[i] == b'\n' {
                        out.push('\n');
                    }
                    i += 1;
                }
                i += 2;
            }
            q @ (b'"' | b'\'') => {
                out.push(q as char);
                i += 1;
                while i < b.len() && b[i] != q {
                    if b[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
                out.push(q as char);
                i += 1;
            }
            _ => {
                // Copy one UTF-8 character.
                let ch = src[i..].chars().next().unwrap_or(' ');
                out.push(ch);
                i += ch.len_utf8();
            }
        }
    }
    out
}

/// Does this script draw over or change the health bar?
fn touches_health_bar(code: &str) -> bool {
    static R: OnceLock<Regex> = OnceLock::new();
    re(&R, r"\b(healthBar|healthBarBG|iconP1|iconP2)\b").is_match(code)
}

/// Does this script move or place a strumline (or its arrows)? Names that
/// hold a strumline are followed through `var a = ...strumline...` and
/// `for (a in ...strumline...)`; a position written through one of them counts.
fn moves_strumlines(code: &str) -> bool {
    static NAMES: OnceLock<Regex> = OnceLock::new();
    static BIND: OnceLock<Regex> = OnceLock::new();
    let names = re(&NAMES, r"\b(playerStrumline|opponentStrumline|strumlineNotes|playerStrums|opponentStrums|strumLineNotes|getStrumline|Strumline)\b");
    if !names.is_match(code) {
        return false;
    }
    let mut held: Vec<String> = ["playerStrumline", "opponentStrumline", "strumlineNotes", "playerStrums", "opponentStrums", "strumLineNotes"].map(String::from).to_vec();
    // `var a = <expr naming a held name>` / `a = ...` / `for (a in <expr>)`, to a fixed point.
    let bind = re(&BIND, r"(?:\bvar\s+|\bfor\s*\(\s*|^|[;{}]\s*)([A-Za-z_]\w*)\s*(?::[\w<>.]+\s*)?(?:=[^=]|\bin\b)([^;]*)");
    loop {
        let before = held.len();
        for line in code.lines() {
            for c in bind.captures_iter(line.trim()) {
                let (name, rhs) = (c[1].to_owned(), &c[2]);
                if !held.contains(&name) && (names.is_match(rhs) || held.iter().any(|h| word_in(rhs, h))) {
                    held.push(name);
                }
            }
        }
        if held.len() == before {
            break;
        }
    }
    let alt = held.iter().map(|h| regex::escape(h)).collect::<Vec<_>>().join("|");
    let root = format!(r"\b(?:{alt})\b(?:\.\w+|\[[^\]]*\]|\([^)]*\))*");
    let write = Regex::new(&format!(r"{root}\.(?:x|y)\s*(?:=[^=]|\+=|-=|\*=|/=)|{root}\.(?:setPosition|screenCenter)\s*\(")).expect("hud_scan write");
    let tween = Regex::new(&format!(r"\b(?:FlxTween\.tween|FlxTween\.num|tween)\s*\(\s*(?:[\w.]*\.)?{root}\s*,[^;]*[{{,]\s*(?:x|y)\s*:")).expect("hud_scan tween");
    write.is_match(code) || tween.is_match(code)
}

fn word_in(hay: &str, w: &str) -> bool {
    hay.match_indices(w).any(|(i, _)| {
        let before = hay[..i].chars().next_back().is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
        let after = hay[i + w.len()..].chars().next().is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
        before && after
    })
}

/// The song ids a module checks for (`currentSong.id == 'x'`,
/// `startsWith(currentSong.id, "x")`, `["a", "b"].contains(songId)`), each
/// with whether it's a prefix check. Empty when it doesn't check.
fn song_gates(src: &str) -> Vec<(String, bool)> {
    static ID: OnceLock<Regex> = OnceLock::new();
    static LIT: OnceLock<Regex> = OnceLock::new();
    let id = re(&ID, r"\b(currentSong\.id|currentSongId|songId|song\.id|currentSong\.songName)\b");
    let lit = re(&LIT, r#""([^"\\\n]*)"|'([^'\\\n]*)'"#);
    let mut out = Vec::new();
    for line in src.lines() {
        let line = line.split("//").next().unwrap_or("");
        if !id.is_match(line) {
            continue;
        }
        let prefix = line.contains("startsWith");
        for c in lit.captures_iter(line) {
            let s = c.get(1).or_else(|| c.get(2)).map_or("", |m| m.as_str());
            if !s.is_empty() {
                out.push((s.to_lowercase(), prefix));
            }
        }
    }
    out
}

/// One script's source. `module`: loaded for every song, so a song check in
/// it decides whether it applies to `song_id`.
pub fn scan(src: &str, module: bool, song_id: &str) -> HudTakeover {
    if module {
        let gates = song_gates(src);
        let song = song_id.to_lowercase();
        if !gates.is_empty() && !gates.iter().any(|(g, prefix)| if *prefix { song.starts_with(g.as_str()) } else { *g == song }) {
            return HudTakeover::default();
        }
    }
    let code = code_only(src);
    HudTakeover { health_bar: touches_health_bar(&code), strumlines: moves_strumlines(&code) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_bar_names() {
        let s = "function onCreate(e) { PlayState.instance.iconP2.visible = false; }";
        assert!(scan(s, false, "x").health_bar);
        assert!(!scan("// iconP2 is fine\nvar a = 'healthBar';", false, "x").health_bar);
    }

    #[test]
    fn strumline_needs_a_write() {
        let read = "var p = PlayState.instance.playerStrumline; trace(p.members[0].x);";
        assert!(!scan(read, false, "x").strumlines);
        let write = "var p = PlayState.instance.playerStrumline; p.x = FlxG.width / 2;";
        assert!(scan(write, false, "x").strumlines);
        let tween = "FlxTween.tween(PlayState.instance.opponentStrumline, {x: 100}, 1);";
        assert!(scan(tween, false, "x").strumlines);
        assert!(!scan("if (a.x == b) {} var s = opponentStrumline;", false, "x").strumlines);
        let arrows = "for (arrow in PlayState.instance.opponentStrumline.members) { arrow.alpha = 0; }
bg.x = 100;";
        assert!(!scan(arrows, false, "x").strumlines);
        let moved = "for (arrow in PlayState.instance.opponentStrumline.members) { arrow.x -= 600; }";
        assert!(scan(moved, false, "x").strumlines);
    }

    #[test]
    fn modules_follow_their_song_checks() {
        let m = "function onSongLoaded(e) { isSilly = PlayState.instance.currentSong.id == 'silly-billy'; }\nfunction f() { PlayState.instance.healthBar.visible = false; }";
        assert!(scan(m, true, "silly-billy").health_bar);
        assert!(!scan(m, true, "bopeebo").health_bar);
        let p = "songIsAllowed = StringTools.startsWith(playState.currentSong.id, \"obliterated\");\nvar y = PlayState.instance.healthBarBG.y;";
        assert!(scan(p, true, "obliterated-erect").health_bar);
        assert!(!scan(p, true, "go-go-go").health_bar);
        // No song check: it applies everywhere.
        assert!(scan("function f() { PlayState.instance.iconP1.alpha = 0; }", true, "bopeebo").health_bar);
        // A song's own script always counts.
        assert!(scan(m, false, "bopeebo").health_bar);
    }
}
