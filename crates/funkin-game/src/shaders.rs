//! The engine's shader classes as scripts see them (`new AdjustColorShader()`,
//! `new FlxRuntimeShader(src)`, `RuntimeRainShader`, ...): each keeps its
//! uniform values the way the original's setters write them, and builds its
//! GPU program the first time something is drawn with it.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::gfx::builtin_shaders;
use crate::gfx::shader::{ShaderId, ShaderParams};
use crate::gfx::Gfx;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// `FlxRuntimeShader` (and script classes extending it).
    Runtime,
    /// `RuntimePostEffectShader`.
    PostEffect,
    AdjustColor,
    DropShadow,
    Rain,
    Sserafim,
    GaussianBlur,
    OverlayBlend,
    /// `WiggleEffectRuntime(speed, freq, amplitude, effectType)`.
    Wiggle,
}

/// Engine shader classes a script can make, with the class.
pub const CLASSES: [(&str, Class); 10] = [
    ("FlxRuntimeShader", Class::Runtime),
    ("FlxShader", Class::Runtime),
    ("RuntimePostEffectShader", Class::PostEffect),
    ("AdjustColorShader", Class::AdjustColor),
    ("DropShadowShader", Class::DropShadow),
    ("RuntimeRainShader", Class::Rain),
    ("SserafimShader", Class::Sserafim),
    ("GaussianBlurShader", Class::GaussianBlur),
    ("OverlayBlend", Class::OverlayBlend),
    ("WiggleEffectRuntime", Class::Wiggle),
];

pub fn class(name: &str) -> Option<Class> {
    CLASSES.iter().find(|(n, _)| *n == name).map(|(_, c)| *c)
}

pub struct ShaderInst {
    pub id: u64,
    pub class: Class,
    /// GLSL fragment source (None = it failed to load).
    src: Option<String>,
    post: bool,
    pub params: ShaderParams,
    /// The class's own properties (`hue`, `intensity`...), for reads and tweens.
    props: HashMap<String, f64>,
    /// `sampler2D` inputs still to load: (uniform, image file).
    pending: Vec<(String, PathBuf)>,
    /// Built program (`None` inside = it failed; logged once).
    program: Option<Option<ShaderId>>,
    /// `DropShadowShader.attachedSprite` / `updateFrameInfo`: the frame
    /// bounds follow whatever is drawn with it.
    pub auto_frame: bool,
}

fn hue_matrix(deg: f64) -> Vec<f64> {
    let h = deg.to_radians();
    let (s, c) = h.sin_cos();
    let (wr, wg, wb) = (0.299, 0.587, 0.114);
    vec![
        wr + (1.0 - wr) * c - wr * s, wg - wg * c - wg * s, wb - wb * c + (1.0 - wb) * s,
        wr - wr * c + 0.143 * s, wg + (1.0 - wg) * c + 0.140 * s, wb - wb * c - 0.283 * s,
        wr - wr * c - (1.0 - wr) * s, wg - wg * c + wg * s, wb + (1.0 - wb) * c + wb * s,
    ]
}

fn saturation_matrix(v: f64) -> Vec<f64> {
    let v = if v > 0.0 { v * 3.0 } else { v };
    let s = 1.0 + v / 100.0;
    let (lr, lg, lb) = (0.2126, 0.7152, 0.0722);
    let inv = 1.0 - s;
    vec![
        lr * inv + s, lg * inv, lb * inv,
        lr * inv, lg * inv + s, lb * inv,
        lr * inv, lg * inv, lb * inv + s,
    ]
}

fn contrast(v: f64) -> f64 {
    let mut v = 1.0 + v / 100.0;
    if v > 1.0 {
        v = ((0.00852259 * (4.76454 * (v - 1.0)).exp()) * 1.01 - 0.0086078159) * 10.0 + 1.0;
    }
    v
}

fn rgb(c: i64) -> Vec<f64> {
    let c = c as u32;
    vec![((c >> 16) & 255) as f64 / 255.0, ((c >> 8) & 255) as f64 / 255.0, (c & 255) as f64 / 255.0]
}

