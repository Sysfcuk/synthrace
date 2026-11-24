//! SYNTHRACE renderer (M0 spike): Neon Bent-World ground + sky into an HDR
//! target, COD:AW-style bloom chain, AgX composite. Designed strictly to the
//! WebGL2 downlevel limits so the same code runs on WebGPU and WebGL2.

pub mod demo;
pub mod shaders;

use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;

pub const PROFILE_SAMPLES: usize = 64;
pub const PROFILE_STEP: f32 = 20.0;
const BLOOM_LEVELS: usize = 6;
const GROUND_COLS: u32 = 161;
const GROUND_ROWS: u32 = 253;
const GROUND_CELL_Z: f32 = 5.0;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth24Plus;

/// Everything the renderer needs for one frame. Produced by the game (later: from a sim snapshot).
#[derive(Clone, Debug)]
pub struct FrameParams {
    pub time: f32,
    /// Distance travelled along the track (m).
    pub scroll: f32,
    /// Speed normalised to 0..1 (drives FOV and speed effects).
    pub speed01: f32,
    /// Musical beat phase 0..1 (0 = on the beat).
    pub beat_phase: f32,
    /// Player lateral position (m, track space).
    pub player_x: f32,
    pub profile: [[f32; 2]; PROFILE_SAMPLES],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct FrameUniform {
    view_proj: [[f32; 4]; 4],
    inv_view_proj: [[f32; 4]; 4],
    cam: [f32; 4],
    world: [f32; 4],
    screen: [f32; 4],
    profile: [[f32; 4]; PROFILE_SAMPLES],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct PostUniform {
    a: [f32; 4],
    b: [f32; 4],
}

/// Per-frame counters for the debug overlay.
#[derive(Clone, Copy, Debug, Default)]
pub struct RenderStats {
    pub draw_calls: u32,
    pub passes: u32,
}

struct Target {
    _tex: wgpu::Texture,
    view: wgpu::TextureView,
    width: u32,
    height: u32,
}

struct SizedResources {
    hdr: Target,
    depth: Target,
    down: Vec<Target>,
    up: Vec<Target>,
    down_bgs: Vec<wgpu::BindGroup>,
    up_bgs: Vec<wgpu::BindGroup>,
    composite_bg: wgpu::BindGroup,
    _param_bufs: Vec<wgpu::Buffer>,
}

pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    hdr_format: wgpu::TextureFormat,
    output_format: wgpu::TextureFormat,
    manual_srgb: bool,
    frame_ubo: wgpu::Buffer,
    post_ubo: wgpu::Buffer,
    frame_bg: wgpu::BindGroup,
    sampler: wgpu::Sampler,
    sky_pipe: wgpu::RenderPipeline,
    ground_pipe: wgpu::RenderPipeline,
    down_pipe: wgpu::RenderPipeline,
    up_pipe: wgpu::RenderPipeline,
    composite_pipe: wgpu::RenderPipeline,
    down_bgl: wgpu::BindGroupLayout,
    up_bgl: wgpu::BindGroupLayout,
    composite_bgl: wgpu::BindGroupLayout,
    ground_vb: wgpu::Buffer,
    ground_ib: wgpu::Buffer,
    ground_index_count: u32,
    sized: SizedResources,
    pub stats: RenderStats,
}

/// Picks the HDR scene format: RGBA16F when renderable + filterable (EXT_color_buffer_float on WebGL2),
/// otherwise an LDR fallback so the game still runs.
pub fn choose_hdr_format(adapter: &wgpu::Adapter) -> wgpu::TextureFormat {
    let f = wgpu::TextureFormat::Rgba16Float;
    let feats = adapter.get_texture_format_features(f);
    let need = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
    if feats.allowed_usages.contains(need) && feats.flags.contains(wgpu::TextureFormatFeatureFlags::FILTERABLE) {
        f
    } else {
        log::warn!("RGBA16F not renderable; falling back to RGBA8 (no HDR)");
        wgpu::TextureFormat::Rgba8Unorm
    }
}

fn shader(device: &wgpu::Device, name: &str) -> wgpu::ShaderModule {
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(name),
        source: wgpu::ShaderSource::Wgsl(shaders::compose(name).into()),
    })
}

fn tex_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn sampler_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    }
}

fn ubo_entry(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
        count: None,
    }
}

