//! Minimal wgpu 2D renderer: textured, tinted quads batched by texture, drawn
//! into a fixed 1280x720 virtual canvas (the original's `FlxG.width/height`)
//! that is letterboxed to the window.
//!
//! Masks (Adobe Animate mask layers) use the stencil buffer: the mask's
//! shapes raise the stencil value, what they clip is drawn where it matches,
//! and the shapes are drawn again to lower it (so masks nest).

pub mod animate;
pub mod atlas;
pub mod builtin_shaders;
pub mod glsl;
pub mod shader;
pub mod text;
pub mod ttf;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;
use winit::window::Window;

pub const VIEW_W: f32 = 1280.0;
pub const VIEW_H: f32 = 720.0;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    pos: [f32; 2],
    uv: [f32; 2],
    color: [f32; 4],
    /// Spare per-vertex data (zero; kept so the vertex layout stays fixed).
    fx: [f32; 4],
}

/// The stencil format for masks.
const STENCIL: wgpu::TextureFormat = wgpu::TextureFormat::Stencil8;

/// The stencil state shared by every pipeline drawn in a pass with masks.
pub(crate) fn stencil_state(pass_op: wgpu::StencilOperation) -> wgpu::DepthStencilState {
    let face = wgpu::StencilFaceState {
        compare: wgpu::CompareFunction::Equal,
        fail_op: wgpu::StencilOperation::Keep,
        depth_fail_op: wgpu::StencilOperation::Keep,
        pass_op,
    };
    wgpu::DepthStencilState {
        format: STENCIL,
        depth_write_enabled: None,
        depth_compare: None,
        stencil: wgpu::StencilState { front: face, back: face, read_mask: 0xff, write_mask: 0xff },
        bias: Default::default(),
    }
}

/// Steps of drawing through a mask (see [`Gfx::mask`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaskOp {
    /// What is drawn next is the mask's shape.
    Begin,
    /// What is drawn next is clipped by that shape.
    Content,
    /// The mask's shape again, to remove it.
    End,
    /// Back to drawing normally.
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StencilMode {
    Draw,
    Raise,
    Lower,
}

#[derive(Debug, Clone, Copy)]
struct Batch {
    tex: TexId,
    blend: Blend,
    mode: StencilMode,
    /// Stencil reference value.
    level: u32,
    start: u32,
    end: u32,
    /// Index into this frame's shader uses ([`Gfx::set_shader`]).
    shader: Option<u32>,
}

/// Handle to a loaded texture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TexId(usize);

impl TexId {
    /// For handing a texture to scripts as a number (`sprite.pixels`).
    pub fn index(self) -> usize {
        self.0
    }
    pub fn from_index(i: usize) -> Self {
        TexId(i)
    }
}

struct GpuTexture {
    bind_group: wgpu::BindGroup,
    /// The same texture sampled nearest-neighbour (antialiasing off), for
    /// smooth textures.
    nearest_group: Option<wgpu::BindGroup>,
    view: wgpu::TextureView,
    width: u32,
    height: u32,
    /// The song scope that loaded it (0 = kept for the whole run).
    scope: usize,
}

/// RGBA tint, `[1, 1, 1, 1]` = unmodified.
pub type Color = [f32; 4];
pub const WHITE: Color = [1.0, 1.0, 1.0, 1.0];

/// Solid `color` in the shape of the texture's alpha (text recolouring).
pub fn fill(c: Color) -> Color {
    [c[0], c[1], c[2], c[3] + 2.0]
}

/// 2D affine transform `[a, b, c, d, tx, ty]`: `x' = a x + c y + tx`, `y' = b x + d y + ty`
/// (the layout of Flash/OpenFL `Matrix` and Adobe Animate `MX`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Affine(pub [f32; 6]);

