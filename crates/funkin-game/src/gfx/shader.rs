//! Script shaders on the GPU: a sprite drawn with `shader = ...`, and camera
//! filters (`camera.filters = [new ShaderFilter(shader)]`).
//!
//! Each program is the game's GLSL translated by [`super::glsl`] and paired with
//! a vertex shader that feeds it Flixel's varyings. Group 0 is the sprite's own
//! texture (as for plain sprites); group 1 holds the built-in and user uniform
//! blocks and any extra `sampler2D`s.

use super::glsl;
use super::{Blend, TexId};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ShaderId(pub usize);

/// The values one shader instance carries (`FlxShader` parameters).
#[derive(Debug, Default, Clone)]
pub struct ShaderParams {
    /// Uniform values by name (`setFloat`, `setFloatArray`, `setInt`, `setBool`...).
    pub values: HashMap<String, Vec<f64>>,
    /// `sampler2D` inputs by name (`setSampler2D`, `setBitmapData`).
    pub textures: HashMap<String, TexId>,
}

impl ShaderParams {
    pub fn set(&mut self, name: &str, v: Vec<f64>) {
        self.values.insert(name.to_owned(), v);
    }
    pub fn get(&self, name: &str) -> Option<&Vec<f64>> {
        self.values.get(name)
    }
}

pub(super) struct Program {
    pub pipelines: Vec<wgpu::RenderPipeline>,
    pub group: wgpu::BindGroupLayout,
    pub layout: glsl::Layout,
    pub samplers: Vec<String>,
}

/// Vertex stage: Flixel's `openfl_TextureCoordv`, the colour transform, and
/// `screenCoord` (0..1 across the canvas; for a camera filter, across the camera).
pub(super) const VERTEX: &str = r#"
struct VsOut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32>, @location(1) color: vec4<f32>, @location(2) screen: vec2<f32> };
@vertex fn vs(@location(0) pos: vec2<f32>, @location(1) uv: vec2<f32>, @location(2) color: vec4<f32>, @location(3) fx: vec4<f32>) -> VsOut {
    var o: VsOut;
    o.pos = vec4<f32>(pos.x / 640.0 - 1.0, 1.0 - pos.y / 360.0, 0.0, 1.0);
    o.uv = uv;
    // "Fill" colours (alpha above 1.5) aren't a colour transform.
    o.color = select(color, vec4<f32>(1.0, 1.0, 1.0, color.a - 2.0), color.a > 1.5);
    // RuntimePostEffectShader: 0 or 1 by whether the texture coordinate is past 0.
    o.screen = select(vec2<f32>(0.0), vec2<f32>(1.0), uv > vec2<f32>(0.0));
    return o;
}
"#;

pub(super) fn compile(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    group0: &wgpu::BindGroupLayout,
    vertex: &wgpu::ShaderModule,
    src: &str,
    post: bool,
) -> Result<Program, String> {
    let t = glsl::translate(src, post);
    let (_, layout) = glsl::reflect(&t.source)?;
    let mut entries = vec![
        wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 1,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
            count: None,
        },
    ];
    for i in 1..t.samplers.len() {
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: i as u32 + 1,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        });
    }
    let group = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("shader"), entries: &entries });
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("script shader"),
        source: wgpu::ShaderSource::Glsl { shader: t.source.into(), stage: wgpu::naga::ShaderStage::Fragment, defines: &[] },
    });
    let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[Some(group0), Some(&group)],
        immediate_size: 0,
    });
    let comp = |src, dst| wgpu::BlendComponent { src_factor: src, dst_factor: dst, operation: wgpu::BlendOperation::Add };
    use wgpu::BlendFactor as F;
    // Output is premultiplied (as OpenFL's is).
    let blends = [
        wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING,
        wgpu::BlendState { color: comp(F::One, F::One), alpha: comp(F::Zero, F::One) },
        wgpu::BlendState { color: comp(F::Dst, F::OneMinusSrcAlpha), alpha: comp(F::Zero, F::One) },
        wgpu::BlendState { color: comp(F::One, F::OneMinusSrc), alpha: comp(F::One, F::OneMinusSrcAlpha) },
    ];
    let pipelines = blends
        .iter()
        .map(|b| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("script shader"),
                layout: Some(&pl),
                vertex: wgpu::VertexState {
                    module: vertex,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<super::Vertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4, 3 => Float32x4],
                    })],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: Some(super::stencil_state(wgpu::StencilOperation::Keep)),
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format, blend: Some(*b), write_mask: wgpu::ColorWrites::ALL })],
                }),
                multiview_mask: None,
                cache: None,
            })
        })
        .collect();
    if let Some(e) = pollster::block_on(scope.pop()) {
        return Err(e.to_string());
    }
    Ok(Program { pipelines, group, layout, samplers: t.samplers })
}