impl ShaderInst {
    /// A new instance of an engine class. `src` is the fragment source for
    /// the classes that take one; `frag` reads `shaders/<name>.frag`.
    pub fn new(id: u64, class: Class, src: Option<String>, frag: impl Fn(&str) -> Option<String>) -> Self {
        let src = match class {
            Class::Runtime | Class::PostEffect => src,
            Class::AdjustColor => frag("adjustColor"),
            Class::Rain => frag("rain"),
            Class::GaussianBlur => frag("gaussianBlur"),
            Class::Wiggle => frag("wiggle"),
            Class::DropShadow => Some(builtin_shaders::DROP_SHADOW_SHADER.to_owned()),
            Class::Sserafim => Some(builtin_shaders::SSERAFIM_SHADER.to_owned()),
            Class::OverlayBlend => Some(builtin_shaders::OVERLAY_BLEND_SHADER.to_owned()),
        };
        let post = matches!(class, Class::PostEffect | Class::Rain);
        let mut s = Self { id, class, src, post, params: ShaderParams::default(), props: HashMap::new(), pending: Vec::new(), program: None, auto_frame: false };
        // The constructors' defaults.
        let defaults: &[(&str, f64)] = match class {
            Class::AdjustColor => &[("hue", 0.0), ("saturation", 0.0), ("brightness", 0.0), ("contrast", 0.0)],
            Class::DropShadow => &[
                ("angle", 0.0), ("angleOffset", 0.0), ("strength", 1.0), ("distance", 15.0), ("threshold", 0.1),
                ("baseHue", 0.0), ("baseSaturation", 0.0), ("baseBrightness", 0.0), ("baseContrast", 0.0), ("useAltMask", 0.0),
            ],
            Class::Rain => &[("time", 1.0), ("spriteMode", 0.0), ("scale", 1.0), ("intensity", 0.5), ("puddleY", 0.0), ("puddleScaleY", 0.0), ("rainColor", 0xFF6680cc_u32 as f64), ("numLightsSwag", 0.0)],
            Class::Sserafim => &[
                ("darkenAmount", 0.0), ("pulseLightColor", 0xFFFFFFFF_u32 as f64), ("pulseLightStrength", 0.0), ("truckLightStrength", 0.0),
                ("isCharacter", 0.0), ("baseHue", 0.0), ("baseSaturation", 0.0), ("baseBrightness", 0.0), ("baseContrast", 0.0),
            ],
            Class::GaussianBlur => &[("amount", 1.0)],
            Class::Wiggle => &[("waveSpeed", 0.0), ("waveFrequency", 0.0), ("waveAmplitude", 0.0), ("effectType", 0.0), ("time", 0.0)],
            Class::OverlayBlend => &[("funnyX", 0.0), ("funnyY", 0.0)],
            Class::Runtime | Class::PostEffect => &[],
        };
        for &(k, v) in defaults {
            s.set_prop(k, v);
        }
        if post {
            s.params.set("uScreenResolution", vec![1280.0, 720.0]);
            s.params.set("uCameraBounds", vec![0.0, 0.0, 1280.0, 720.0]);
            s.params.set("uFrameBounds", vec![0.0, 0.0, 1280.0, 720.0]);
        }
        s
    }

