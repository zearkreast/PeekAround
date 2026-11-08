//! The photo shader on the GPU through wgpu: a headless device renders into an offscreen
//! texture that is read back (the preview at screen resolution, the export in full-size tiles).
//! The source is uploaded once with a mip chain built by render passes.

use std::sync::Arc;

use image::RgbaImage;

use super::render::{mip_levels, RenderParams, Renderer, MIP_SHADER, SHADER};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

struct Source {
    view: wgpu::TextureView,
    levels: u32,
    tex_scale: f64,
}

pub struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    name: String,
    max_tex: u32,
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    mip_pipeline: wgpu::RenderPipeline,
    mip_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniforms: wgpu::Buffer,
    source: Option<Source>,
    /// reused output texture + readback buffer (preview renders keep the same size)
    target: Option<(u32, u32, wgpu::Texture, wgpu::Buffer)>,
}

fn make_pipeline(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, src: &str, label: &str) -> wgpu::RenderPipeline {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(src.into()),
    });
    let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &[Some(layout)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(&pl),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: FORMAT,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
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

impl Gpu {
    pub fn new() -> Result<Gpu, String> {
        // DX12 first on Windows: the Vulkan loader there logs errors for broken implicit layers
        let first = if cfg!(windows) { wgpu::Backends::DX12 } else { wgpu::Backends::all() };
        let request = |backends: wgpu::Backends| {
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
                backends,
                ..wgpu::InstanceDescriptor::new_without_display_handle()
            });
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                ..Default::default()
            }))
        };
        let adapter = request(first)
            .or_else(|_| request(wgpu::Backends::all()))
            .map_err(|e| e.to_string())?;
        let info = adapter.get_info();
        if info.device_type == wgpu::DeviceType::Cpu {
            return Err(format!("software adapter {}", info.name));
        }
        let limits = adapter.limits();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("photo-editor"),
            required_limits: wgpu::Limits {
                max_texture_dimension_2d: limits.max_texture_dimension_2d,
                ..wgpu::Limits::downlevel_defaults()
            },
            ..Default::default()
        }))
        .map_err(|e| e.to_string())?;
        device.on_uncaptured_error(Arc::new(|e| log::error!("photo editor GPU: {e}")));

        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("photo"),
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
                sampler_entry(2),
            ],
        });
        let mip_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mip"),
            entries: &[tex_entry(0), sampler_entry(1)],
        });
        let pipeline = make_pipeline(&device, &layout, SHADER, "photo");
        let mip_pipeline = make_pipeline(&device, &mip_layout, MIP_SHADER, "mip");
        if let Some(e) = pollster::block_on(scope.pop()) {
            return Err(e.to_string());
        }
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("photo"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("photo-uniforms"),
            size: 48 * 4,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Ok(Gpu {
            name: format!("gpu: {} ({:?})", info.name, info.backend),
            max_tex: limits.max_texture_dimension_2d,
            device,
            queue,
            pipeline,
            layout,
            mip_pipeline,
            mip_layout,
            sampler,
            uniforms,
            source: None,
            target: None,
        })
    }

    fn build_mips(&self, tex: &wgpu::Texture, levels: u32) {
        let mut enc = self.device.create_command_encoder(&Default::default());
        for level in 1..levels {
            let src = tex.create_view(&wgpu::TextureViewDescriptor {
                base_mip_level: level - 1,
                mip_level_count: Some(1),
                ..Default::default()
            });
            let dst = tex.create_view(&wgpu::TextureViewDescriptor {
                base_mip_level: level,
                mip_level_count: Some(1),
                ..Default::default()
            });
            let bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("mip"),
                layout: &self.mip_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&src) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                ],
            });
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("mip"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &dst,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&self.mip_pipeline);
            pass.set_bind_group(0, &bg, &[]);
            pass.draw(0..3, 0..1);
        }
        self.queue.submit([enc.finish()]);
    }

    fn target(&mut self, w: u32, h: u32) -> (wgpu::Texture, wgpu::Buffer) {
        if let Some((tw, th, t, b)) = &self.target {
            if *tw == w && *th == h {
                return (t.clone(), b.clone());
            }
        }
        let tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("photo-out"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("photo-readback"),
            size: (padded_row(w) * h) as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.target = Some((w, h, tex.clone(), buf.clone()));
        (tex, buf)
    }
}

