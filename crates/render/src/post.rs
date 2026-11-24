//! Resolution-dependent resources: HDR scene target, depth, bloom mip chain and their bind groups.

use crate::gpu::{DEPTH_FORMAT, Target, make_target};
use wgpu::util::DeviceExt;

pub const BLOOM_LEVELS: usize = 6;

pub struct SizedResources {
    pub hdr: Target,
    pub depth: Target,
    pub down: Vec<Target>,
    pub up: Vec<Target>,
    pub down_bgs: Vec<wgpu::BindGroup>,
    pub up_bgs: Vec<wgpu::BindGroup>,
    pub composite_bg: wgpu::BindGroup,
    _param_bufs: Vec<wgpu::Buffer>,
}

pub struct Layouts<'a> {
    pub down: &'a wgpu::BindGroupLayout,
    pub up: &'a wgpu::BindGroupLayout,
    pub composite: &'a wgpu::BindGroupLayout,
    pub sampler: &'a wgpu::Sampler,
    pub post_ubo: &'a wgpu::Buffer,
}

impl SizedResources {
    pub fn new(device: &wgpu::Device, hdr_format: wgpu::TextureFormat, l: &Layouts, width: u32, height: u32) -> Self {
        let hdr = make_target(device, "hdr", width, height, hdr_format);
        let depth = make_target(device, "depth", width, height, DEPTH_FORMAT);
        let mut down = Vec::new();
        let mut up = Vec::new();
        let (mut w, mut h) = (width, height);
        for i in 0..BLOOM_LEVELS {
            w = (w / 2).max(1);
            h = (h / 2).max(1);
            down.push(make_target(device, &format!("bloom down {i}"), w, h, hdr_format));
            // The smallest level needs no "up" target: up[N-2] reads down[N-1] directly.
            if i < BLOOM_LEVELS - 1 {
                up.push(make_target(device, &format!("bloom up {i}"), w, h, hdr_format));
            }
        }
        let mut bufs = Vec::new();
        let mut ubo = |v: [f32; 4]| {
            bufs.push(device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("bloom params"),
                contents: bytemuck::cast_slice(&v),
                usage: wgpu::BufferUsages::UNIFORM,
            }));
            bufs.len() - 1
        };
        let down_params: Vec<usize> = (0..BLOOM_LEVELS)
            .map(|i| {
                let src = if i == 0 { &hdr } else { &down[i - 1] };
                ubo([1.0 / src.width as f32, 1.0 / src.height as f32, if i == 0 { 1.0 } else { 0.0 }, 0.0])
            })
            .collect();
        let lower = |i: usize| if i + 1 == BLOOM_LEVELS - 1 { &down[i + 1] } else { &up[i + 1] };
        let up_params: Vec<usize> = (0..BLOOM_LEVELS - 1)
            .map(|i| {
                let lo = lower(i);
                ubo([1.0 / lo.width as f32, 1.0 / lo.height as f32, 1.0, 0.0])
            })
            .collect();
        fn tex(t: &Target) -> wgpu::BindingResource<'_> {
            wgpu::BindingResource::TextureView(&t.view)
        }
        let samp = wgpu::BindingResource::Sampler(l.sampler);
        let down_bgs = (0..BLOOM_LEVELS)
            .map(|i| {
                let src = if i == 0 { &hdr } else { &down[i - 1] };
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("bloom down bg"),
                    layout: l.down,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: tex(src) },
                        wgpu::BindGroupEntry { binding: 1, resource: samp.clone() },
                        wgpu::BindGroupEntry { binding: 2, resource: bufs[down_params[i]].as_entire_binding() },
                    ],
                })
            })
            .collect();
        let up_bgs = (0..BLOOM_LEVELS - 1)
            .map(|i| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("bloom up bg"),
                    layout: l.up,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: tex(lower(i)) },
                        wgpu::BindGroupEntry { binding: 1, resource: tex(&down[i]) },
                        wgpu::BindGroupEntry { binding: 2, resource: samp.clone() },
                        wgpu::BindGroupEntry { binding: 3, resource: bufs[up_params[i]].as_entire_binding() },
                    ],
                })
            })
            .collect();
        let composite_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("composite bg"),
            layout: l.composite,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: tex(&hdr) },
                wgpu::BindGroupEntry { binding: 1, resource: tex(&up[0]) },
                wgpu::BindGroupEntry { binding: 2, resource: samp.clone() },
                wgpu::BindGroupEntry { binding: 3, resource: l.post_ubo.as_entire_binding() },
            ],
        });
        Self { hdr, depth, down, up, down_bgs, up_bgs, composite_bg, _param_bufs: bufs }
    }
}
