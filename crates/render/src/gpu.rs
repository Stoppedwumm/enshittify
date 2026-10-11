//! wgpu compositor (preview and export).

use std::collections::HashMap;

use cutroom_core::MediaId;
use cutroom_core::model::BlendMode;
use wgpu::util::DeviceExt;

use crate::color::{YuvMatrix, YuvRange, yuv_to_rgb_matrix};
use crate::plan::{LayerSource, RenderPlan};

pub const OUTPUT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const UNIFORM_STRIDE: u64 = 256;
const MAX_CACHED_TEXTURES: usize = 48;

#[derive(Debug, thiserror::Error)]
pub enum GpuError {
    #[error("no GPU adapter: {0}")]
    NoAdapter(String),
    #[error("device request failed: {0}")]
    Device(String),
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct LayerUniform {
    m0: [f32; 4],
    m1: [f32; 4],
    color: [f32; 4],
    params: [f32; 4],
}

struct CachedTex {
    _tex: wgpu::Texture,
    bind: wgpu::BindGroup,
    last_used: u64,
}

type TexKey = (MediaId, i64, u32, u32);

pub struct Compositor {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipelines: HashMap<BlendMode, wgpu::RenderPipeline>,
    uniform_layout: wgpu::BindGroupLayout,
    tex_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniforms: wgpu::Buffer,
    uniform_bind: wgpu::BindGroup,
    uniform_capacity: u64,
    white: wgpu::BindGroup,
    textures: HashMap<TexKey, CachedTex>,
    output: Option<(wgpu::Texture, u32, u32)>,
    frame_no: u64,
}

fn blend_state(mode: BlendMode) -> wgpu::BlendState {
    use wgpu::{BlendComponent as C, BlendFactor as F, BlendOperation as O};
    let alpha = C {
        src_factor: F::One,
        dst_factor: F::OneMinusSrcAlpha,
        operation: O::Add,
    };
    let color = match mode {
        BlendMode::Normal => alpha,
        BlendMode::Add => C {
            src_factor: F::One,
            dst_factor: F::One,
            operation: O::Add,
        },
        BlendMode::Multiply => C {
            src_factor: F::Dst,
            dst_factor: F::OneMinusSrcAlpha,
            operation: O::Add,
        },
        BlendMode::Screen => C {
            src_factor: F::One,
            dst_factor: F::OneMinusSrc,
            operation: O::Add,
        },
    };
    let alpha = match mode {
        BlendMode::Add => color,
        BlendMode::Screen => C {
            src_factor: F::One,
            dst_factor: F::OneMinusSrcAlpha,
            operation: O::Add,
        },
        _ => alpha,
    };
    wgpu::BlendState { color, alpha }
}

/// Creates a headless device (export, tests).
pub fn headless_device() -> Result<(wgpu::Device, wgpu::Queue), GpuError> {
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        force_fallback_adapter: false,
        compatible_surface: None,
        apply_limit_buckets: false,
    }))
    .map_err(|e| GpuError::NoAdapter(e.to_string()))?;
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("cutroom-headless"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
        ..Default::default()
    }))
    .map_err(|e| GpuError::Device(e.to_string()))
}

