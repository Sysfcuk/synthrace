//! SYNTHRACE renderer: Neon Bent-World ground, sky, instanced meshes and additive particles
//! into an HDR target, COD:AW-style bloom chain, AgX composite. Designed strictly to the
//! WebGL2 downlevel limits so the same code runs on WebGPU and WebGL2.

mod gpu;
mod post;
pub mod shaders;

use glam::{Mat4, Vec3};
use gpu::{ADDITIVE, GpuMesh, PipelineDesc, depth_state, pipeline, sampler_entry, shader, tex_entry, ubo_entry, upload_mesh};
use post::{BLOOM_LEVELS, Layouts, SizedResources};
use wgpu::util::DeviceExt;

pub const PROFILE_SAMPLES: usize = 64;
pub const PROFILE_STEP: f32 = 20.0;
/// Maximum palm instances drawn per frame.
pub const MAX_PALMS: usize = 255;
/// Maximum traffic instances per vehicle kind.
pub const MAX_TRAFFIC_PER_KIND: usize = 48;
pub const MAX_PARTICLES: usize = 6000;
const TRAFFIC_KINDS: usize = 4;
const MAX_INSTANCES: usize = 1 + MAX_PALMS + TRAFFIC_KINDS * MAX_TRAFFIC_PER_KIND;

/// One mesh instance in camera-relative track space (x lateral, y up, z ahead of the camera).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Instance {
    pub pos_yaw: [f32; 4],
    /// scale, roll, brake light 0..1, indicator signal (−1 left, +1 right, 0 off).
    pub scale_roll: [f32; 4],
    /// pitch (rad, + = nose up), unused × 3.
    pub extra: [f32; 4],
}

impl Instance {
    pub fn new(x: f32, y: f32, z: f32, yaw: f32, scale: f32, roll: f32) -> Self {
        Self { pos_yaw: [x, y, z, yaw], scale_roll: [scale, roll, 0.0, 0.0], extra: [0.0; 4] }
    }

    pub fn with_pitch(mut self, pitch: f32) -> Self {
        self.extra[0] = pitch;
        self
    }

    pub fn with_lights(mut self, brake: f32, indicator: f32) -> Self {
        self.scale_roll[2] = brake;
        self.scale_roll[3] = indicator;
        self
    }
}

/// Additive HDR particle in camera-relative track space.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Particle {
    /// x, y, z, half-size (m).
    pub pos_size: [f32; 4],
    /// Linear HDR colour (premultiplied by intensity), w = streak length along z (0 = round sprite).
    pub color: [f32; 4],
}

/// Chase camera, in camera-relative track space (the camera always sits at z = 0).
#[derive(Clone, Copy, Debug)]
pub struct Camera {
    pub x: f32,
    /// Yaw (rad, + = looking right) relative to the road direction.
    pub yaw: f32,
    pub height: f32,
    /// Pitch (rad); negative looks down.
    pub pitch: f32,
    pub fov_deg: f32,
}

/// Screen-space feedback driven by gameplay.
#[derive(Clone, Copy, Debug, Default)]
pub struct Fx {
    /// Camera offset (m) from shake.
    pub shake: [f32; 2],
    /// Nitro intensity 0..1 (stronger blur and chromatic aberration).
    pub nitro: f32,
    /// Full-screen flash: linear colour × intensity.
    pub flash: [f32; 4],
}

/// Everything the renderer needs for one frame (built by the game from a sim snapshot).
#[derive(Clone, Debug)]
pub struct FrameParams<'a> {
    pub time: f32,
    /// Camera distance along the track (m); scrolls world-anchored patterns.
    pub scroll: f32,
    /// Speed normalised to 0..1 (drives speed effects).
    pub speed01: f32,
    /// Musical beat phase 0..1 (0 = on the beat).
    pub beat_phase: f32,
    pub camera: Camera,
    pub fx: Fx,
    pub profile: [[f32; 2]; PROFILE_SAMPLES],
    pub car: Instance,
    pub palms: &'a [Instance],
    /// Per vehicle kind: coupe, van, truck, sports.
    pub traffic: [&'a [Instance]; TRAFFIC_KINDS],
    pub particles: &'a [Particle],
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
    flash: [f32; 4],
}

/// Per-frame counters for the debug overlay.
#[derive(Clone, Copy, Debug, Default)]
pub struct RenderStats {
    pub draw_calls: u32,
    pub passes: u32,
    pub instances: u32,
    pub particles: u32,
}