impl Affine {
    pub const IDENTITY: Affine = Affine([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);

    pub fn translate(x: f32, y: f32) -> Self {
        Affine([1.0, 0.0, 0.0, 1.0, x, y])
    }
    pub fn scale(x: f32, y: f32) -> Self {
        Affine([x, 0.0, 0.0, y, 0.0, 0.0])
    }
    pub fn rotate_deg(deg: f32) -> Self {
        let (s, c) = deg.to_radians().sin_cos();
        Affine([c, s, -s, c, 0.0, 0.0])
    }
    /// Uniform zoom about `(cx, cy)`.
    pub fn zoom_about(z: f32, cx: f32, cy: f32) -> Self {
        Affine([z, 0.0, 0.0, z, cx - cx * z, cy - cy * z])
    }
    /// `self ∘ rhs`: apply `rhs` first, then `self`.
    pub fn then(&self, rhs: &Affine) -> Affine {
        let [a, b, c, d, tx, ty] = self.0;
        let [e, f, g, h, ux, uy] = rhs.0;
        Affine([
            a * e + c * f,
            b * e + d * f,
            a * g + c * h,
            b * g + d * h,
            a * ux + c * uy + tx,
            b * ux + d * uy + ty,
        ])
    }
    pub fn apply(&self, p: [f32; 2]) -> [f32; 2] {
        let [a, b, c, d, tx, ty] = self.0;
        [a * p[0] + c * p[1] + tx, b * p[0] + d * p[1] + ty]
    }
    pub fn is_identity(&self) -> bool {
        *self == Self::IDENTITY
    }
}

pub struct Gfx {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    /// One per `Blend` mode.
    pipelines: Vec<wgpu::RenderPipeline>,
    blend: Blend,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// Slots; `None` once released (and listed in `free`).
    textures: Vec<Option<GpuTexture>>,
    free: Vec<usize>,
    /// Scope new textures are tagged with (see [`Gfx::texture_mark`]).
    scope: usize,
    next_scope: usize,
    white: TexId,
    vertices: Vec<Vertex>,
    batches: Vec<Batch>,
    /// Pipelines that raise / lower the stencil (mask shapes, no colour).
    mask_pipelines: [wgpu::RenderPipeline; 2],
    stencil_mode: StencilMode,
    stencil_level: u32,
    /// Stencil buffers for the window and the camera-filter targets, with their sizes.
    stencil_main: Option<(u32, u32, wgpu::TextureView)>,
    stencil_post: Option<(u32, u32, wgpu::TextureView)>,
    vbuf: wgpu::Buffer,
    vbuf_cap: usize,
    nearest: wgpu::Sampler,
    /// Options > Antialiasing: off samples every texture nearest-neighbour.
    pub antialiasing: bool,
    cache: HashMap<(PathBuf, bool), TexId>,
    transform: Affine,
    present_modes: Vec<wgpu::PresentMode>,
    /// Read the next presented frame back (screenshots).
    capture: bool,
    captured: Option<(u32, u32, Vec<u8>)>,
    /// Script shader programs, and the sources they were built from.
    programs: Vec<shader::Program>,
    program_cache: HashMap<(String, bool), Result<shader::ShaderId, String>>,
    shader_vs: wgpu::ShaderModule,
    /// This frame's shader uses, and the one applied to what is queued next.
    frame_draws: Vec<shader::FrameDraw>,
    cur_shader: Option<u32>,
    /// Ping-pong targets for camera filters (size, view, group-0 bind group).
    post_targets: Vec<(u32, u32, wgpu::TextureView, wgpu::BindGroup)>,
    /// Multiplies the alpha of everything queued (a camera's `alpha`).
    pub alpha_mul: f32,
    /// Added to every colour drawn (`colorTransform` offsets, 0..1 per channel).
    pub color_offset: [f32; 4],
    /// This frame's camera filters: the batch count when [`Gfx::post_filters`]
    /// was called (earlier batches get them) and the shader uses, in order.
    post: Option<(usize, Vec<shader::FrameDraw>)>,
}

const SHADER: &str = r#"
struct VsOut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32>, @location(1) color: vec4<f32>, @location(2) fx: vec4<f32> };
@vertex fn vs(@location(0) pos: vec2<f32>, @location(1) uv: vec2<f32>, @location(2) color: vec4<f32>, @location(3) fx: vec4<f32>) -> VsOut {
    var o: VsOut;
    o.pos = vec4<f32>(pos.x / 640.0 - 1.0, 1.0 - pos.y / 360.0, 0.0, 1.0);
    o.uv = uv;
    o.color = color;
    o.fx = fx;
    return o;
}
@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var smp: sampler;
fn shade(i: VsOut) -> vec4<f32> {
    let t = textureSample(tex, smp, i.uv);
    // Alpha above 1.5 means "fill": the colour, shaped by the texture's alpha.
    if (i.color.a > 1.5) {
        return vec4<f32>(i.color.rgb, t.a * (i.color.a - 2.0));
    }
    // `fx`: colour offsets, added after the multipliers (OpenFL's ColorTransform);
    // fully transparent pixels stay transparent.
    let c = t * i.color;
    if (t.a <= 0.0) {
        return c;
    }
    return vec4<f32>(clamp(c.rgb + i.fx.rgb, vec3<f32>(0.0), vec3<f32>(1.0)), clamp(c.a + i.fx.a, 0.0, 1.0));
}
@fragment fn fs(i: VsOut) -> @location(0) vec4<f32> {
    return shade(i);
}
@fragment fn fs_premul(i: VsOut) -> @location(0) vec4<f32> {
    let c = shade(i);
    return vec4<f32>(c.rgb * c.a, c.a);
}
// Mask shapes: only where the texture is solid counts.
@fragment fn fs_mask(i: VsOut) -> @location(0) vec4<f32> {
    let t = textureSample(tex, smp, i.uv);
    if (t.a < 0.5) {
        discard;
    }
    return t;
}
"#;

/// OpenFL blend modes used by the game.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blend {
    Normal,
    Add,
    Multiply,
    /// `BlendMode.SCREEN` (as OpenFL draws it).
    Screen,
}