impl Compositor {
    pub fn new(device: wgpu::Device, queue: wgpu::Queue) -> Self {
        let shader = device.create_shader_module(wgpu::include_wgsl!("shaders/composite.wgsl"));
        let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("layer-uniform"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(
                        std::mem::size_of::<LayerUniform>() as u64
                    ),
                },
                count: None,
            }],
        });
        let tex_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("layer-texture"),
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
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("composite"),
            bind_group_layouts: &[Some(&uniform_layout), Some(&tex_layout)],
            immediate_size: 0,
        });
        let mut pipelines = HashMap::new();
        for mode in [
            BlendMode::Normal,
            BlendMode::Add,
            BlendMode::Multiply,
            BlendMode::Screen,
        ] {
            let p = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("composite"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: OUTPUT_FORMAT,
                        blend: Some(blend_state(mode)),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            });
            pipelines.insert(mode, p);
        }
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("layer"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let uniform_capacity = 16;
        let (uniforms, uniform_bind) =
            Self::make_uniforms(&device, &uniform_layout, uniform_capacity);
        let white_tex = device.create_texture_with_data(
            &queue,
            &wgpu::TextureDescriptor {
                label: Some("white"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: OUTPUT_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &[255, 255, 255, 255],
        );
        let white = Self::tex_bind(&device, &tex_layout, &sampler, &white_tex);
        Compositor {
            device,
            queue,
            pipelines,
            uniform_layout,
            tex_layout,
            sampler,
            uniforms,
            uniform_bind,
            uniform_capacity,
            white,
            textures: HashMap::new(),
            output: None,
            frame_no: 0,
        }
    }

    pub fn new_headless() -> Result<Self, GpuError> {
        let (d, q) = headless_device()?;
        Ok(Self::new(d, q))
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    fn make_uniforms(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        capacity: u64,
    ) -> (wgpu::Buffer, wgpu::BindGroup) {
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("layer-uniforms"),
            size: capacity * UNIFORM_STRIDE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("layer-uniforms"),
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &buf,
                    offset: 0,
                    size: wgpu::BufferSize::new(std::mem::size_of::<LayerUniform>() as u64),
                }),
            }],
        });
        (buf, bind)
    }

    fn tex_bind(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        sampler: &wgpu::Sampler,
        tex: &wgpu::Texture,
    ) -> wgpu::BindGroup {
        let view = tex.create_view(&Default::default());
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("layer-texture"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        })
    }

    fn ensure_output(&mut self, w: u32, h: u32) {
        if matches!(self.output, Some((_, ow, oh)) if ow == w && oh == h) {
            return;
        }
        let tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("composite-output"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: OUTPUT_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        self.output = Some((tex, w, h));
    }

    fn upload(&mut self, key: TexKey, f: &cutroom_media::VideoFrame) {
        if let Some(t) = self.textures.get_mut(&key) {
            t.last_used = self.frame_no;
            return;
        }
        let tex = self.device.create_texture_with_data(
            &self.queue,
            &wgpu::TextureDescriptor {
                label: Some("frame"),
                size: wgpu::Extent3d {
                    width: f.width,
                    height: f.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: OUTPUT_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &f.rgba,
        );
        let bind = Self::tex_bind(&self.device, &self.tex_layout, &self.sampler, &tex);
        self.textures.insert(
            key,
            CachedTex {
                _tex: tex,
                bind,
                last_used: self.frame_no,
            },
        );
        if self.textures.len() > MAX_CACHED_TEXTURES {
            let mut ages: Vec<(u64, TexKey)> = self
                .textures
                .iter()
                .map(|(k, v)| (v.last_used, *k))
                .collect();
            ages.sort();
            for (_, k) in ages.iter().take(self.textures.len() - MAX_CACHED_TEXTURES) {
                self.textures.remove(k);
            }
        }
    }

    /// Executes a plan into the output texture and returns it.
    pub fn render(&mut self, plan: &RenderPlan) -> &wgpu::Texture {
        self.frame_no += 1;
        self.ensure_output(plan.width.max(1), plan.height.max(1));
        let n = plan.layers.len() as u64;
        if n > self.uniform_capacity {
            self.uniform_capacity = n.next_power_of_two();
            let (b, g) =
                Self::make_uniforms(&self.device, &self.uniform_layout, self.uniform_capacity);
            self.uniforms = b;
            self.uniform_bind = g;
        }
        let mut data = vec![0u8; (n.max(1) * UNIFORM_STRIDE) as usize];
        for (i, l) in plan.layers.iter().enumerate() {
            let m = l.xform.0.map(|v| v as f32);
            let (color, mode) = match &l.source {
                LayerSource::Solid(c) => (*c, 1.0),
                LayerSource::Frame(_) => ([1.0; 4], 0.0),
            };
            let u = LayerUniform {
                m0: [m[0], m[1], m[2], 0.0],
                m1: [m[3], m[4], m[5], 0.0],
                color,
                params: [l.opacity, mode, plan.width as f32, plan.height as f32],
            };
            let off = i * UNIFORM_STRIDE as usize;
            data[off..off + std::mem::size_of::<LayerUniform>()]
                .copy_from_slice(bytemuck::bytes_of(&u));
            if let (LayerSource::Frame(f), Some((m, idx))) = (&l.source, l.source_key) {
                self.upload((m, idx, f.width, f.height), f);
            }
        }
        self.queue.write_buffer(&self.uniforms, 0, &data);

        let view = self
            .output
            .as_ref()
            .expect("ensured")
            .0
            .create_view(&Default::default());
        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("composite"),
            });
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("composite"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            for (i, l) in plan.layers.iter().enumerate() {
                let bind = match (&l.source, l.source_key) {
                    (LayerSource::Frame(f), Some((m, idx))) => {
                        &self.textures[&(m, idx, f.width, f.height)].bind
                    }
                    (LayerSource::Frame(_), None) => continue,
                    (LayerSource::Solid(_), _) => &self.white,
                };
                pass.set_pipeline(&self.pipelines[&l.blend]);
                pass.set_bind_group(0, &self.uniform_bind, &[(i as u64 * UNIFORM_STRIDE) as u32]);
                pass.set_bind_group(1, bind, &[]);
                pass.draw(0..6, 0..1);
            }
        }
        self.queue.submit([enc.finish()]);
        &self.output.as_ref().expect("ensured").0
    }

    /// Copies the last output back to the CPU as tightly packed RGBA8.
    pub fn read_output(&self) -> Option<Vec<u8>> {
        let (tex, w, h) = self.output.as_ref()?;
        Some(read_texture(&self.device, &self.queue, tex, *w, *h))
    }
}