fn padded_row(w: u32) -> u32 {
    (w * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT
}

impl Renderer for Gpu {
    fn name(&self) -> String {
        self.name.clone()
    }

    fn max_tile(&self) -> u32 {
        self.max_tex.min(4096)
    }

    fn set_source(&mut self, img: Arc<RgbaImage>) {
        let (sw, sh) = img.dimensions();
        let scaled;
        let img: &RgbaImage = if sw.max(sh) > self.max_tex {
            let k = self.max_tex as f64 / sw.max(sh) as f64;
            let (w, h) = (((sw as f64 * k) as u32).max(1), ((sh as f64 * k) as u32).max(1));
            scaled = image::imageops::resize(&*img, w, h, image::imageops::FilterType::Triangle);
            &scaled
        } else {
            &img
        };
        let (w, h) = img.dimensions();
        let levels = mip_levels(w, h);
        let tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("photo-source"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: levels,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            img.as_raw(),
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(h) },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        self.build_mips(&tex, levels);
        self.source = Some(Source {
            view: tex.create_view(&Default::default()),
            levels,
            tex_scale: w as f64 / sw as f64,
        });
    }

    fn has_source(&self) -> bool {
        self.source.is_some()
    }

    fn clear_source(&mut self) {
        self.source = None;
        self.target = None;
    }

    fn render(&mut self, p: &RenderParams, w: u32, h: u32) -> Vec<u8> {
        let (w, h) = (w.clamp(1, self.max_tex), h.clamp(1, self.max_tex));
        let Some(src) = &self.source else {
            return vec![0; (w * h * 4) as usize];
        };
        let u = p.uniforms(src.tex_scale, (src.levels - 1) as f32);
        let mut bytes = [0u8; 48 * 4];
        for (i, v) in u.iter().enumerate() {
            bytes[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        self.queue.write_buffer(&self.uniforms, 0, &bytes);
        let bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("photo"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.uniforms.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&src.view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        });
        let (tex, buf) = self.target(w, h);
        let view = tex.create_view(&Default::default());
        let mut enc = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("photo"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bg, &[]);
            pass.draw(0..3, 0..1);
        }
        let row = padded_row(w);
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(h) },
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        self.queue.submit([enc.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        buf.map_async(wgpu::MapMode::Read, .., move |r| {
            let _ = tx.send(r);
        });
        let _ = self.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
        let mut out = vec![0u8; (w * h * 4) as usize];
        if let Ok(Ok(())) = rx.recv() {
            if let Ok(data) = buf.get_mapped_range(..) {
                for y in 0..h as usize {
                    let s = y * row as usize;
                    out[y * w as usize * 4..(y + 1) * w as usize * 4].copy_from_slice(&data[s..s + w as usize * 4]);
                }
            }
            buf.unmap();
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::doc::{effective_adjust, Filter, Rect};
    use crate::editor::render::Cpu;

    /// The WGSL and the CPU path agree (skipped where no adapter exists).
    #[test]
    fn gpu_matches_cpu() {
        let Ok(mut gpu) = Gpu::new() else {
            eprintln!("no GPU adapter; skipped");
            return;
        };
        let img = Arc::new(RgbaImage::from_fn(96, 64, |x, y| {
            image::Rgba([(x * 255 / 95) as u8, (y * 255 / 63) as u8, ((x + y) * 2) as u8, 255])
        }));
        let mut cpu = Cpu::default();
        cpu.set_source(img.clone());
        gpu.set_source(img);
        let mut a = [0.0; 15];
        a[0] = 20.0;
        a[2] = 30.0;
        a[7] = -40.0;
        a[9] = 25.0;
        a[11] = 50.0;
        let full = Rect { x: 0.0, y: 0.0, w: 96.0, h: 64.0 };
        let p = RenderParams {
            adjust: effective_adjust(&a, Some(&Filter { id: Some("film".into()), strength: 80.0 })),
            src_w: 96.0,
            src_h: 64.0,
            rot: 1,
            flip_h: true,
            flip_v: false,
            straighten: 4.0,
            frame_w: 64.0,
            frame_h: 96.0,
            ox: 0.0,
            oy: 0.0,
            k: 1.0,
            clip: Rect { w: 64.0, h: 96.0, ..full },
            crop: Rect { w: 64.0, h: 96.0, ..full },
            grain_cell: 1.0,
            sharpen_step: 1.0,
        };
        let g = gpu.render(&p, 64, 96);
        let c = cpu.render(&p, 64, 96);
        let mut big = 0;
        for (x, y) in g.iter().zip(&c) {
            if (*x as i32 - *y as i32).abs() > 3 {
                big += 1;
            }
        }
        assert!(big * 100 < g.len(), "{big} of {} channels differ by > 3", g.len());
    }
}