impl Gfx {
    pub fn new(window: Arc<Window>) -> Result<Self, String> {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let surface = instance
            .create_surface(window.clone())
            .map_err(|e| e.to_string())?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
            ..Default::default()
        }))
        .map_err(|e| format!("no GPU adapter: {e}"))?;
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
                .map_err(|e| e.to_string())?;

        let size = window.inner_size();
        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .ok_or("surface not supported by adapter")?;
        // Blend in gamma space like Flixel/OpenFL: use a non-sRGB swapchain format.
        config.format = config.format.remove_srgb_suffix();
        // Lowest-latency mode available: Mailbox (no tearing) → Immediate → Fifo.
        let caps = surface.get_capabilities(&adapter);
        config.present_mode = [wgpu::PresentMode::Mailbox, wgpu::PresentMode::Immediate]
            .into_iter()
            .find(|m| caps.present_modes.contains(m))
            .unwrap_or(wgpu::PresentMode::Fifo);
        config.desired_maximum_frame_latency = 1;
        if caps.usages.contains(wgpu::TextureUsages::COPY_SRC) {
            config.usage |= wgpu::TextureUsages::COPY_SRC;
        }
        let present_modes = caps.present_modes.clone();
        surface.configure(&device, &config);

        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: None,
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let make_with = |entry: &str, blend: wgpu::BlendState, op: wgpu::StencilOperation, writes: wgpu::ColorWrites| device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: None,
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4, 3 => Float32x4],
                })],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(stencil_state(op)),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(blend),
                    write_mask: writes,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let make = |entry: &str, blend: wgpu::BlendState| make_with(entry, blend, wgpu::StencilOperation::Keep, wgpu::ColorWrites::ALL);
        let comp = |src, dst| wgpu::BlendComponent { src_factor: src, dst_factor: dst, operation: wgpu::BlendOperation::Add };
        use wgpu::BlendFactor as F;
        let pipelines = vec![
            make("fs", wgpu::BlendState::ALPHA_BLENDING),
            make("fs", wgpu::BlendState { color: comp(F::SrcAlpha, F::One), alpha: comp(F::Zero, F::One) }),
            make("fs_premul", wgpu::BlendState { color: comp(F::Dst, F::OneMinusSrcAlpha), alpha: comp(F::Zero, F::One) }),
            // OpenFL's SCREEN: blendFunc(ONE, ONE_MINUS_SRC_COLOR) on premultiplied colour.
            make("fs_premul", wgpu::BlendState { color: comp(F::One, F::OneMinusSrc), alpha: comp(F::One, F::OneMinusSrcAlpha) }),
        ];
        let mask_pipelines = [
            make_with("fs_mask", wgpu::BlendState::REPLACE, wgpu::StencilOperation::IncrementClamp, wgpu::ColorWrites::empty()),
            make_with("fs_mask", wgpu::BlendState::REPLACE, wgpu::StencilOperation::DecrementClamp, wgpu::ColorWrites::empty()),
        ];
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let nearest = device.create_sampler(&wgpu::SamplerDescriptor::default());
        let vbuf_cap = 6 * 4096;
        let vbuf = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (vbuf_cap * std::mem::size_of::<Vertex>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let shader_vs = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("script shader vs"),
            source: wgpu::ShaderSource::Wgsl(shader::VERTEX.into()),
        });
        let mut gfx = Self {
            programs: Vec::new(),
            program_cache: HashMap::new(),
            shader_vs,
            frame_draws: Vec::new(),
            cur_shader: None,
            post_targets: Vec::new(),
            post: None,
            alpha_mul: 1.0,
            color_offset: [0.0; 4],
            window,
            surface,
            device,
            queue,
            config,
            pipelines,
            blend: Blend::Normal,
            layout,
            sampler,
            textures: Vec::new(),
            free: Vec::new(),
            scope: 0,
            next_scope: 0,
            white: TexId(0),
            vertices: Vec::new(),
            batches: Vec::new(),
            mask_pipelines,
            stencil_mode: StencilMode::Draw,
            stencil_level: 0,
            stencil_main: None,
            stencil_post: None,
            vbuf,
            vbuf_cap,
            nearest,
            antialiasing: true,
            cache: HashMap::new(),
            transform: Affine::IDENTITY,
            present_modes,
            capture: false,
            captured: None,
        };
        gfx.white = gfx.upload_rgba(1, 1, &[255, 255, 255, 255]);
        Ok(gfx)
    }

    /// 1x1 white texture (solid fills).
    pub fn white(&self) -> TexId {
        self.white
    }

    pub fn window(&self) -> &Window {
        &self.window
    }

    /// VSync off (lowest latency), on (Fifo) or adaptive (FifoRelaxed).
    pub fn set_vsync(&mut self, mode: crate::settings::VSync) {
        use crate::settings::VSync;
        use wgpu::PresentMode as P;
        let wanted: &[P] = match mode {
            VSync::Off => &[P::Mailbox, P::Immediate],
            VSync::On => &[P::Fifo],
            VSync::Adaptive => &[P::FifoRelaxed, P::Fifo],
        };
        let m = wanted.iter().copied().find(|m| self.present_modes.contains(m)).unwrap_or(P::Fifo);
        if self.config.present_mode != m {
            self.config.present_mode = m;
            self.surface.configure(&self.device, &self.config);
        }
    }

    /// Capture the next presented frame; collect it with [`Gfx::take_capture`].
    pub fn request_capture(&mut self) {
        self.capture = true;
    }

    /// The captured frame (the 16:9 canvas only), as RGBA.
    pub fn take_capture(&mut self) -> Option<(u32, u32, Vec<u8>)> {
        self.captured.take()
    }

    /// Replace a texture's pixels (and size) in place.
    pub fn replace_rgba(&mut self, tex: TexId, width: u32, height: u32, rgba: &[u8]) {
        let new = self.upload_rgba(width, height, rgba);
        let t = self.textures[new.0].take();
        self.free.push(new.0);
        if let Some(mut t) = t {
            t.scope = self.textures[tex.0].as_ref().map_or(0, |o| o.scope);
            self.textures[tex.0] = Some(t);
        }
    }

    pub fn resize(&mut self, w: u32, h: u32) {
        if w > 0 && h > 0 {
            self.config.width = w;
            self.config.height = h;
            self.surface.configure(&self.device, &self.config);
        }
    }

    pub fn upload_rgba(&mut self, width: u32, height: u32, rgba: &[u8]) -> TexId {
        self.upload_rgba_filtered(width, height, rgba, true)
    }

    fn upload_rgba_filtered(&mut self, width: u32, height: u32, rgba: &[u8], smooth: bool) -> TexId {
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let texture = self.device.create_texture_with_data(
            &self.queue,
            &wgpu::TextureDescriptor {
                label: None,
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            rgba,
        );
        let view = texture.create_view(&Default::default());
        let group = |s: &wgpu::Sampler| {
            self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(s) },
                ],
            })
        };
        let bind_group = group(if smooth { &self.sampler } else { &self.nearest });
        let nearest_group = smooth.then(|| group(&self.nearest));
        let t = Some(GpuTexture { bind_group, nearest_group, view, width, height, scope: self.scope });
        match self.free.pop() {
            Some(i) => {
                self.textures[i] = t;
                TexId(i)
            }
            None => {
                self.textures.push(t);
                TexId(self.textures.len() - 1)
            }
        }
    }

    /// Load a PNG from disk (cached by path).
    pub fn load_texture(&mut self, path: &Path) -> Result<TexId, String> {
        self.load_texture_filtered(path, true)
    }

    /// Load a PNG; `smooth = false` samples nearest-neighbour (pixel-art stages).
    pub fn load_texture_filtered(&mut self, path: &Path, smooth: bool) -> Result<TexId, String> {
        let key = (path.to_path_buf(), smooth);
        if let Some(&t) = self.cache.get(&key) {
            return Ok(t);
        }
        let bytes = funkin_core::vfs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let img = image::load_from_memory(&bytes)
            .map_err(|e| format!("{}: {e}", path.display()))?
            .to_rgba8();
        let (w, h) = img.dimensions();
        let t = self.upload_rgba_filtered(w, h, &img, smooth);
        self.cache.insert(key, t);
        Ok(t)
    }

    /// Start a scope: textures loaded from now until [`Gfx::end_scope`] belong
    /// to it and are freed together by [`Gfx::release_textures`] (per-song
    /// stage and character art). Things created lazily later, like font
    /// glyphs, stay out of it.
    pub fn texture_mark(&mut self) -> usize {
        self.next_scope += 1;
        self.scope = self.next_scope;
        self.scope
    }

    /// Run `f` with new textures tagged with `scope` (a song's, for art its
    /// scripts load mid-song), then restore the open scope.
    pub fn with_scope<T>(&mut self, scope: usize, f: impl FnOnce(&mut Self) -> T) -> T {
        let prev = std::mem::replace(&mut self.scope, scope);
        let r = f(self);
        self.scope = prev;
        r
    }

    /// Close the open scope, returning it (0 if none was open).
    pub fn end_scope(&mut self) -> usize {
        std::mem::take(&mut self.scope)
    }

    /// Free every texture of a scope from [`Gfx::texture_mark`].
    pub fn release_textures(&mut self, scope: usize) {
        if scope == 0 {
            return;
        }
        if self.scope == scope {
            self.scope = 0;
        }
        for (i, t) in self.textures.iter_mut().enumerate() {
            if t.as_ref().is_some_and(|t| t.scope == scope) {
                *t = None;
                self.free.push(i);
            }
        }
        let textures = &self.textures;
        self.cache.retain(|_, t| textures[t.0].is_some());
    }

    /// Transform applied to everything queued from now on (camera / HUD zoom).
    pub fn set_transform(&mut self, m: Affine) {
        self.transform = m;
    }

    pub fn texture_size(&self, t: TexId) -> (f32, f32) {
        match self.textures.get(t.0).and_then(Option::as_ref) {
            Some(g) => (g.width as f32, g.height as f32),
            None => (1.0, 1.0),
        }
    }

    /// Queue a quad. `src` is a pixel rect in the texture, `dst` the four
    /// corners on screen (top-left, top-right, bottom-right, bottom-left).
    pub fn quad(&mut self, tex: TexId, src: [f32; 4], dst: [[f32; 2]; 4], color: Color) {
        let (tw, th) = self.texture_size(tex);
        let (u0, v0, u1, v1) = (
            src[0] / tw,
            src[1] / th,
            (src[0] + src[2]) / tw,
            (src[1] + src[3]) / th,
        );
        let uv = [[u0, v0], [u1, v0], [u1, v1], [u0, v1]];
        self.quad_uv(tex, uv, dst, color);
    }

    /// Queue a quad with explicit per-corner UVs (used for rotated atlas frames).
    pub fn quad_uv(&mut self, tex: TexId, uv: [[f32; 2]; 4], dst: [[f32; 2]; 4], color: Color) {
        let start = self.vertices.len() as u32;
        let dst = if self.transform.is_identity() {
            dst
        } else {
            dst.map(|p| self.transform.apply(p))
        };
        let color = [color[0], color[1], color[2], color[3] * self.alpha_mul];
        for i in [0, 1, 2, 0, 2, 3] {
            self.vertices.push(Vertex {
                pos: dst[i],
                uv: uv[i],
                color,
                fx: self.color_offset,
            });
        }
        if let Some(i) = self.cur_shader {
            let d = &self.frame_draws[i as usize];
            if d.auto.on {
                let (mut l, mut t, mut r, mut b) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
                for q in &uv {
                    (l, t, r, b) = (l.min(q[0]), t.min(q[1]), r.max(q[0]), b.max(q[1]));
                }
                // A rotated atlas frame runs its texture down the quad's top edge.
                let rotated = (uv[1][1] - uv[0][1]).abs() > (uv[1][0] - uv[0][0]).abs();
                let key = ([l, t, r, b], if rotated { -90.0 } else { 0.0 });
                if d.frame_key != Some(key) {
                    let nd = d.with_frame(&self.programs[d.prog.0], key.0, key.1);
                    self.frame_draws.push(nd);
                    self.cur_shader = Some(self.frame_draws.len() as u32 - 1);
                }
            }
        }
        let (blend, mode, level) = (self.blend, self.stencil_mode, self.stencil_level);
        let shader = if mode == StencilMode::Draw { self.cur_shader } else { None };
        // Never merge across the post-effect split.
        let split = self.post.as_ref().map(|(k, _)| *k == self.batches.len()).unwrap_or(false);
        match self.batches.last_mut() {
            Some(b) if b.tex == tex && b.blend == blend && b.mode == mode && b.level == level && b.shader == shader && !split => b.end = start + 6,
            _ => self.batches.push(Batch { tex, blend, mode, level, start, end: start + 6, shader }),
        }
    }

    /// Blend mode for what is queued next.
    pub fn set_blend(&mut self, b: Blend) {
        self.blend = b;
    }

    /// Draw through a mask: [`MaskOp::Begin`], the mask's shapes,
    /// [`MaskOp::Content`], what it clips, [`MaskOp::End`], the same shapes
    /// again, [`MaskOp::Done`]. Masks nest.
    pub fn mask(&mut self, op: MaskOp) {
        match op {
            MaskOp::Begin => self.stencil_mode = StencilMode::Raise,
            MaskOp::Content => {
                self.stencil_level += 1;
                self.stencil_mode = StencilMode::Draw;
            }
            MaskOp::End => self.stencil_mode = StencilMode::Lower,
            MaskOp::Done => {
                self.stencil_level = self.stencil_level.saturating_sub(1);
                self.stencil_mode = StencilMode::Draw;
            }
        }
    }

    /// A stencil buffer of `w`×`h`, reusing `slot` when the size matches.
    fn stencil_view(device: &wgpu::Device, slot: &mut Option<(u32, u32, wgpu::TextureView)>, w: u32, h: u32) -> wgpu::TextureView {
        if !matches!(slot, Some((sw, sh, _)) if *sw == w && *sh == h) {
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("stencil"),
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: STENCIL,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            *slot = Some((w, h, tex.create_view(&Default::default())));
        }
        slot.as_ref().map(|(_, _, v)| v.clone()).expect("stencil view")
    }

    /// Axis-aligned textured rectangle.
    #[allow(clippy::too_many_arguments)]
    pub fn image(
        &mut self,
        tex: TexId,
        src: [f32; 4],
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        color: Color,
    ) {
        self.quad(
            tex,
            src,
            [[x, y], [x + w, y], [x + w, y + h], [x, y + h]],
            color,
        );
    }

    /// Whole texture at `(x, y)` scaled by `scale`.
    pub fn sprite(&mut self, tex: TexId, x: f32, y: f32, scale: f32, color: Color) {
        let (w, h) = self.texture_size(tex);
        self.image(tex, [0.0, 0.0, w, h], x, y, w * scale, h * scale, color);
    }

    /// Solid rectangle.
    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: Color) {
        let white = self.white;
        self.image(white, [0.0, 0.0, 1.0, 1.0], x, y, w, h, color);
    }

    /// Build (or fetch) a script shader from its GLSL fragment source.
    /// `post` = used as a camera filter (`RuntimePostEffectShader`'s extras).
    pub fn shader_program(&mut self, src: &str, post: bool) -> Result<shader::ShaderId, String> {
        let key = (src.to_owned(), post);
        if let Some(r) = self.program_cache.get(&key) {
            return r.clone();
        }
        let r = shader::compile(&self.device, self.config.format, &self.layout, &self.shader_vs, src, post).map(|p| {
            self.programs.push(p);
            shader::ShaderId(self.programs.len() - 1)
        });
        self.program_cache.insert(key, r.clone());
        r
    }

    /// Draw what is queued next with a script shader (`sprite.shader`), or
    /// back to the plain sprite shader with `None`.
    pub fn set_shader(&mut self, s: Option<(shader::ShaderId, &shader::ShaderParams, shader::AutoFrame)>) {
        self.cur_shader = s.and_then(|(id, params, auto)| {
            let p = self.programs.get(id.0)?;
            self.frame_draws.push(shader::FrameDraw::new(id, p, params, auto));
            Some(self.frame_draws.len() as u32 - 1)
        });
    }

    /// Run everything drawn so far this frame (the game camera) through
    /// these filters, in order; what is drawn afterwards (the HUD) is not.
    pub fn post_filters(&mut self, chain: &[(shader::ShaderId, &shader::ShaderParams)]) {
        let draws: Vec<_> = chain
            .iter()
            .filter_map(|(id, params)| Some(shader::FrameDraw::new(*id, self.programs.get(id.0)?, params, Default::default())))
            .collect();
        if !draws.is_empty() {
            self.post = Some((self.batches.len(), draws));
        }
    }

    /// Make sure camera-filter target `i` is `w`×`h`.
    fn post_target(&mut self, i: usize, w: u32, h: u32) {
        while self.post_targets.len() <= i {
            let t = self.make_target(1, 1);
            self.post_targets.push(t);
        }
        if self.post_targets[i].0 != w || self.post_targets[i].1 != h {
            self.post_targets[i] = self.make_target(w, h);
        }
    }

    fn make_target(&self, w: u32, h: u32) -> (u32, u32, wgpu::TextureView, wgpu::BindGroup) {
        let tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("camera filter"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.config.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = tex.create_view(&Default::default());
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        });
        (w, h, view, bind_group)
    }

    /// Group-1 bind group for one shader use: its uniform blocks (at `offs`
    /// in `ubuf`) and extra textures (`own` = the drawn texture itself).
    fn shader_group(&self, d: &shader::FrameDraw, offs: [u64; 2], ubuf: &wgpu::Buffer, own: &wgpu::TextureView) -> wgpu::BindGroup {
        let p = &self.programs[d.prog.0];
        let sizes = [glsl::BUILTIN_SIZE as u64, uniform_len(d.user.len())];
        let views: Vec<&wgpu::TextureView> = d
            .textures
            .iter()
            .map(|t| t.and_then(|t| self.textures.get(t.0).and_then(Option::as_ref)).map(|g| &g.view).unwrap_or(own))
            .collect();
        let mut entries = vec![];
        for i in 0..2 {
            entries.push(wgpu::BindGroupEntry {
                binding: i as u32,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: ubuf,
                    offset: offs[i],
                    size: std::num::NonZeroU64::new(sizes[i]),
                }),
            });
        }
        for (i, v) in views.iter().enumerate() {
            entries.push(wgpu::BindGroupEntry { binding: i as u32 + 2, resource: wgpu::BindingResource::TextureView(v) });
        }
        self.device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &p.group, entries: &entries })
    }

    fn draw_batches(&self, pass: &mut wgpu::RenderPass<'_>, batches: &[Batch], groups: &[Option<(usize, wgpu::BindGroup)>]) {
        for (b, g) in batches.iter().zip(groups) {
            let Batch { tex, blend, mode, level, start, end, .. } = *b;
            match (mode, g) {
                (StencilMode::Draw, Some((prog, group))) => {
                    pass.set_pipeline(&self.programs[*prog].pipelines[shader::blend_index(blend)]);
                    pass.set_bind_group(1, group, &[]);
                }
                (StencilMode::Draw, None) => pass.set_pipeline(&self.pipelines[blend as usize]),
                (StencilMode::Raise, _) => pass.set_pipeline(&self.mask_pipelines[0]),
                (StencilMode::Lower, _) => pass.set_pipeline(&self.mask_pipelines[1]),
            }
            // Raising compares against the level below; drawing and lowering
            // against the mask's own level (the batch records which).
            pass.set_stencil_reference(level);
            // A released texture draws as plain white instead of crashing.
            let t = self.textures.get(tex.0).and_then(Option::as_ref).or(self.textures[self.white.0].as_ref());
            if let Some(t) = t {
                let g = if self.antialiasing { &t.bind_group } else { t.nearest_group.as_ref().unwrap_or(&t.bind_group) };
                pass.set_bind_group(0, g, &[]);
            }
            pass.draw(start..end, 0..1);
        }
    }

    /// Submit everything queued this frame.
    pub fn present(&mut self) {
        let post = self.post.take();
        let draws = std::mem::take(&mut self.frame_draws);
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f)
            | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                self.vertices.clear();
                self.batches.clear();
                self.cur_shader = None;
                return;
            }
            _ => {
                self.vertices.clear();
                self.batches.clear();
                self.cur_shader = None;
                return;
            }
        };

        // A canvas-sized quad for the camera filters.
        let full = self.vertices.len() as u32;
        if post.is_some() {
            let corners = [([0.0, 0.0], [0.0, 0.0]), ([VIEW_W, 0.0], [1.0, 0.0]), ([VIEW_W, VIEW_H], [1.0, 1.0]), ([0.0, VIEW_H], [0.0, 1.0])];
            for i in [0, 1, 2, 0, 2, 3] {
                let (pos, uv) = corners[i];
                self.vertices.push(Vertex { pos, uv, color: WHITE, fx: [0.0; 4] });
            }
        }

        if self.vertices.len() > self.vbuf_cap {
            self.vbuf_cap = self.vertices.len().next_power_of_two();
            self.vbuf = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: (self.vbuf_cap * std::mem::size_of::<Vertex>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        if !self.vertices.is_empty() {
            self.queue
                .write_buffer(&self.vbuf, 0, bytemuck::cast_slice(&self.vertices));
        }

        // Letterbox the 16:9 canvas.
        let (ww, wh) = (self.config.width as f32, self.config.height as f32);
        let scale = (ww / VIEW_W).min(wh / VIEW_H);
        let (vw, vh) = (VIEW_W * scale, VIEW_H * scale);
        let (vx, vy) = ((ww - vw) / 2.0, (wh - vh) / 2.0);
        let (tw, th) = ((vw.round() as u32).max(1), (vh.round() as u32).max(1));

        let view = frame.texture.create_view(&Default::default());
        let mut enc = self.device.create_command_encoder(&Default::default());
        let batches = std::mem::take(&mut self.batches);
        let split = post.as_ref().map_or(0, |(k, _)| (*k).min(batches.len()));
        let filters = post.map(|(_, f)| f).unwrap_or_default();
        if !filters.is_empty() {
            self.post_target(0, tw, th);
            if filters.len() > 1 {
                self.post_target(1, tw, th);
            }
        }

        // Uniforms for every shader use, then their bind groups.
        let mut ubytes = Vec::new();
        let white = self.textures[self.white.0].as_ref().expect("white texture");
        let tex_of = |t: TexId| self.textures.get(t.0).and_then(Option::as_ref).unwrap_or(white);
        let batch_offs: Vec<Option<[u64; 2]>> = batches
            .iter()
            .map(|b| {
                let d = draws.get(b.shader? as usize)?;
                let t = tex_of(b.tex);
                Some([push_uniforms(&mut ubytes, &d.builtin(t.width as f32, t.height as f32)), push_uniforms(&mut ubytes, &d.user)])
            })
            .collect();
        let filter_offs: Vec<[u64; 2]> = filters
            .iter()
            .map(|d| [push_uniforms(&mut ubytes, &d.builtin(tw as f32, th as f32)), push_uniforms(&mut ubytes, &d.user)])
            .collect();
        let ubuf = (!ubytes.is_empty()).then(|| {
            self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("shader uniforms"),
                contents: &ubytes,
                usage: wgpu::BufferUsages::UNIFORM,
            })
        });
        let groups: Vec<Option<(usize, wgpu::BindGroup)>> = batches
            .iter()
            .zip(&batch_offs)
            .map(|(b, offs)| {
                let d = &draws[b.shader? as usize];
                Some((d.prog.0, self.shader_group(d, (*offs)?, ubuf.as_ref()?, &tex_of(b.tex).view)))
            })
            .collect();
        // Filter `j` reads target `j % 2` (filter 0 reads the camera's image).
        let filter_groups: Vec<wgpu::BindGroup> = filters
            .iter()
            .zip(&filter_offs)
            .enumerate()
            .filter_map(|(j, (d, offs))| Some(self.shader_group(d, *offs, ubuf.as_ref()?, &self.post_targets[j % 2].2)))
            .collect();

        let begin = |enc: &mut wgpu::CommandEncoder, target: &wgpu::TextureView, stencil: &wgpu::TextureView| {
            enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: stencil,
                    depth_ops: None,
                    stencil_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0), store: wgpu::StoreOp::Discard }),
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            })
            .forget_lifetime()
        };
        let filter_quad = |pass: &mut wgpu::RenderPass<'_>, j: usize| {
            let d = &filters[j];
            pass.set_pipeline(&self.programs[d.prog.0].pipelines[0]);
            pass.set_stencil_reference(0);
            pass.set_bind_group(0, &self.post_targets[j % 2].3, &[]);
            pass.set_bind_group(1, &filter_groups[j], &[]);
            pass.draw(full..full + 6, 0..1);
        };
        let ready = filter_groups.len() == filters.len();
        if !filters.is_empty() {
            let stencil = Self::stencil_view(&self.device, &mut self.stencil_post, tw, th);
            // The game camera's image, at the letterboxed size.
            {
                let mut pass = begin(&mut enc, &self.post_targets[0].2, &stencil);
                pass.set_viewport(0.0, 0.0, tw as f32, th as f32, 0.0, 1.0);
                pass.set_vertex_buffer(0, self.vbuf.slice(..));
                self.draw_batches(&mut pass, &batches[..split], &groups[..split]);
            }
            for j in 0..filters.len() - 1 {
                if !ready {
                    break;
                }
                let mut pass = begin(&mut enc, &self.post_targets[(j + 1) % 2].2, &stencil);
                pass.set_viewport(0.0, 0.0, tw as f32, th as f32, 0.0, 1.0);
                pass.set_vertex_buffer(0, self.vbuf.slice(..));
                filter_quad(&mut pass, j);
            }
        }
        {
            let stencil = Self::stencil_view(&self.device, &mut self.stencil_main, self.config.width.max(1), self.config.height.max(1));
            let mut pass = begin(&mut enc, &view, &stencil);
            pass.set_viewport(vx, vy, vw, vh, 0.0, 1.0);
            pass.set_vertex_buffer(0, self.vbuf.slice(..));
            if !filters.is_empty() && ready {
                filter_quad(&mut pass, filters.len() - 1);
            }
            self.draw_batches(&mut pass, &batches[split..], &groups[split..]);
        }
        self.batches = batches;
        let readback = if std::mem::take(&mut self.capture)
            && self.config.usage.contains(wgpu::TextureUsages::COPY_SRC)
        {
            let (x0, y0) = (vx.round().max(0.0) as u32, vy.round().max(0.0) as u32);
            let w = (vw.round() as u32).min(self.config.width - x0).max(1);
            let h = (vh.round() as u32).min(self.config.height - y0).max(1);
            let bpr = (w * 4).div_ceil(256) * 256;
            let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: (bpr * h) as u64,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            enc.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: &frame.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x: x0, y: y0, z: 0 },
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &buf,
                    layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(bpr), rows_per_image: Some(h) },
                },
                wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            );
            Some((buf, w, h, bpr))
        } else {
            None
        };
        self.queue.submit([enc.finish()]);
        self.window.pre_present_notify();
        self.queue.present(frame);
        if let Some((buf, w, h, bpr)) = readback {
            let slice = buf.slice(..);
            slice.map_async(wgpu::MapMode::Read, |_| {});
            let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
            let Ok(data) = slice.get_mapped_range() else {
                return;
            };
            let bgra = matches!(self.config.format, wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb);
            let mut out = Vec::with_capacity((w * h * 4) as usize);
            for row in 0..h as usize {
                let r = &data[row * bpr as usize..row * bpr as usize + w as usize * 4];
                for px in r.as_chunks::<4>().0 {
                    if bgra {
                        out.extend_from_slice(&[px[2], px[1], px[0], 255]);
                    } else {
                        out.extend_from_slice(&[px[0], px[1], px[2], 255]);
                    }
                }
            }
            drop(data);
            buf.unmap();
            self.captured = Some((w, h, out));
        }
        self.vertices.clear();
        self.batches.clear();
        self.frame_draws.clear();
        self.cur_shader = None;
    }

    /// Start a frame by filling the canvas with `color`.
    pub fn begin(&mut self, color: Color) {
        self.transform = Affine::IDENTITY;
        self.blend = Blend::Normal;
        self.stencil_mode = StencilMode::Draw;
        self.stencil_level = 0;
        self.cur_shader = None;
        self.rect(0.0, 0.0, VIEW_W, VIEW_H, color);
    }

    /// Convert window pixel coordinates into canvas coordinates.
    #[allow(dead_code)]
    pub fn to_canvas(&self, x: f32, y: f32) -> (f32, f32) {
        let (ww, wh) = (self.config.width as f32, self.config.height as f32);
        let scale = (ww / VIEW_W).min(wh / VIEW_H);
        (
            (x - (ww - VIEW_W * scale) / 2.0) / scale,
            (y - (wh - VIEW_H * scale) / 2.0) / scale,
        )
    }
}

/// A uniform block's bound size (whole 16-byte rows, never empty).
fn uniform_len(n: usize) -> u64 {
    (n as u64).div_ceil(16).max(1) * 16
}

/// Append a uniform block to `buf` at a 256-aligned offset, returning it.
fn push_uniforms(buf: &mut Vec<u8>, bytes: &[u8]) -> u64 {
    let at = buf.len().div_ceil(256) * 256;
    buf.resize(at, 0);
    buf.extend_from_slice(bytes);
    buf.resize(at + uniform_len(bytes.len()) as usize, 0);
    at as u64
}

#[cfg(test)]
mod tests {
    #[test]
    fn sprite_shader_validates() {
        use wgpu::naga;
        let m = naga::front::wgsl::parse_str(super::SHADER).unwrap_or_else(|e| panic!("{}", e.emit_to_string(super::SHADER)));
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all())
            .validate(&m)
            .expect("sprite shader validates");
    }
}