/// Blocking texture readback (export, snapshots, tests).
pub fn read_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    tex: &wgpu::Texture,
    w: u32,
    h: u32,
) -> Vec<u8> {
    let row = w * 4;
    let padded =
        row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: (padded * h) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&Default::default());
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buf,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([enc.finish()]);
    let slice = buf.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    let _ = device.poll(wgpu::PollType::wait_indefinitely());
    let data = slice.get_mapped_range().expect("mapped readback buffer");
    let mut out = Vec::with_capacity((row * h) as usize);
    for y in 0..h {
        let s = (y * padded) as usize;
        out.extend_from_slice(&data[s..s + row as usize]);
    }
    drop(data);
    buf.unmap();
    out
}

/// GPU planar-YUV → RGBA converter.
pub struct YuvConverter {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
}

impl YuvConverter {
    pub fn new(device: wgpu::Device, queue: wgpu::Queue) -> Self {
        let shader = device.create_shader_module(wgpu::include_wgsl!("shaders/yuv.wgsl"));
        let tex_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("yuv"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                tex_entry(1),
                tex_entry(2),
                tex_entry(3),
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("yuv"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("yuv"),
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(OUTPUT_FORMAT.into())],
            }),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        YuvConverter {
            device,
            queue,
            pipeline,
            layout,
            sampler,
        }
    }

    /// Converts 8-bit planes (`y` is `w×h`, chroma `cw×ch`) to an RGBA texture.
    #[allow(clippy::too_many_arguments)]
    pub fn convert(
        &self,
        y: &[u8],
        u: &[u8],
        v: &[u8],
        w: u32,
        h: u32,
        cw: u32,
        ch: u32,
        matrix: YuvMatrix,
        range: YuvRange,
    ) -> wgpu::Texture {
        let plane = |data: &[u8], pw, ph| {
            self.device.create_texture_with_data(
                &self.queue,
                &wgpu::TextureDescriptor {
                    label: Some("yuv-plane"),
                    size: wgpu::Extent3d {
                        width: pw,
                        height: ph,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::R8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
                wgpu::util::TextureDataOrder::LayerMajor,
                data,
            )
        };
        let (ty, tu, tv) = (plane(y, w, h), plane(u, cw, ch), plane(v, cw, ch));
        let m = yuv_to_rgb_matrix(matrix, range);
        let ubuf = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("yuv-params"),
                contents: bytemuck::cast_slice(&m),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let views = [&ty, &tu, &tv].map(|t| t.create_view(&Default::default()));
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("yuv"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: ubuf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&views[0]),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&views[1]),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&views[2]),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        let out = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("yuv-out"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: OUTPUT_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = out.create_view(&Default::default());
        let mut enc = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("yuv"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.draw(0..3, 0..1);
        }
        self.queue.submit([enc.finish()]);
        out
    }

    pub fn read(&self, tex: &wgpu::Texture) -> Vec<u8> {
        read_texture(&self.device, &self.queue, tex, tex.width(), tex.height())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{Affine, Layer, RenderPlan};
    use cutroom_core::model::Transform;
    use cutroom_core::{ClipId, Ticks};
    use std::sync::Arc;

    fn gpu() -> Option<(wgpu::Device, wgpu::Queue)> {
        match headless_device() {
            Ok(d) => Some(d),
            Err(e) => {
                eprintln!("skipping GPU test: {e}");
                None
            }
        }
    }

    #[test]
    fn gpu_matches_cpu_reference() {
        let Some((d, q)) = gpu() else { return };
        let mut comp = Compositor::new(d, q);
        let rgba: Vec<u8> = (0..64 * 32)
            .flat_map(|i| [(i % 64 * 4) as u8, (i / 64 * 8) as u8, 77, 255])
            .collect();
        let f = cutroom_media::VideoFrame {
            width: 64,
            height: 32,
            index: 0,
            rgba: Arc::from(rgba),
        };
        let t = Transform {
            scale_x: 0.5,
            scale_y: 0.5,
            rotation: 30.0,
            opacity: 0.8,
            ..Transform::default()
        };
        let plan = RenderPlan {
            width: 64,
            height: 32,
            tick: Ticks::ZERO,
            layers: vec![
                Layer {
                    clip: ClipId::new(),
                    source: LayerSource::Solid([0.2, 0.4, 0.6, 1.0]),
                    source_key: None,
                    xform: Affine::for_layer(64.0, 32.0, 64.0, 32.0, &Transform::default()),
                    opacity: 1.0,
                    blend: BlendMode::Normal,
                },
                Layer {
                    clip: ClipId::new(),
                    source: LayerSource::Frame(f),
                    source_key: Some((MediaId::of_bytes(b"x"), 0)),
                    xform: Affine::for_layer(64.0, 32.0, 64.0, 32.0, &t),
                    opacity: 0.8,
                    blend: BlendMode::Normal,
                },
            ],
            missing: vec![],
        };
        comp.render(&plan);
        let g = comp.read_output().unwrap();
        let c = crate::cpu::render(&plan);
        // Rasterisation rules differ at polygon edges; compare interiors.
        let diff = g
            .iter()
            .zip(&c)
            .filter(|(a, b)| (**a as i32 - **b as i32).abs() > 3)
            .count();
        assert!(diff < g.len() / 50, "{diff} differing channels");
    }

    #[test]
    fn gpu_yuv_matches_cpu() {
        let Some((d, q)) = gpu() else { return };
        let conv = YuvConverter::new(d, q);
        let (w, h) = (4, 2);
        let y = [16, 60, 120, 235, 81, 145, 41, 210];
        let u = [128, 90];
        let v = [128, 240];
        let tex = conv.convert(&y, &u, &v, w, h, 2, 1, YuvMatrix::Bt709, YuvRange::Limited);
        let out = conv.read(&tex);
        let m = yuv_to_rgb_matrix(YuvMatrix::Bt709, YuvRange::Limited);
        // Pixel (0,0) sits on chroma sample 0 exactly.
        let want = crate::color::yuv_to_rgb(&m, 16, 128, 128);
        assert!(
            out[..3]
                .iter()
                .zip(want)
                .all(|(a, b)| (*a as i32 - b as i32).abs() <= 2)
        );
    }
}
