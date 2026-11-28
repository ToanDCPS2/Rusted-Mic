//! Flixel / OpenFL fragment shaders (`FlxRuntimeShader`, `FlxShader`,
//! `RuntimePostEffectShader`) turned into Vulkan-style GLSL 450 that wgpu's
//! GLSL front end accepts, plus the uniform layout needed to fill them.
//!
//! What changes from the game's GLSL ES source:
//! - `#pragma header` / `#pragma body` become Flixel's header and body
//!   (`flixel_texture2D`, the `openfl_*` varyings), written against our vertex outputs.
//! - Plain uniforms move into one std140 block (`set 1, binding 1`), at the place
//!   the last one was declared; `bool`s are stored as `int`.
//! - `sampler2D` uniforms become texture bindings, and every `sampler2D` name
//!   becomes an `int` id: `texture2D(id, uv)` calls `_rm_tex`, which picks the
//!   texture. Sampling uses an explicit LOD so it is allowed in any control flow.
//! - Textures are premultiplied on read, as OpenFL's are, and the output is
//!   premultiplied too.

use regex::Regex;
use std::sync::LazyLock;
use wgpu::naga;

/// Scalar kind of a uniform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Float,
    Int,
    Uint,
    /// Stored as an int (0 / 1).
    Bool,
}

/// One settable uniform (an element of an array or struct is its own entry,
/// named like `lights[0].position`; arrays also get an entry for the whole).
#[derive(Debug, Clone)]
pub struct Uniform {
    pub name: String,
    pub offset: u32,
    pub kind: Kind,
    /// Components per column, and columns (1 for scalars and vectors).
    pub rows: u32,
    pub cols: u32,
    pub col_stride: u32,
    /// Array elements (1 if not an array) and their stride.
    pub count: u32,
    pub stride: u32,
}

impl Uniform {
    fn elem_len(&self) -> usize {
        (self.rows * self.cols) as usize
    }

    /// Write `values` (column-major per element, elements in order) into `buf`.
    pub fn write(&self, buf: &mut [u8], values: &[f64]) {
        let n = self.elem_len();
        for (e, chunk) in values.chunks(n).take(self.count as usize).enumerate() {
            for (i, v) in chunk.iter().enumerate() {
                let (c, r) = (i as u32 / self.rows, i as u32 % self.rows);
                let at = (self.offset + e as u32 * self.stride + c * self.col_stride + r * 4) as usize;
                let Some(dst) = buf.get_mut(at..at + 4) else { continue };
                let bytes = match self.kind {
                    Kind::Float => (*v as f32).to_le_bytes(),
                    Kind::Int | Kind::Bool => (*v as i32).to_le_bytes(),
                    Kind::Uint => (*v as u32).to_le_bytes(),
                };
                dst.copy_from_slice(&bytes);
            }
        }
    }
}

/// A translated shader.
#[derive(Debug, Clone)]
pub struct Translated {
    pub source: String,
    /// Sampler uniforms by id; 0 is always `bitmap`.
    pub samplers: Vec<String>,
}

/// The shader once parsed: its uniforms and their block sizes.
#[derive(Debug, Clone)]
pub struct Layout {
    pub uniforms: Vec<Uniform>,
    pub user_size: u32,
}

impl Layout {
    pub fn find(&self, name: &str) -> Option<&Uniform> {
        self.uniforms.iter().find(|u| u.name == name)
    }
}

/// Built-in uniforms (`set 1, binding 0`): `openfl_TextureSize`, then for post
/// effects `uScreenResolution`, `uCameraBounds`, `uFrameBounds`.
pub const BUILTIN_TEXTURE_SIZE: u32 = 0;
pub const BUILTIN_SCREEN: u32 = 8;
pub const BUILTIN_CAMERA: u32 = 16;
pub const BUILTIN_FRAME: u32 = 32;
pub const BUILTIN_SIZE: u32 = 48;

/// `RuntimePostEffectShader`'s fragment header (its functions; the uniforms
/// are in the built-in block).
const POST_FUNCS: &str = r#"
vec2 screenToWorld(vec2 screenCoord) {
    float left = uCameraBounds.x;
    float top = uCameraBounds.y;
    float right = uCameraBounds.z;
    float bottom = uCameraBounds.w;
    vec2 scale = vec2(right - left, bottom - top);
    vec2 offset = vec2(left, top);
    return screenCoord * scale + offset;
}
vec2 worldToScreen(vec2 worldCoord) {
    float left = uCameraBounds.x;
    float top = uCameraBounds.y;
    float right = uCameraBounds.z;
    float bottom = uCameraBounds.w;
    vec2 scale = vec2(right - left, bottom - top);
    vec2 offset = vec2(left, top);
    return (worldCoord - offset) / scale;
}
vec2 screenToFrame(vec2 screenCoord) {
    float left = uFrameBounds.x;
    float top = uFrameBounds.y;
    float right = uFrameBounds.z;
    float bottom = uFrameBounds.w;
    float width = right - left;
    float height = bottom - top;
    float clampedX = clamp(screenCoord.x, left, right);
    float clampedY = clamp(screenCoord.y, top, bottom);
    return vec2((clampedX - left) / (width), (clampedY - top) / (height));
}
vec2 bitmapCoordScale() {
    return openfl_TextureCoordv / screenCoord;
}
vec2 screenToBitmap(vec2 screenCoord) {
    return screenCoord * bitmapCoordScale();
}
vec4 sampleBitmapScreen(vec2 screenCoord) {
    return texture2D(bitmap, screenToBitmap(screenCoord));
}
vec4 sampleBitmapWorld(vec2 worldCoord) {
    return sampleBitmapScreen(worldToScreen(worldCoord));
}
"#;