/// Frame bounds that follow what is drawn (`DropShadowShader.attachedSprite`,
/// `updateFrameInfo(sprite.frame)`): each quad sets `uFrameBounds` to its own
/// texture rectangle, and a drop shadow's light turns with rotated frames.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct AutoFrame {
    pub on: bool,
    /// `DropShadowShader.angle` (degrees).
    pub drop_angle: Option<f32>,
}

/// One use of a shader this frame: its uniform bytes and extra textures.
#[derive(Clone)]
pub(super) struct FrameDraw {
    pub prog: ShaderId,
    pub user: Vec<u8>,
    /// Post effects' camera / frame / screen values (built-in block).
    pub screen: [f32; 2],
    pub camera: [f32; 4],
    pub frame: [f32; 4],
    /// One per sampler after `bitmap` (`None` = the sprite's own texture).
    pub textures: Vec<Option<TexId>>,
    pub auto: AutoFrame,
    /// The frame (uv bounds, angle) this use was made for.
    pub frame_key: Option<([f32; 4], f32)>,
}

impl FrameDraw {
    pub fn new(prog: ShaderId, p: &Program, params: &ShaderParams, auto: AutoFrame) -> Self {
        let mut user = vec![0u8; p.layout.user_size as usize];
        // Uniforms the script never set stay zero, as in OpenFL.
        for (name, v) in &params.values {
            if let Some(u) = p.layout.find(name) {
                u.write(&mut user, v);
            }
        }
        let get4 = |n: &str, d: [f32; 4]| -> [f32; 4] {
            match params.values.get(n) {
                Some(v) if v.len() >= 4 => [v[0] as f32, v[1] as f32, v[2] as f32, v[3] as f32],
                _ => d,
            }
        };
        let screen = match params.values.get("uScreenResolution") {
            Some(v) if v.len() >= 2 => [v[0] as f32, v[1] as f32],
            _ => [super::VIEW_W, super::VIEW_H],
        };
        Self {
            prog,
            user,
            screen,
            camera: get4("uCameraBounds", [0.0, 0.0, super::VIEW_W, super::VIEW_H]),
            frame: get4("uFrameBounds", [0.0, 0.0, super::VIEW_W, super::VIEW_H]),
            textures: p.samplers[1..].iter().map(|n| params.textures.get(n).copied()).collect(),
            auto,
            frame_key: None,
        }
    }

    /// This use for a quad showing texture rectangle `bounds` (uv) rotated by `angle`.
    pub fn with_frame(&self, p: &Program, bounds: [f32; 4], angle: f32) -> Self {
        let mut d = self.clone();
        d.frame = bounds;
        d.frame_key = Some((bounds, angle));
        if let Some(u) = p.layout.find("uFrameBounds") {
            u.write(&mut d.user, &bounds.map(f64::from));
        }
        if let Some(a) = self.auto.drop_angle {
            let r = ((a + angle) as f64).to_radians();
            for (n, v) in [("angCos", r.cos()), ("angSin", r.sin())] {
                if let Some(u) = p.layout.find(n) {
                    u.write(&mut d.user, &[v]);
                }
            }
        }
        d
    }

    /// The built-in block for a draw of a `w`×`h` texture.
    pub fn builtin(&self, w: f32, h: f32) -> [u8; glsl::BUILTIN_SIZE as usize] {
        let mut b = [0u8; glsl::BUILTIN_SIZE as usize];
        let mut put = |at: u32, vals: &[f32]| {
            for (i, v) in vals.iter().enumerate() {
                let o = at as usize + i * 4;
                b[o..o + 4].copy_from_slice(&v.to_le_bytes());
            }
        };
        put(glsl::BUILTIN_TEXTURE_SIZE, &[w, h]);
        put(glsl::BUILTIN_SCREEN, &self.screen);
        put(glsl::BUILTIN_CAMERA, &self.camera);
        put(glsl::BUILTIN_FRAME, &self.frame);
        b
    }
}

/// Blend index into [`Program::pipelines`].
pub(super) fn blend_index(b: Blend) -> usize {
    b as usize
}