#[allow(clippy::too_many_arguments)]
fn pipeline(
    device: &wgpu::Device,
    label: &str,
    module: &wgpu::ShaderModule,
    vs: &str,
    fs: &str,
    layouts: &[Option<&wgpu::BindGroupLayout>],
    buffers: &[Option<wgpu::VertexBufferLayout>],
    format: wgpu::TextureFormat,
    depth: Option<wgpu::DepthStencilState>,
) -> wgpu::RenderPipeline {
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: layouts,
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(&layout),
        vertex: wgpu::VertexState { module, entry_point: Some(vs), compilation_options: Default::default(), buffers },
        primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
        depth_stencil: depth,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some(fs),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

fn make_target(device: &wgpu::Device, label: &str, w: u32, h: u32, format: wgpu::TextureFormat) -> Target {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d { width: w.max(1), height: h.max(1), depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = tex.create_view(&Default::default());
    Target { _tex: tex, view, width: w.max(1), height: h.max(1) }
}

/// Ground lattice: lateral columns spread non-linearly (dense near the road, wide at the edges),
/// rows every 5 m out to ~1.26 km.
fn ground_mesh() -> (Vec<[f32; 2]>, Vec<u32>) {
    let mut v = Vec::with_capacity((GROUND_COLS * GROUND_ROWS) as usize);
    for r in 0..GROUND_ROWS {
        let z = r as f32 * GROUND_CELL_Z;
        for c in 0..GROUND_COLS {
            let u = c as f32 / (GROUND_COLS - 1) as f32 * 2.0 - 1.0;
            let x = 40.0 * u + 1460.0 * u * u * u;
            v.push([x, z]);
        }
    }
    let mut idx = Vec::with_capacity(((GROUND_COLS - 1) * (GROUND_ROWS - 1) * 6) as usize);
    for r in 0..GROUND_ROWS - 1 {
        for c in 0..GROUND_COLS - 1 {
            let i = r * GROUND_COLS + c;
            idx.extend_from_slice(&[i, i + GROUND_COLS, i + 1, i + 1, i + GROUND_COLS, i + GROUND_COLS + 1]);
        }
    }
    (v, idx)
}

impl Renderer {
    pub fn new(
        device: wgpu::Device,
        queue: wgpu::Queue,
        hdr_format: wgpu::TextureFormat,
        output_format: wgpu::TextureFormat,
        width: u32,
        height: u32,
    ) -> Self {
        let manual_srgb = !output_format.is_srgb();

        let frame_ubo = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("frame ubo"),
            size: std::mem::size_of::<FrameUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let post_ubo = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("post ubo"),
            size: std::mem::size_of::<PostUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let frame_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("frame bgl"),
            entries: &[ubo_entry(0, wgpu::ShaderStages::VERTEX_FRAGMENT)],
        });
        let frame_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("frame bg"),
            layout: &frame_bgl,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: frame_ubo.as_entire_binding() }],
        });
        let down_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bloom down bgl"),
            entries: &[tex_entry(0), sampler_entry(1), ubo_entry(2, wgpu::ShaderStages::FRAGMENT)],
        });
        let up_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bloom up bgl"),
            entries: &[tex_entry(0), tex_entry(1), sampler_entry(2), ubo_entry(3, wgpu::ShaderStages::FRAGMENT)],
        });
        let composite_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("composite bgl"),
            entries: &[tex_entry(0), tex_entry(1), sampler_entry(2), ubo_entry(3, wgpu::ShaderStages::FRAGMENT)],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("linear clamp"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let depth_test = |write: bool, compare| wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(write),
            depth_compare: Some(compare),
            stencil: Default::default(),
            bias: Default::default(),
        };

        let sky_mod = shader(&device, "sky.wgsl");
        let ground_mod = shader(&device, "ground.wgsl");
        let down_mod = shader(&device, "bloom_down.wgsl");
        let up_mod = shader(&device, "bloom_up.wgsl");
        let comp_mod = shader(&device, "composite.wgsl");

        let sky_pipe = pipeline(
            &device,
            "sky",
            &sky_mod,
            "vs_fullscreen",
            "fs_sky",
            &[Some(&frame_bgl)],
            &[],
            hdr_format,
            Some(depth_test(false, wgpu::CompareFunction::Always)),
        );
        let ground_layout = wgpu::VertexBufferLayout {
            array_stride: 8,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 0, shader_location: 0 }],
        };
        let ground_pipe = pipeline(
            &device,
            "ground",
            &ground_mod,
            "vs_ground",
            "fs_ground",
            &[Some(&frame_bgl)],
            &[Some(ground_layout)],
            hdr_format,
            Some(depth_test(true, wgpu::CompareFunction::Less)),
        );
        let down_pipe =
            pipeline(&device, "bloom down", &down_mod, "vs_fullscreen", "fs_down", &[Some(&down_bgl)], &[], hdr_format, None);
        let up_pipe = pipeline(&device, "bloom up", &up_mod, "vs_fullscreen", "fs_up", &[Some(&up_bgl)], &[], hdr_format, None);
        let composite_pipe = pipeline(
            &device,
            "composite",
            &comp_mod,
            "vs_fullscreen",
            "fs_composite",
            &[Some(&frame_bgl), Some(&composite_bgl)],
            &[],
            output_format,
            None,
        );

        let (verts, idx) = ground_mesh();
        let ground_vb = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("ground vb"),
            contents: bytemuck::cast_slice(&verts),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let ground_ib = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("ground ib"),
            contents: bytemuck::cast_slice(&idx),
            usage: wgpu::BufferUsages::INDEX,
        });

        let sized =
            Self::create_sized(&device, hdr_format, &down_bgl, &up_bgl, &composite_bgl, &sampler, &post_ubo, width, height);

        Self {
            device,
            queue,
            hdr_format,
            output_format,
            manual_srgb,
            frame_ubo,
            post_ubo,
            frame_bg,
            sampler,
            sky_pipe,
            ground_pipe,
            down_pipe,
            up_pipe,
            composite_pipe,
            down_bgl,
            up_bgl,
            composite_bgl,
            ground_vb,
            ground_ib,
            ground_index_count: idx.len() as u32,
            sized,
            stats: RenderStats::default(),
        }
    }

    pub fn hdr_format(&self) -> wgpu::TextureFormat {
        self.hdr_format
    }

    pub fn output_format(&self) -> wgpu::TextureFormat {
        self.output_format
    }

    pub fn size(&self) -> (u32, u32) {
        (self.sized.hdr.width, self.sized.hdr.height)
    }

    #[allow(clippy::too_many_arguments)]
    fn create_sized(
        device: &wgpu::Device,
        hdr_format: wgpu::TextureFormat,
        down_bgl: &wgpu::BindGroupLayout,
        up_bgl: &wgpu::BindGroupLayout,
        composite_bgl: &wgpu::BindGroupLayout,
        sampler: &wgpu::Sampler,
        post_ubo: &wgpu::Buffer,
        width: u32,
        height: u32,
    ) -> SizedResources {
        let hdr = make_target(device, "hdr", width, height, hdr_format);
        let depth = make_target(device, "depth", width, height, DEPTH_FORMAT);
        let mut down = Vec::new();
        let mut up = Vec::new();
        let (mut w, mut h) = (width, height);
        for i in 0..BLOOM_LEVELS {
            w = (w / 2).max(1);
            h = (h / 2).max(1);
            down.push(make_target(device, &format!("bloom down {i}"), w, h, hdr_format));
            up.push(make_target(device, &format!("bloom up {i}"), w, h, hdr_format));
        }
        let mut param_bufs = Vec::new();
        let mut ubo = |v: [f32; 4]| {
            let b = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("bloom params"),
                contents: bytemuck::cast_slice(&v),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            param_bufs.push(b);
            param_bufs.len() - 1
        };
        let mut down_param_idx = Vec::new();
        for i in 0..BLOOM_LEVELS {
            let src = if i == 0 { &hdr } else { &down[i - 1] };
            down_param_idx.push(ubo([1.0 / src.width as f32, 1.0 / src.height as f32, if i == 0 { 1.0 } else { 0.0 }, 0.0]));
        }
        let mut up_param_idx = Vec::new();
        for i in 0..BLOOM_LEVELS - 1 {
            let lower = &up[i + 1];
            up_param_idx.push(ubo([1.0 / lower.width as f32, 1.0 / lower.height as f32, 1.0, 0.0]));
        }
        let down_bgs = (0..BLOOM_LEVELS)
            .map(|i| {
                let src = if i == 0 { &hdr } else { &down[i - 1] };
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("bloom down bg"),
                    layout: down_bgl,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&src.view) },
                        wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(sampler) },
                        wgpu::BindGroupEntry { binding: 2, resource: param_bufs[down_param_idx[i]].as_entire_binding() },
                    ],
                })
            })
            .collect();
        // up[N-1] is produced by a pass reading down[N-1] twice-equivalent: we simply treat
        // the smallest level's "lower" as itself with zero contribution by using down[N-1] as both.
        let up_bgs = (0..BLOOM_LEVELS - 1)
            .map(|i| {
                let lower = if i + 1 == BLOOM_LEVELS - 1 { &down[i + 1] } else { &up[i + 1] };
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("bloom up bg"),
                    layout: up_bgl,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&lower.view) },
                        wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&down[i].view) },
                        wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(sampler) },
                        wgpu::BindGroupEntry { binding: 3, resource: param_bufs[up_param_idx[i]].as_entire_binding() },
                    ],
                })
            })
            .collect();
        let composite_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("composite bg"),
            layout: composite_bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&hdr.view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&up[0].view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(sampler) },
                wgpu::BindGroupEntry { binding: 3, resource: post_ubo.as_entire_binding() },
            ],
        });
        SizedResources { hdr, depth, down, up, down_bgs, up_bgs, composite_bg, _param_bufs: param_bufs }
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if (width.max(1), height.max(1)) == self.size() {
            return;
        }
        self.sized = Self::create_sized(
            &self.device,
            self.hdr_format,
            &self.down_bgl,
            &self.up_bgl,
            &self.composite_bgl,
            &self.sampler,
            &self.post_ubo,
            width,
            height,
        );
    }

    fn upload(&self, p: &FrameParams) {
        let (w, h) = self.size();
        let aspect = w as f32 / h as f32;
        let fov = (58.0 + 14.0 * p.speed01 * p.speed01).to_radians();
        let eye = Vec3::new(p.player_x * 0.85, 2.6, 0.0);
        let dir = Vec3::new(0.0, -0.035, 1.0).normalize();
        let view = Mat4::look_to_lh(eye, dir, Vec3::Y);
        let proj = Mat4::perspective_lh(fov, aspect, 0.3, 2000.0);
        let vp = proj * view;
        let mut profile = [[0.0f32; 4]; PROFILE_SAMPLES];
        for (d, s) in profile.iter_mut().zip(p.profile.iter()) {
            *d = [s[0], s[1], 0.0, 0.0];
        }
        let fu = FrameUniform {
            view_proj: vp.to_cols_array_2d(),
            inv_view_proj: vp.inverse().to_cols_array_2d(),
            cam: [eye.x, eye.y, eye.z, p.time],
            world: [0.000035, p.scroll, p.speed01, p.beat_phase],
            screen: [w as f32, h as f32, 1.0 / w as f32, 1.0 / h as f32],
            profile,
        };
        self.queue.write_buffer(&self.frame_ubo, 0, bytemuck::bytes_of(&fu));
        let pu = PostUniform {
            a: [0.05, 0.95, 1.0 / BLOOM_LEVELS as f32, if self.manual_srgb { 1.0 } else { 0.0 }],
            b: [0.55, 0.035, 0.004 * p.speed01, 0.035 * p.speed01 * p.speed01],
        };
        self.queue.write_buffer(&self.post_ubo, 0, bytemuck::bytes_of(&pu));
    }

    /// Renders one frame into `output` (a surface texture view or an offscreen target).
    pub fn render(&mut self, output: &wgpu::TextureView, p: &FrameParams) {
        self.upload(p);
        let mut stats = RenderStats::default();
        let mut enc = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });
        let s = &self.sized;

        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &s.hdr.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &s.depth.view,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.frame_bg, &[]);
            pass.set_pipeline(&self.sky_pipe);
            pass.draw(0..3, 0..1);
            pass.set_pipeline(&self.ground_pipe);
            pass.set_vertex_buffer(0, self.ground_vb.slice(..));
            pass.set_index_buffer(self.ground_ib.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..self.ground_index_count, 0, 0..1);
            stats.draw_calls += 2;
            stats.passes += 1;
        }

        let mut fullscreen = |enc: &mut wgpu::CommandEncoder,
                              label: &str,
                              target: &wgpu::TextureView,
                              pipe: &wgpu::RenderPipeline,
                              groups: &[&wgpu::BindGroup]| {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(label),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(pipe);
            for (i, g) in groups.iter().enumerate() {
                pass.set_bind_group(i as u32, *g, &[]);
            }
            pass.draw(0..3, 0..1);
            stats.draw_calls += 1;
            stats.passes += 1;
        };

        for i in 0..BLOOM_LEVELS {
            fullscreen(&mut enc, "bloom down", &s.down[i].view, &self.down_pipe, &[&s.down_bgs[i]]);
        }
        // Smallest level: up[N-1] is not rendered; up[N-2] reads down[N-1] directly (see create_sized).
        for i in (0..BLOOM_LEVELS - 1).rev() {
            fullscreen(&mut enc, "bloom up", &s.up[i].view, &self.up_pipe, &[&s.up_bgs[i]]);
        }
        fullscreen(&mut enc, "composite", output, &self.composite_pipe, &[&self.frame_bg, &s.composite_bg]);

        self.queue.submit(Some(enc.finish()));
        self.stats = stats;
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }
}