static RE_VERSION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^[ \t]*#[ \t]*version[^\n]*").unwrap());
static RE_PRECISION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bprecision\s+(lowp|mediump|highp)\s+\w+\s*;").unwrap());
static RE_ALIAS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^[ \t]*#[ \t]*define[ \t]+(\w+)[ \t]+uniform[ \t]*$").unwrap());
static RE_UNIFORM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\buniform\s+(?:(?:lowp|mediump|highp)\s+)?(\w+)\s+(\w+)\s*(\[[^\]]*\])?\s*;").unwrap());
static RE_SAMPLER_TYPE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bsampler2D\b").unwrap());
static RE_TEXCALL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(texture2D|texture)\s*\(").unwrap());
static RE_PRAGMA_HEADER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^[ \t]*#[ \t]*pragma[ \t]+header[^\n]*").unwrap());
static RE_PRAGMA_BODY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^[ \t]*#[ \t]*pragma[ \t]+body[^\n]*").unwrap());

/// Strip comments (keeping line breaks) so declarations inside them are ignored.
fn strip_comments(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'/' && b.get(i + 1) == Some(&b'/') {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else if b[i] == b'/' && b.get(i + 1) == Some(&b'*') {
            i += 2;
            while i < b.len() && !(b[i] == b'*' && b.get(i + 1) == Some(&b'/')) {
                if b[i] == b'\n' {
                    out.push('\n');
                }
                i += 1;
            }
            i += 2;
        } else {
            let ch = src[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

/// Translate a Flixel fragment shader. `post`: a `RuntimePostEffectShader`
/// (adds `screenCoord` helpers and the camera/frame uniforms).
pub fn translate(src: &str, post: bool) -> Translated {
    let mut s = strip_comments(&src.replace("\r\n", "\n"));
    s = RE_VERSION.replace_all(&s, "").into_owned();
    s = RE_PRECISION.replace_all(&s, "").into_owned();
    // `#define UNIFORM uniform` (rain.frag): use the keyword itself.
    let aliases: Vec<String> = RE_ALIAS.captures_iter(&s).map(|c| c[1].to_owned()).collect();
    s = RE_ALIAS.replace_all(&s, "").into_owned();
    for a in aliases {
        s = Regex::new(&format!(r"\b{}\b", regex::escape(&a))).unwrap().replace_all(&s, "uniform").into_owned();
    }
    s = RE_PRAGMA_BODY.replace_all(&s, "gl_FragColor = flixel_texture2D(bitmap, openfl_TextureCoordv);").into_owned();
    // The header goes where `#pragma header` was (or the top).
    let had_header = RE_PRAGMA_HEADER.is_match(&s);
    s = RE_PRAGMA_HEADER.replace(&s, "\u{1}HEADER\u{1}").into_owned();
    s = RE_PRAGMA_HEADER.replace_all(&s, "").into_owned();
    if !had_header {
        s = format!("\u{1}HEADER\u{1}\n{s}");
    }
    if post {
        s = s.replacen("\u{1}HEADER\u{1}", &format!("\u{1}HEADER\u{1}\n{POST_FUNCS}"), 1);
    }

    // Uniform declarations.
    let mut samplers = vec!["bitmap".to_owned()];
    let mut members = Vec::new();
    let mut bool_defs = Vec::new();
    let mut out = String::with_capacity(s.len());
    let mut last = 0;
    let mut block_at = None;
    for c in RE_UNIFORM.captures_iter(&s) {
        let m = c.get(0).unwrap();
        out.push_str(&s[last..m.start()]);
        last = m.end();
        let (ty, name, arr) = (&c[1], &c[2], c.get(3).map(|a| a.as_str()).unwrap_or(""));
        match ty {
            "sampler2D" | "samplerCube" => {
                if name != "bitmap" {
                    out.push_str(&format!("const int {name} = {};", samplers.len()));
                    samplers.push(name.to_owned());
                }
            }
            "bool" => {
                members.push(format!("int _rm_b_{name}{arr};"));
                bool_defs.push(format!("#define {name} (_rm_b_{name} != 0)"));
            }
            "bvec2" | "bvec3" | "bvec4" => {
                let n = &ty[1..];
                members.push(format!("i{n} _rm_b_{name}{arr};"));
                bool_defs.push(format!("#define {name} b{n}(_rm_b_{name})"));
            }
            _ => members.push(format!("{ty} {name}{arr};")),
        }
        // The block goes where the last plain uniform was.
        if !matches!(ty, "sampler2D" | "samplerCube") {
            block_at = Some(out.len());
        }
    }
    out.push_str(&s[last..]);
    if members.is_empty() {
        members.push("float _rm_pad;".to_owned());
    }
    let block = format!("\nlayout(set = 1, binding = 1, std140) uniform _RmUser {{\n    {}\n}};\n{}\n", members.join("\n    "), bool_defs.join("\n"));
    let mut s = out;
    match block_at {
        Some(at) => s.insert_str(at, &block),
        None => s = s.replacen("\u{1}HEADER\u{1}", &format!("\u{1}HEADER\u{1}{block}"), 1),
    }

    // Samplers are ids now; texture reads go through `_rm_tex`.
    s = RE_SAMPLER_TYPE.replace_all(&s, "int").into_owned();
    s = RE_TEXCALL.replace_all(&s, "_rm_tex(").into_owned();

    let mut header = String::from(
        "layout(location = 0) in vec2 openfl_TextureCoordv;\n\
         layout(location = 1) in vec4 _rm_color;\n\
         layout(location = 2) in vec2 screenCoord;\n\
         layout(location = 0) out vec4 _rm_frag;\n\
         #define gl_FragColor _rm_frag\n\
         #define openfl_Alphav (_rm_color.a)\n\
         #define openfl_ColorMultiplierv vec4(_rm_color.rgb, 1.0)\n\
         #define openfl_ColorOffsetv vec4(0.0)\n\
         #define openfl_HasColorTransform true\n\
         #define hasTransform true\n\
         #define hasColorTransform true\n\
         layout(set = 0, binding = 0) uniform texture2D _rm_t0;\n\
         layout(set = 0, binding = 1) uniform sampler _rm_smp;\n",
    );
    header.push_str(if post {
        "layout(set = 1, binding = 0, std140) uniform _RmBuiltin {\n    vec2 openfl_TextureSize;\n    vec2 uScreenResolution;\n    vec4 uCameraBounds;\n    vec4 uFrameBounds;\n};\n"
    } else {
        "layout(set = 1, binding = 0, std140) uniform _RmBuiltin {\n    vec2 openfl_TextureSize;\n};\n"
    });
    for i in 1..samplers.len() {
        header.push_str(&format!("layout(set = 1, binding = {}) uniform texture2D _rm_t{i};\n", i + 1));
    }
    header.push_str("const int bitmap = 0;\nvec4 _rm_tex(int id, vec2 uv) {\n    vec4 c;\n");
    for i in 1..samplers.len() {
        header.push_str(&format!("    if (id == {i}) c = textureLod(sampler2D(_rm_t{i}, _rm_smp), uv, 0.0); else\n"));
    }
    header.push_str(
        "    c = textureLod(sampler2D(_rm_t0, _rm_smp), uv, 0.0);\n    return vec4(c.rgb * c.a, c.a);\n}\n\
         vec4 _rm_tex(int id, vec2 uv, float bias) { return _rm_tex(id, uv); }\n\
         vec4 flixel_texture2D(int id, vec2 coord) {\n\
         \x20   vec4 color = _rm_tex(id, coord);\n\
         \x20   if (color.a == 0.0) return vec4(0.0);\n\
         \x20   color = vec4(color.rgb / color.a, color.a);\n\
         \x20   color = clamp(openfl_ColorOffsetv + color * openfl_ColorMultiplierv, 0.0, 1.0);\n\
         \x20   if (color.a > 0.0) return vec4(color.rgb * color.a * openfl_Alphav, color.a * openfl_Alphav);\n\
         \x20   return vec4(0.0);\n}\n",
    );
    let s = s.replacen("\u{1}HEADER\u{1}", &header, 1);
    Translated { source: format!("#version 450\n{s}"), samplers }
}

/// Parse and validate a translated shader, returning its uniform layout.
pub fn reflect(source: &str) -> Result<(naga::Module, Layout), String> {
    let module = naga::front::glsl::Frontend::default()
        .parse(&naga::front::glsl::Options::from(naga::ShaderStage::Fragment), source)
        .map_err(|e| e.emit_to_string(source))?;
    naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all())
        .validate(&module)
        .map_err(|e| e.emit_to_string(source))?;
    let mut layout = Layout { uniforms: Vec::new(), user_size: 16 };
    for (_, var) in module.global_variables.iter() {
        let Some(b) = &var.binding else { continue };
        if b.group != 1 || b.binding != 1 {
            continue;
        }
        let ty = &module.types[var.ty];
        layout.user_size = ty.inner.size(module.to_ctx()).max(16).next_multiple_of(16);
        if let naga::TypeInner::Struct { members, .. } = &ty.inner {
            for m in members {
                let name = m.name.clone().unwrap_or_default();
                let name = name.strip_prefix("_rm_b_").map(str::to_owned).unwrap_or(name);
                let is_bool = m.name.as_deref().is_some_and(|n| n.starts_with("_rm_b_"));
                flatten(&module, m.ty, &name, m.offset, is_bool, &mut layout.uniforms);
            }
        }
    }
    Ok((module, layout))
}

fn scalar_kind(s: naga::Scalar, is_bool: bool) -> Kind {
    match s.kind {
        _ if is_bool => Kind::Bool,
        naga::ScalarKind::Sint => Kind::Int,
        naga::ScalarKind::Uint => Kind::Uint,
        naga::ScalarKind::Bool => Kind::Bool,
        _ => Kind::Float,
    }
}

fn flatten(module: &naga::Module, ty: naga::Handle<naga::Type>, name: &str, offset: u32, is_bool: bool, out: &mut Vec<Uniform>) {
    let base = |kind, rows, cols, col_stride| Uniform { name: name.to_owned(), offset, kind, rows, cols, col_stride, count: 1, stride: 0 };
    match &module.types[ty].inner {
        naga::TypeInner::Scalar(s) => out.push(base(scalar_kind(*s, is_bool), 1, 1, 4)),
        naga::TypeInner::Vector { size, scalar } => out.push(base(scalar_kind(*scalar, is_bool), *size as u32, 1, 16)),
        naga::TypeInner::Matrix { columns, rows, scalar } => {
            let col_stride = if *rows as u32 == 2 { 8 } else { 16 };
            out.push(base(scalar_kind(*scalar, false), *rows as u32, *columns as u32, col_stride));
        }
        naga::TypeInner::Array { base: elem, size, stride } => {
            let count = match size {
                naga::ArraySize::Constant(n) => n.get(),
                _ => 1,
            };
            let before = out.len();
            for i in 0..count {
                flatten(module, *elem, &format!("{name}[{i}]"), offset + i * stride, is_bool, out);
            }
            // The whole array, for `setFloatArray(name, [...])`.
            if let Some(first) = out.get(before).cloned() {
                if !matches!(module.types[*elem].inner, naga::TypeInner::Struct { .. }) {
                    out.push(Uniform { name: name.to_owned(), count, stride: *stride, ..first });
                }
            }
        }
        naga::TypeInner::Struct { members, .. } => {
            for m in members {
                let n = m.name.clone().unwrap_or_default();
                flatten(module, m.ty, &format!("{name}.{n}"), offset + m.offset, false, out);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(name: &str, src: &str, post: bool) {
        let t = translate(src, post);
        if let Err(e) = reflect(&t.source) {
            panic!("{name}:\n{e}\n--- translated ---\n{}", t.source);
        }
    }

    #[test]
    fn simple_shader_translates() {
        let src = "#pragma header\nuniform float _amount;\nuniform bool on;\nuniform mat3 m;\nvoid main() {\n  vec4 c = flixel_texture2D(bitmap, openfl_TextureCoordv);\n  if (on) c.rgb = m * c.rgb * _amount;\n  gl_FragColor = c;\n}\n";
        let t = translate(src, false);
        let (_, l) = reflect(&t.source).unwrap_or_else(|e| panic!("{e}\n{}", t.source));
        assert!(l.find("_amount").is_some());
        assert_eq!(l.find("on").unwrap().kind, Kind::Bool);
        let m = l.find("m").unwrap();
        assert_eq!((m.rows, m.cols, m.col_stride), (3, 3, 16));
    }

    /// Every `.frag` in the base game (set RUSTED_MIC_TEST_ASSETS).
    #[test]
    #[ignore]
    fn game_shaders_translate() {
        let root = std::path::PathBuf::from(std::env::var("RUSTED_MIC_TEST_ASSETS").expect("RUSTED_MIC_TEST_ASSETS"));
        let mut n = 0;
        for e in std::fs::read_dir(root.join("shaders")).unwrap().flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "frag") {
                let src = std::fs::read_to_string(&p).unwrap();
                let post = p.file_stem().is_some_and(|s| s == "rain");
                check(&p.display().to_string(), &src, post);
                n += 1;
            }
        }
        for (name, src) in crate::gfx::builtin_shaders::ALL {
            check(name, src, false);
            n += 1;
        }
        assert!(n > 5);
    }
}