/// Mesh slots in the order instances are packed into the instance buffer.
const SLOT_CAR: usize = 0;
const SLOT_PALM: usize = 1;
const SLOT_TRAFFIC: usize = 2;

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
    down_bgl: wgpu::BindGroupLayout,
    up_bgl: wgpu::BindGroupLayout,
    composite_bgl: wgpu::BindGroupLayout,
    sky_pipe: wgpu::RenderPipeline,
    ground_pipe: wgpu::RenderPipeline,
    mesh_pipe: wgpu::RenderPipeline,
    particle_pipe: wgpu::RenderPipeline,
    down_pipe: wgpu::RenderPipeline,
    up_pipe: wgpu::RenderPipeline,
    composite_pipe: wgpu::RenderPipeline,
    ground: GpuMesh,
    /// car, palm, coupe, van, truck, sports
    meshes: Vec<GpuMesh>,
    instance_buf: wgpu::Buffer,
    particle_buf: wgpu::Buffer,
    /// (first instance, count) per mesh slot for the current frame.
    batches: Vec<(u32, u32)>,
    particle_count: u32,
    sized: SizedResources,
    pub stats: RenderStats,
}

/// Picks the HDR scene format: RGBA16F when renderable, blendable and filterable
/// (EXT_color_buffer_float on WebGL2), otherwise an LDR fallback so the game still runs.
pub fn choose_hdr_format(adapter: &wgpu::Adapter) -> wgpu::TextureFormat {
    let f = wgpu::TextureFormat::Rgba16Float;
    let feats = adapter.get_texture_format_features(f);
    let need = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
    let flags = wgpu::TextureFormatFeatureFlags::FILTERABLE | wgpu::TextureFormatFeatureFlags::BLENDABLE;
    if feats.allowed_usages.contains(need) && feats.flags.contains(flags) {
        f
    } else {
        log::warn!("RGBA16F not renderable/blendable; falling back to RGBA8 (no HDR)");
        wgpu::TextureFormat::Rgba8Unorm
    }
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
        let ubo = |label: &str, size: usize| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: size as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let frame_ubo = ubo("frame ubo", std::mem::size_of::<FrameUniform>());
        let post_ubo = ubo("post ubo", std::mem::size_of::<PostUniform>());
        let bgl = |label: &str, entries: &[wgpu::BindGroupLayoutEntry]| {
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some(label), entries })
        };
        let frame_bgl = bgl("frame bgl", &[ubo_entry(0, wgpu::ShaderStages::VERTEX_FRAGMENT)]);
        let frame_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("frame bg"),
            layout: &frame_bgl,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: frame_ubo.as_entire_binding() }],
        });
        let down_bgl = bgl("bloom down bgl", &[tex_entry(0), sampler_entry(1), ubo_entry(2, wgpu::ShaderStages::FRAGMENT)]);
        let up_bgl =
            bgl("bloom up bgl", &[tex_entry(0), tex_entry(1), sampler_entry(2), ubo_entry(3, wgpu::ShaderStages::FRAGMENT)]);
        let composite_bgl =
            bgl("composite bgl", &[tex_entry(0), tex_entry(1), sampler_entry(2), ubo_entry(3, wgpu::ShaderStages::FRAGMENT)]);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("linear clamp"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let sky_mod = shader(&device, "sky.wgsl");
        let ground_mod = shader(&device, "ground.wgsl");
        let mesh_mod = shader(&device, "mesh.wgsl");
        let particle_mod = shader(&device, "particles.wgsl");
        let down_mod = shader(&device, "bloom_down.wgsl");
        let up_mod = shader(&device, "bloom_up.wgsl");
        let comp_mod = shader(&device, "composite.wgsl");
        let frame_only = [Some(&frame_bgl)];

        let base = |label, module, vs, fs| PipelineDesc {
            label,
            module,
            vs,
            fs,
            layouts: &frame_only,
            buffers: &[],
            format: hdr_format,
            depth: None,
            blend: None,
        };
        let sky_pipe = pipeline(
            &device,
            PipelineDesc {
                depth: Some(depth_state(false, wgpu::CompareFunction::Always)),
                ..base("sky", &sky_mod, "vs_fullscreen", "fs_sky")
            },
        );
        let ground_layout = wgpu::VertexBufferLayout {
            array_stride: 8,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x2],
        };
        let ground_pipe = pipeline(
            &device,
            PipelineDesc {
                buffers: &[Some(ground_layout)],
                depth: Some(depth_state(true, wgpu::CompareFunction::Less)),
                ..base("ground", &ground_mod, "vs_ground", "fs_ground")
            },
        );
        let mesh_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<procgen::Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x3, 3 => Float32],
        };
        let instance_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Instance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &wgpu::vertex_attr_array![4 => Float32x4, 5 => Float32x4, 6 => Float32x4],
        };
        let mesh_pipe = pipeline(
            &device,
            PipelineDesc {
                buffers: &[Some(mesh_layout), Some(instance_layout)],
                depth: Some(depth_state(true, wgpu::CompareFunction::Less)),
                ..base("mesh", &mesh_mod, "vs_mesh", "fs_mesh")
            },
        );
        let particle_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Particle>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4],
        };
        let particle_pipe = pipeline(
            &device,
            PipelineDesc {
                buffers: &[Some(particle_layout)],
                depth: Some(depth_state(false, wgpu::CompareFunction::Less)),
                blend: Some(ADDITIVE),
                ..base("particles", &particle_mod, "vs_particle", "fs_particle")
            },
        );
        let down_layouts = [Some(&down_bgl)];
        let up_layouts = [Some(&up_bgl)];
        let comp_layouts = [Some(&frame_bgl), Some(&composite_bgl)];
        let down_pipe = pipeline(
            &device,
            PipelineDesc { layouts: &down_layouts, ..base("bloom down", &down_mod, "vs_fullscreen", "fs_down") },
        );
        let up_pipe =
            pipeline(&device, PipelineDesc { layouts: &up_layouts, ..base("bloom up", &up_mod, "vs_fullscreen", "fs_up") });
        let composite_pipe = pipeline(
            &device,
            PipelineDesc {
                layouts: &comp_layouts,
                format: output_format,
                ..base("composite", &comp_mod, "vs_fullscreen", "fs_composite")
            },
        );

        let (gv, gi) = gpu::ground_mesh();
        let ground = GpuMesh {
            vb: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("ground vb"),
                contents: bytemuck::cast_slice(&gv),
                usage: wgpu::BufferUsages::VERTEX,
            }),
            ib: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("ground ib"),
                contents: bytemuck::cast_slice(&gi),
                usage: wgpu::BufferUsages::INDEX,
            }),
            count: gi.len() as u32,
        };
        use procgen::VehicleKind as V;
        let meshes = vec![
            upload_mesh(&device, "car", &procgen::car(&procgen::CAR_DEFAULT)),
            upload_mesh(&device, "palm", &procgen::palm()),
            upload_mesh(&device, "coupe", &procgen::vehicle(V::Coupe)),
            upload_mesh(&device, "van", &procgen::vehicle(V::Van)),
            upload_mesh(&device, "truck", &procgen::vehicle(V::Truck)),
            upload_mesh(&device, "sports", &procgen::vehicle(V::Sports)),
        ];
        let instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("instances"),
            size: (MAX_INSTANCES * std::mem::size_of::<Instance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let particle_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("particles"),
            size: (MAX_PARTICLES * std::mem::size_of::<Particle>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sized = SizedResources::new(
            &device,
            hdr_format,
            &Layouts { down: &down_bgl, up: &up_bgl, composite: &composite_bgl, sampler: &sampler, post_ubo: &post_ubo },
            width,
            height,
        );
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
            down_bgl,
            up_bgl,
            composite_bgl,
            sky_pipe,
            ground_pipe,
            mesh_pipe,
            particle_pipe,
            down_pipe,
            up_pipe,
            composite_pipe,
            ground,
            meshes,
            instance_buf,
            particle_buf,
            batches: vec![(0, 0); 2 + TRAFFIC_KINDS],
            particle_count: 0,
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

    pub fn resize(&mut self, width: u32, height: u32) {
        if (width.max(1), height.max(1)) == self.size() {
            return;
        }
        let l = Layouts {
            down: &self.down_bgl,
            up: &self.up_bgl,
            composite: &self.composite_bgl,
            sampler: &self.sampler,
            post_ubo: &self.post_ubo,
        };
        self.sized = SizedResources::new(&self.device, self.hdr_format, &l, width, height);
    }

    fn upload(&mut self, p: &FrameParams) {
        let (w, h) = self.size();
        let aspect = w as f32 / h as f32;
        let fov = p.camera.fov_deg.to_radians();
        let eye = Vec3::new(p.camera.x + p.fx.shake[0], p.camera.height + p.fx.shake[1], 0.0);
        let cp = p.camera.pitch.cos();
        let dir = Vec3::new(p.camera.yaw.sin() * cp, p.camera.pitch.sin(), p.camera.yaw.cos() * cp);
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
        let s2 = p.speed01 * p.speed01;
        let pu = PostUniform {
            a: [0.05 + 0.02 * p.fx.nitro, 0.95, 1.0 / BLOOM_LEVELS as f32, if self.manual_srgb { 1.0 } else { 0.0 }],
            b: [0.55, 0.035, 0.004 * p.speed01 + 0.008 * p.fx.nitro, 0.035 * s2 + 0.05 * p.fx.nitro],
            flash: p.fx.flash,
        };
        self.queue.write_buffer(&self.post_ubo, 0, bytemuck::bytes_of(&pu));

        // Pack all instances into one buffer: car, palms, then each traffic kind.
        let mut packed: Vec<Instance> = Vec::with_capacity(MAX_INSTANCES);
        let push = |slot: usize, items: &[Instance], cap: usize, batches: &mut Vec<(u32, u32)>, packed: &mut Vec<Instance>| {
            let n = items.len().min(cap);
            batches[slot] = (packed.len() as u32, n as u32);
            packed.extend_from_slice(&items[..n]);
        };
        push(SLOT_CAR, std::slice::from_ref(&p.car), 1, &mut self.batches, &mut packed);
        push(SLOT_PALM, p.palms, MAX_PALMS, &mut self.batches, &mut packed);
        for (k, items) in p.traffic.iter().enumerate() {
            push(SLOT_TRAFFIC + k, items, MAX_TRAFFIC_PER_KIND, &mut self.batches, &mut packed);
        }
        self.queue.write_buffer(&self.instance_buf, 0, bytemuck::cast_slice(&packed));
        let np = p.particles.len().min(MAX_PARTICLES);
        if np > 0 {
            self.queue.write_buffer(&self.particle_buf, 0, bytemuck::cast_slice(&p.particles[..np]));
        }
        self.particle_count = np as u32;
        self.stats.instances = packed.len() as u32;
        self.stats.particles = np as u32;
    }

    /// Renders one frame into `output` (a surface texture view or an offscreen target).
    pub fn render(&mut self, output: &wgpu::TextureView, p: &FrameParams) {
        self.upload(p);
        let mut draws = 0u32;
        let mut passes = 0u32;
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
            pass.set_vertex_buffer(0, self.ground.vb.slice(..));
            pass.set_index_buffer(self.ground.ib.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..self.ground.count, 0, 0..1);
            draws += 2;

            pass.set_pipeline(&self.mesh_pipe);
            pass.set_vertex_buffer(1, self.instance_buf.slice(..));
            for (mesh, &(first, count)) in self.meshes.iter().zip(self.batches.iter()) {
                if count == 0 {
                    continue;
                }
                pass.set_vertex_buffer(0, mesh.vb.slice(..));
                pass.set_index_buffer(mesh.ib.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..mesh.count, 0, first..first + count);
                draws += 1;
            }

            if self.particle_count > 0 {
                pass.set_pipeline(&self.particle_pipe);
                pass.set_vertex_buffer(0, self.particle_buf.slice(..));
                pass.draw(0..6, 0..self.particle_count);
                draws += 1;
            }
            passes += 1;
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
            draws += 1;
            passes += 1;
        };

        for i in 0..BLOOM_LEVELS {
            fullscreen(&mut enc, "bloom down", &s.down[i].view, &self.down_pipe, &[&s.down_bgs[i]]);
        }
        for i in (0..BLOOM_LEVELS - 1).rev() {
            fullscreen(&mut enc, "bloom up", &s.up[i].view, &self.up_pipe, &[&s.up_bgs[i]]);
        }
        fullscreen(&mut enc, "composite", output, &self.composite_pipe, &[&self.frame_bg, &s.composite_bg]);

        self.queue.submit(Some(enc.finish()));
        self.stats.draw_calls = draws;
        self.stats.passes = passes;
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }
}