    pub fn name(&self) -> &'static str {
        CLASSES.iter().find(|(_, c)| *c == self.class).map(|(n, _)| *n).unwrap_or("FlxShader")
    }

    fn set(&mut self, uniform: &str, v: Vec<f64>) {
        self.params.set(uniform, v);
    }

    /// A class property write (its setter). False = not one of the class's.
    pub fn set_prop(&mut self, name: &str, v: f64) -> bool {
        use Class::*;
        let known = match (self.class, name) {
            (AdjustColor, "hue") => {
                self.set("hueMatrix", hue_matrix(v));
                true
            }
            (AdjustColor, "saturation") => {
                self.set("saturationMatrix", saturation_matrix(v));
                true
            }
            (AdjustColor, "brightness") => {
                self.set("brightness", vec![v / 255.0]);
                true
            }
            (AdjustColor, "contrast") => {
                self.set("contrast", vec![contrast(v)]);
                true
            }
            (DropShadow, "baseHue") => {
                self.set("hueMatrix", hue_matrix(v));
                true
            }
            (DropShadow, "baseSaturation") => {
                self.set("saturationMatrix", saturation_matrix(v));
                true
            }
            (DropShadow, "baseBrightness") => {
                self.set("brightness", vec![v / 255.0]);
                true
            }
            (DropShadow, "baseContrast") => {
                self.set("contrast", vec![contrast(v)]);
                true
            }
            (DropShadow, "angle" | "angleOffset") => {
                self.props.insert(name.to_owned(), v);
                let a = (self.prop("angle") + self.prop("angleOffset")).to_radians();
                self.set("angCos", vec![a.cos()]);
                self.set("angSin", vec![a.sin()]);
                true
            }
            (DropShadow, "color") => {
                self.set("dropColor", rgb(v as i64));
                true
            }
            (DropShadow, "distance") => {
                self.set("dist", vec![v]);
                true
            }
            (DropShadow, "strength") => {
                self.set("str", vec![v]);
                true
            }
            (DropShadow, "threshold") => {
                self.set("thr", vec![v]);
                true
            }
            (DropShadow, "maskThreshold") => {
                self.set("thr2", vec![v]);
                true
            }
            (DropShadow, "useAltMask") => {
                self.set("useMask", vec![(v != 0.0) as i32 as f64]);
                true
            }
            (Sserafim, "darkenAmount") => {
                self.set("darkAmt", vec![v]);
                true
            }
            (Sserafim, "pulseLightColor") => {
                self.set("lightColor", rgb(v as i64));
                true
            }
            (Sserafim, "pulseLightStrength") => {
                self.set("pulseStrength", vec![v]);
                true
            }
            (Sserafim, "truckLightStrength") => {
                self.set("truckStrength", vec![v]);
                true
            }
            (Sserafim, "isCharacter") => {
                self.set("isChar", vec![(v != 0.0) as i32 as f64]);
                true
            }
            (Sserafim, "baseHue") => {
                self.set("hue", vec![v]);
                true
            }
            (Sserafim, "baseSaturation") => {
                self.set("saturation", vec![v]);
                true
            }
            (Sserafim, "baseBrightness") => {
                self.set("brightness", vec![v]);
                true
            }
            (Sserafim, "baseContrast") => {
                self.set("contrast", vec![v]);
                true
            }
            (Rain, "time") => {
                self.set("uTime", vec![v]);
                true
            }
            (Rain, "spriteMode") => {
                self.set("uSpriteMode", vec![(v != 0.0) as i32 as f64]);
                true
            }
            (Rain, "scale") => {
                self.set("uScale", vec![v]);
                true
            }
            (Rain, "intensity") => {
                self.set("uIntensity", vec![v]);
                true
            }
            (Rain, "puddleY") => {
                self.set("uPuddleY", vec![v]);
                true
            }
            (Rain, "puddleScaleY") => {
                self.set("uPuddleScaleY", vec![v]);
                true
            }
            (Rain, "rainColor") => {
                self.set("uRainColor", rgb(v as i64));
                true
            }
            (Rain, "numLightsSwag") => {
                self.set("numLights", vec![v.trunc()]);
                true
            }
            (OverlayBlend, "funnyX") => {
                self.set("xPos", vec![v]);
                true
            }
            (OverlayBlend, "funnyY") => {
                self.set("yPos", vec![v]);
                true
            }
            (Wiggle, "waveSpeed") => {
                self.set("uSpeed", vec![v]);
                true
            }
            (Wiggle, "waveFrequency") => {
                self.set("uFrequency", vec![v]);
                true
            }
            (Wiggle, "waveAmplitude") => {
                self.set("uWaveAmplitude", vec![v]);
                true
            }
            (Wiggle, "effectType") => {
                self.set("effectType", vec![v.trunc()]);
                true
            }
            (Wiggle, "time") => {
                self.set("uTime", vec![v]);
                true
            }
            (GaussianBlur, "amount") => {
                self.set("_amount", vec![v]);
                true
            }
            _ => false,
        };
        if known {
            self.props.insert(name.to_owned(), v);
        }
        known
    }

    pub fn prop(&self, name: &str) -> f64 {
        self.props.get(name).copied().unwrap_or(0.0)
    }

    pub fn has_prop(&self, name: &str) -> bool {
        self.props.contains_key(name)
    }

    /// `setFloat` / `setInt` / `setBool` / `setFloatArray` ...
    pub fn set_uniform(&mut self, name: &str, v: Vec<f64>) {
        self.set(name, v);
    }

    pub fn uniform(&self, name: &str) -> Option<&Vec<f64>> {
        self.params.get(name)
    }

    /// A `sampler2D` input from a loaded texture (`shader.name.input = sprite.pixels`).
    pub fn set_texture(&mut self, uniform: &str, t: crate::gfx::TexId) {
        self.pending.retain(|(n, _)| n != uniform);
        self.params.textures.insert(uniform.to_owned(), t);
    }

    /// `setSampler2D` / `setBitmapData` / `loadAltMask` with an image file.
    pub fn set_image(&mut self, uniform: &str, path: PathBuf) {
        self.pending.push((uniform.to_owned(), path));
    }

    /// `RuntimePostEffectShader.updateViewInfo`: the screen size and the
    /// camera's view rectangle (left, top, right, bottom) in the world.
    pub fn view_info(&mut self, w: f64, h: f64, view: [f64; 4]) {
        self.set("uScreenResolution", vec![w, h]);
        self.set("uCameraBounds", view.to_vec());
    }

    /// The program to draw with, built on first use.
    pub fn program(&mut self, gfx: &mut Gfx) -> Option<ShaderId> {
        for (name, path) in std::mem::take(&mut self.pending) {
            match gfx.load_texture(&path) {
                Ok(t) => {
                    self.params.textures.insert(name, t);
                }
                Err(e) => eprintln!("warning: {}: {e}", self.name()),
            }
        }
        if self.program.is_none() {
            let r = match &self.src {
                Some(src) => gfx.shader_program(src, self.post),
                None => Err("its fragment source didn't load".into()),
            };
            self.program = Some(match r {
                Ok(id) => Some(id),
                Err(e) => {
                    eprintln!("warning: {} couldn't be built, drawing without it: {e}", self.name());
                    None
                }
            });
        }
        self.program.flatten()
    }

    /// `DropShadowShader` follows the drawn frame's rotation with `angle`.
    pub fn drop_angle(&self) -> Option<f32> {
        (self.class == Class::DropShadow && self.auto_frame).then(|| self.prop("angle") as f32)
    }
}
