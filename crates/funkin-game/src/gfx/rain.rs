//! Port of `RuntimeRainShader` (`shaders/rain.frag` on top of
//! `RuntimePostEffectShader`): Weekend 1's rain, a post effect on the game
//! camera. Everything drawn before [`super::Gfx::rain`] is rendered to an
//! offscreen texture, which this pass then draws to the screen with the rain
//! distortion; the HUD is drawn on top.
//!
//! The original's puddle reflections and light map are commented out in the
//! shader, so they're not ported.

/// The shader's uniforms for one frame.
#[derive(Debug, Clone, Copy)]
pub struct RainParams {
    /// `uCameraBounds`: the game camera's view (left, top, right, bottom) in world px.
    pub bounds: [f32; 4],
    /// `uRainColor` (0..1 RGB).
    pub color: [f32; 3],
    /// `uScale`: raindrop size (`FlxG.height / 200`).
    pub scale: f32,
    /// `uIntensity`.
    pub intensity: f32,
    /// `uTime` in seconds.
    pub time: f32,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    bounds: [f32; 4],
    color_scale: [f32; 4],
    params: [f32; 4],
}

impl From<RainParams> for Uniforms {
    fn from(p: RainParams) -> Self {
        Self {
            bounds: p.bounds,
            color_scale: [p.color[0], p.color[1], p.color[2], p.scale],
            params: [p.intensity, p.time, 0.0, 0.0],
        }
    }
}

const SHADER: &str = r#"
struct U { bounds: vec4<f32>, color_scale: vec4<f32>, params: vec4<f32> };
@group(0) @binding(0) var screen: texture_2d<f32>;
@group(0) @binding(1) var smp: sampler;
@group(0) @binding(2) var<uniform> u: U;

struct VsOut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };
@vertex fn vs(@builtin(vertex_index) i: u32) -> VsOut {
    // One triangle covering the viewport; uv (0, 0) is the top left.
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var o: VsOut;
    o.pos = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0);
    o.uv = uv;
    return o;
}

// GLSL `mod` (floored).
fn gmod(x: f32, y: f32) -> f32 { return x - y * floor(x / y); }
fn gmod2(x: vec2<f32>, y: f32) -> vec2<f32> { return x - y * floor(x / y); }

fn rand(a: vec2<f32>) -> f32 {
    return fract(sin(dot(gmod2(a, 1000.0), vec2<f32>(12.9898, 78.233))) * 43758.5453);
}

fn screen_to_world(c: vec2<f32>) -> vec2<f32> {
    let b = u.bounds;
    return c * vec2<f32>(b.z - b.x, b.w - b.y) + b.xy;
}

fn world_to_screen(w: vec2<f32>) -> vec2<f32> {
    let b = u.bounds;
    return (w - b.xy) / vec2<f32>(b.z - b.x, b.w - b.y);
}

fn rain_dist(p0: vec2<f32>, scale: f32, intensity: f32) -> f32 {
    // scale everything
    var p = p0 * 0.1;
    // sheer
    p.x += p.y * 0.1;
    // scroll
    p.y -= u.params.y * 500.0 / scale;
    // expand Y
    p.y *= 0.03;
    let ix = floor(p.x);
    // shift Y
    p.y += gmod(ix, 2.0) * 0.5 + (rand(vec2<f32>(ix)) - 0.5) * 0.3;
    let iy = floor(p.y);
    let index = vec2<f32>(ix, iy);
    // mod
    p -= index;
    // shift X
    p.x += (rand(index.yx) * 2.0 - 1.0) * 0.35;
    // distance
    let a = abs(p - 0.5);
    let res = max(a.x * 0.8, a.y * 0.5) - 0.1;
    // decimate
    let empty = rand(index) < mix(1.0, 0.1, intensity);
    return select(res, 1.0, empty);
}

@fragment fn fs(i: VsOut) -> @location(0) vec4<f32> {
    let uscale = u.color_scale.w;
    let intensity = u.params.x;
    var wpos = screen_to_world(i.uv);
    var add = vec3<f32>(0.0);
    var rain_sum = 0.0;
    let scales = array<f32, 4>(1.0, 1.8, 2.6, 4.8);
    for (var l = 0; l < 4; l++) {
        let scale = scales[l];
        let r = rain_dist(wpos * scale / uscale + 500.0 * f32(l), scale, intensity);
        if (r < 0.0) {
            let v = (1.0 - exp(r * 5.0)) / scale * 2.0;
            wpos.x += v * 10.0 * uscale;
            wpos.y -= v * 2.0 * uscale;
            add += vec3<f32>(0.1, 0.15, 0.2) * v;
            rain_sum += (1.0 - rain_sum) * 0.75;
        }
    }
    let s = textureSampleLevel(screen, smp, world_to_screen(wpos), 0.0);
    var color = s.rgb + add;
    color = mix(color, u.color_scale.rgb, 0.1 * rain_sum);
    return vec4<f32>(color, s.a);
}
"#;

/// The rain pipeline and its offscreen target.
pub(super) struct RainPass {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    ubuf: wgpu::Buffer,
    /// (width, height, view, bind group) of the offscreen texture.
    target: Option<(u32, u32, wgpu::TextureView, wgpu::BindGroup)>,
}

impl RainPass {
    pub(super) fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("rain"),
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
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("rain"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("rain"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let ubuf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rain"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self { pipeline, layout, ubuf, target: None }
    }

    /// The offscreen texture's view, (re)created at `w`×`h`.
    pub(super) fn target(&mut self, device: &wgpu::Device, sampler: &wgpu::Sampler, format: wgpu::TextureFormat, w: u32, h: u32) -> &wgpu::TextureView {
        if !matches!(&self.target, Some((tw, th, ..)) if *tw == w && *th == h) {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("rain target"),
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(sampler) },
                    wgpu::BindGroupEntry { binding: 2, resource: self.ubuf.as_entire_binding() },
                ],
            });
            self.target = Some((w, h, view, bind_group));
        }
        &self.target.as_ref().unwrap().2
    }

    pub(super) fn write(&self, queue: &wgpu::Queue, p: RainParams) {
        queue.write_buffer(&self.ubuf, 0, bytemuck::bytes_of(&Uniforms::from(p)));
    }

    /// Draw the rained-on offscreen image into the current pass's viewport.
    pub(super) fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        if let Some((.., bg)) = &self.target {
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, bg, &[]);
            pass.draw(0..3, 0..1);
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn shader_validates() {
        use wgpu::naga;
        let m = naga::front::wgsl::parse_str(super::SHADER).unwrap_or_else(|e| panic!("{}", e.emit_to_string(super::SHADER)));
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all())
            .validate(&m)
            .expect("rain shader validates");
    }
}
