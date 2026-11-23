//! Discord Rich Presence over Discord's local IPC pipe (`\\.\pipe\discord-ipc-N`),
//! the same protocol the original's `DiscordClient` (discord-rpc) speaks.
//!
//! Discord shows the *application's* name ("Playing Rusted Mic") and its art
//! assets, so presence needs a Discord application id: set `discord_app_id`
//! in settings.json (or `RUSTED_MIC_DISCORD_APP_ID`). The app's Rich Presence
//! art should have an image named `logo` (export one with `RustedMic logo
//! --square logo.png`).

use std::io::{Read, Write};
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::json;

#[derive(Debug, Clone, PartialEq)]
pub struct Presence {
    pub details: String,
    pub state: Option<String>,
    /// Show "elapsed" from this unix time.
    pub start: Option<u64>,
}

enum Msg {
    Set(Option<Presence>),
}

pub struct Discord {
    tx: Option<Sender<Msg>>,
    last: Option<Option<Presence>>,
}

pub fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl Discord {
    /// Start the background client (does nothing without an application id).
    pub fn new(app_id: &str) -> Self {
        let id = if app_id.trim().is_empty() {
            std::env::var("RUSTED_MIC_DISCORD_APP_ID").ok().or_else(|| option_env!("RUSTED_MIC_DISCORD_APP_ID").map(str::to_owned))
        } else {
            Some(app_id.trim().to_owned())
        };
        let Some(id) = id.filter(|s| s.chars().all(|c| c.is_ascii_digit()) && !s.is_empty()) else {
            log::info!("discord: no application id set; rich presence is off");
            return Self { tx: None, last: None };
        };
        let (tx, rx) = channel::<Msg>();
        std::thread::Builder::new()
            .name("discord-rpc".into())
            .spawn(move || {
                let mut want: Option<Presence> = None;
                let mut conn: Option<std::fs::File> = None;
                let mut dirty = true;
                let mut nonce = 0u64;
                loop {
                    match rx.recv_timeout(Duration::from_secs(5)) {
                        Ok(Msg::Set(p)) => {
                            want = p;
                            dirty = true;
                        }
                        Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                    if conn.is_none() {
                        conn = connect(&id);
                        dirty = conn.is_some();
                    }
                    if !dirty {
                        continue;
                    }
                    if let Some(c) = &mut conn {
                        nonce += 1;
                        let activity = want.as_ref().map(|p| {
                            let mut a = json!({
                                "details": p.details,
                                "assets": { "large_image": "logo", "large_text": "Rusted Mic" },
                            });
                            if let Some(s) = &p.state {
                                a["state"] = json!(s);
                            }
                            if let Some(t) = p.start {
                                a["timestamps"] = json!({ "start": t });
                            }
                            a
                        });
                        let msg = json!({
                            "cmd": "SET_ACTIVITY",
                            "args": { "pid": std::process::id(), "activity": activity },
                            "nonce": nonce.to_string(),
                        });
                        if send(c, 1, &msg).and_then(|_| recv(c)).is_err() {
                            log::info!("discord: connection lost");
                            conn = None;
                        } else {
                            dirty = false;
                        }
                    }
                }
            })
            .ok();
        Self { tx: Some(tx), last: None }
    }

    /// Update the presence (None clears it). Repeats are ignored.
    pub fn set(&mut self, p: Option<Presence>) {
        if self.last.as_ref() == Some(&p) {
            return;
        }
        self.last = Some(p.clone());
        if let Some(tx) = &self.tx {
            let _ = tx.send(Msg::Set(p));
        }
    }
}

fn connect(id: &str) -> Option<std::fs::File> {
    for i in 0..10 {
        let path = format!(r"\\.\pipe\discord-ipc-{i}");
        let Ok(mut f) = std::fs::OpenOptions::new().read(true).write(true).open(&path) else { continue };
        let hello = json!({ "v": 1, "client_id": id });
        if send(&mut f, 0, &hello).is_ok() {
            if let Ok((op, body)) = recv(&mut f) {
                if op == 1 && body.contains("READY") {
                    log::info!("discord: connected");
                    return Some(f);
                }
                log::warn!("discord: handshake refused: {body}");
            }
        }
    }
    None
}

fn send(f: &mut std::fs::File, op: u32, v: &serde_json::Value) -> std::io::Result<()> {
    let body = v.to_string();
    let mut buf = Vec::with_capacity(8 + body.len());
    buf.extend_from_slice(&op.to_le_bytes());
    buf.extend_from_slice(&(body.len() as u32).to_le_bytes());
    buf.extend_from_slice(body.as_bytes());
    f.write_all(&buf)
}

fn recv(f: &mut std::fs::File) -> std::io::Result<(u32, String)> {
    let mut head = [0u8; 8];
    f.read_exact(&mut head)?;
    let op = u32::from_le_bytes(head[0..4].try_into().unwrap());
    let len = u32::from_le_bytes(head[4..8].try_into().unwrap()) as usize;
    let mut body = vec![0u8; len.min(1 << 20)];
    f.read_exact(&mut body)?;
    if op == 2 {
        return Err(std::io::Error::new(std::io::ErrorKind::ConnectionAborted, "closed"));
    }
    Ok((op, String::from_utf8_lossy(&body).into_owned()))
}
