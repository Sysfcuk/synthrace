//! Small wgpu helpers shared by the renderer: shader modules, bind-group entries, pipelines,
//! render targets, mesh upload and the ground lattice.

use crate::shaders;
use wgpu::util::DeviceExt;

pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth24Plus;
const GROUND_COLS: u32 = 161;
const GROUND_ROWS: u32 = 253;
const GROUND_CELL_Z: f32 = 5.0;

pub struct Target {
    _tex: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub width: u32,
    pub height: u32,
}

pub struct GpuMesh {
    pub vb: wgpu::Buffer,
    pub ib: wgpu::Buffer,
    pub count: u32,
}

pub fn shader(device: &wgpu::Device, name: &str) -> wgpu::ShaderModule {
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(name),
        source: wgpu::ShaderSource::Wgsl(shaders::compose(name).into()),
    })
}

pub fn tex_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
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

pub fn sampler_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    }
}

pub fn ubo_entry(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
        count: None,
    }
}

pub fn depth_state(write: bool, compare: wgpu::CompareFunction) -> wgpu::DepthStencilState {
    wgpu::DepthStencilState {
        format: DEPTH_FORMAT,
        depth_write_enabled: Some(write),
        depth_compare: Some(compare),
        stencil: Default::default(),
        bias: Default::default(),
    }
}

pub const ADDITIVE: wgpu::BlendState = wgpu::BlendState {
    color: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    },
    alpha: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    },
};

pub struct PipelineDesc<'a> {
    pub label: &'a str,
    pub module: &'a wgpu::ShaderModule,
    pub vs: &'a str,
    pub fs: &'a str,
    pub layouts: &'a [Option<&'a wgpu::BindGroupLayout>],
    pub buffers: &'a [Option<wgpu::VertexBufferLayout<'a>>],
    pub format: wgpu::TextureFormat,
    pub depth: Option<wgpu::DepthStencilState>,
    pub blend: Option<wgpu::BlendState>,
}

pub fn pipeline(device: &wgpu::Device, d: PipelineDesc) -> wgpu::RenderPipeline {
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(d.label),
        bind_group_layouts: d.layouts,
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(d.label),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: d.module,
            entry_point: Some(d.vs),
            compilation_options: Default::default(),
            buffers: d.buffers,
        },
        primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
        depth_stencil: d.depth,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: d.module,
            entry_point: Some(d.fs),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState { format: d.format, blend: d.blend, write_mask: wgpu::ColorWrites::ALL })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

pub fn make_target(device: &wgpu::Device, label: &str, w: u32, h: u32, format: wgpu::TextureFormat) -> Target {
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

pub fn upload_mesh(device: &wgpu::Device, label: &str, m: &procgen::Mesh) -> GpuMesh {
    let vb = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents: bytemuck::cast_slice(&m.vertices),
        usage: wgpu::BufferUsages::VERTEX,
    });
    let ib = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents: bytemuck::cast_slice(&m.indices),
        usage: wgpu::BufferUsages::INDEX,
    });
    GpuMesh { vb, ib, count: m.indices.len() as u32 }
}

/// Ground lattice: lateral columns spread non-linearly (dense near the road, wide at the edges),
/// rows every 5 m out to ~1.26 km.
pub fn ground_mesh() -> (Vec<[f32; 2]>, Vec<u32>) {
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
