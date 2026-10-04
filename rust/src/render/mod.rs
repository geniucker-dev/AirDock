pub mod compositor;
use crate::{
    media::{
        VideoFrame,
        display::{self, Layout},
    },
    state::Shared,
};
use iced::widget::shader;
use iced::{Rectangle, mouse};
use iced_wgpu::{
    primitive::{Pipeline, Primitive},
    wgpu,
};
use std::{
    fmt,
    sync::{Arc, atomic::Ordering},
    time::Instant,
};
#[derive(Clone)]
pub struct Video {
    pub frame: Arc<VideoFrame>,
    pub shared: Arc<Shared>,
    pub crop: bool,
}
impl<Message> shader::Program<Message> for Video {
    type State = ();
    type Primitive = VideoPrimitive;
    fn draw(&self, _: &(), _: mouse::Cursor, _: Rectangle) -> VideoPrimitive {
        VideoPrimitive {
            frame: self.frame.clone(),
            shared: self.shared.clone(),
            crop: self.crop,
        }
    }
}
pub struct VideoPrimitive {
    frame: Arc<VideoFrame>,
    shared: Arc<Shared>,
    crop: bool,
}
impl fmt::Debug for VideoPrimitive {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("YuvVideo")
            .field("sequence", &self.frame.sequence)
            .finish()
    }
}
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Uniforms {
    pub r: [f32; 4],
    pub g: [f32; 4],
    pub b: [f32; 4],
    pub options: [f32; 4],
    pub crop: [f32; 4],
}
#[derive(Clone, Copy)]
struct PreparedFrame {
    epoch: u64,
    sequence: u64,
    received: Instant,
    pts: Option<crate::playback::MediaTime>,
}
pub struct VideoPipeline {
    pub pipeline: wgpu::RenderPipeline,
    pub layout: wgpu::BindGroupLayout,
    pub uniform: wgpu::Buffer,
    pub sampler: wgpu::Sampler,
    pub textures: Option<[wgpu::Texture; 3]>,
    pub bind: Option<wgpu::BindGroup>,
    format: wgpu::TextureFormat,
    key: Option<(u32, u32, Layout)>,
    last: Option<(u64, u64)>,
    prepared: Option<PreparedFrame>,
    rejected: Option<(u64, u64)>,
    area: [f32; 4],
    scratch: [Vec<u8>; 3],
    valid: bool,
}
impl Pipeline for VideoPipeline {
    fn new(device: &wgpu::Device, _: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let entries = (0..3)
            .map(|binding| wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            })
            .chain([
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ])
            .collect::<Vec<_>>();
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("YUV planes"),
            entries: &entries,
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("YUV pipeline"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("YUV SDR conversion"),
            source: wgpu::ShaderSource::Wgsl(include_str!("yuv.wgsl").into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("YUV video"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("YUV colour/crop uniforms"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("YUV scaling"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self {
            pipeline,
            layout,
            uniform,
            sampler,
            textures: None,
            bind: None,
            format,
            key: None,
            last: None,
            prepared: None,
            rejected: None,
            area: [0.; 4],
            scratch: Default::default(),
            valid: false,
        }
    }
}
impl VideoPipeline {
    pub fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        frame: &ffmpeg_next::frame::Video,
        crop: [f32; 4],
    ) -> anyhow::Result<()> {
        let planes = display::describe(frame)?;
        let key = (planes.width, planes.height, planes.layout);
        if self.key != Some(key) {
            let textures = std::array::from_fn(|i| {
                let mut p = planes.planes[i];
                if i == 2 && planes.layout == Layout::Nv12 {
                    p.width = 1;
                    p.height = 1;
                }
                device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("Reusable YUV plane"),
                    size: wgpu::Extent3d {
                        width: p.width,
                        height: p.height,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: if i == 1 && planes.layout == Layout::Nv12 {
                        wgpu::TextureFormat::Rg8Unorm
                    } else {
                        wgpu::TextureFormat::R8Unorm
                    },
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                })
            });
            let views = textures
                .each_ref()
                .map(|t| t.create_view(&Default::default()));
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Reusable YUV bindings"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&views[0]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&views[1]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&views[2]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: self.uniform.as_entire_binding(),
                    },
                ],
            });
            self.textures = Some(textures);
            self.bind = Some(bind);
            self.key = Some(key);
        }
        for (i, p) in planes
            .planes
            .iter()
            .enumerate()
            .take(if planes.layout == Layout::Nv12 { 2 } else { 3 })
        {
            let (data, stride) = if p.stride > 0 {
                let size =
                    (p.height as usize - 1) * p.stride as usize + (p.width * p.bytes) as usize;
                (
                    unsafe { std::slice::from_raw_parts(p.pointer, size) },
                    p.stride as u32,
                )
            } else {
                let row = (p.width * p.bytes) as usize;
                self.scratch[i].resize(row * p.height as usize, 0);
                for y in 0..p.height as usize {
                    let source = unsafe {
                        std::slice::from_raw_parts(
                            p.pointer.offset(y as isize * p.stride as isize),
                            row,
                        )
                    };
                    self.scratch[i][y * row..(y + 1) * row].copy_from_slice(source);
                }
                (&self.scratch[i][..], row as u32)
            };
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.textures.as_ref().unwrap()[i],
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(stride),
                    rows_per_image: Some(p.height),
                },
                wgpu::Extent3d {
                    width: p.width,
                    height: p.height,
                    depth_or_array_layers: 1,
                },
            );
        }
        let params = Uniforms {
            r: planes.matrix[0],
            g: planes.matrix[1],
            b: planes.matrix[2],
            options: [
                if planes.layout == Layout::Nv12 {
                    1.
                } else {
                    0.
                },
                if self.format.is_srgb() { 1. } else { 0. },
                0.,
                0.,
            ],
            crop,
        };
        queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&params));
        self.valid = true;
        Ok(())
    }
}
impl Primitive for VideoPrimitive {
    type Pipeline = VideoPipeline;
    fn prepare(
        &self,
        p: &mut VideoPipeline,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        bounds: &Rectangle,
        viewport: &shader::Viewport,
    ) {
        if self.frame.epoch != self.shared.generation()
            || !self.shared.media.display.visible.load(Ordering::Acquire)
        {
            p.valid = false;
            return;
        }
        // Select again immediately before upload: UI updates may lag the mailbox.
        // Drawing a superseded UI snapshot must not produce a black video pass.
        let frame = self
            .shared
            .media
            .display
            .latest
            .lock()
            .unwrap()
            .as_ref()
            .filter(|f| f.epoch == self.shared.generation())
            .cloned();
        let Some(frame) = frame else {
            p.valid = false;
            return;
        };
        let scale = viewport.scale_factor();
        let (w, h) = (frame.frame.width() as f32, frame.frame.height() as f32);
        let contain = (bounds.width / w).min(bounds.height / h);
        let fill = (bounds.width / w).max(bounds.height / h);
        let (area, crop) = if self.crop {
            let cu = bounds.width / (w * fill);
            let cv = bounds.height / (h * fill);
            (
                [bounds.x, bounds.y, bounds.width, bounds.height],
                [cu, cv, (1. - cu) / 2., (1. - cv) / 2.],
            )
        } else {
            (
                [
                    bounds.x + (bounds.width - w * contain) / 2.,
                    bounds.y + (bounds.height - h * contain) / 2.,
                    w * contain,
                    h * contain,
                ],
                [1., 1., 0., 0.],
            )
        };
        p.area = area.map(|v| v * scale);
        let key = (frame.epoch, frame.sequence);
        if p.rejected == Some(key) {
            p.valid = false;
            return;
        }
        if p.last != Some(key) {
            match p.upload(device, queue, &frame.frame, crop) {
                Ok(()) => {
                    p.last = Some(key);
                    p.rejected = None;
                    p.prepared = Some(PreparedFrame {
                        epoch: frame.epoch,
                        sequence: frame.sequence,
                        received: frame.received,
                        pts: frame.pts,
                    });
                    self.shared.metrics.uploaded.fetch_add(1, Ordering::Relaxed);
                    self.shared
                        .media
                        .display
                        .consumed
                        .store(frame.sequence, Ordering::Release);
                }
                Err(e) => {
                    p.valid = false;
                    if p.rejected == Some(key) {
                        p.valid = false;
                        return;
                    }
                    if p.last != Some(key) {
                        self.shared.report(format!("Video display: {e:#}"));
                        p.rejected = Some(key);
                    }
                }
            }
        } else {
            // Only the geometry uniform changes on resize/crop; planes are not uploaded again.
            queue.write_buffer(&p.uniform, 64, bytemuck::cast_slice(&crop));
            p.valid = true;
        }
    }
    fn draw(&self, p: &VideoPipeline, pass: &mut wgpu::RenderPass<'_>) -> bool {
        let Some(frame) = p.prepared else {
            return true;
        };
        if !p.valid
            || p.last != Some((frame.epoch, frame.sequence))
            || frame.epoch != self.shared.generation()
            || !self.shared.media.display.visible.load(Ordering::Acquire)
        {
            return true;
        }
        if let Some(bind) = &p.bind {
            pass.set_viewport(
                p.area[0],
                p.area[1],
                p.area[2].max(1.),
                p.area[3].max(1.),
                0.,
                1.,
            );
            pass.set_pipeline(&p.pipeline);
            pass.set_bind_group(0, bind, &[]);
            pass.draw(0..6, 0..1);
            // A submission counter, not a physical scanout/FPS measurement.
            if self
                .shared
                .metrics
                .last_submitted_sequence
                .swap(frame.sequence, Ordering::Relaxed)
                != frame.sequence
            {
                self.shared
                    .metrics
                    .presented
                    .fetch_add(1, Ordering::Relaxed);
                if let Some(pts) = frame.pts
                    && let Some(audio) = self.shared.media.audio_clock.position(frame.epoch)
                {
                    self.shared
                        .metrics
                        .estimated_av_offset_us
                        .store(pts.micros().saturating_sub(audio), Ordering::Relaxed);
                    self.shared
                        .metrics
                        .estimated_av_available
                        .store(true, Ordering::Release);
                }
                let latency = frame.received.elapsed().as_micros() as u64;
                self.shared
                    .metrics
                    .latency_us
                    .store(latency, Ordering::Relaxed);
                let mut samples = self.shared.metrics.latency_samples.lock().unwrap();
                if samples.len() == 4096 {
                    samples.pop_front();
                }
                samples.push_back(latency);
                let now = self.shared.started.elapsed().as_micros() as u64;
                let previous = self
                    .shared
                    .metrics
                    .last_submission_us
                    .swap(now, Ordering::Relaxed);
                let previous_epoch = self
                    .shared
                    .metrics
                    .last_submission_epoch
                    .swap(frame.epoch, Ordering::Relaxed);
                if previous != 0 && previous_epoch == frame.epoch {
                    let mut intervals = self.shared.metrics.present_intervals_us.lock().unwrap();
                    if intervals.len() == 4096 {
                        intervals.pop_front();
                    }
                    intervals.push_back(now.saturating_sub(previous));
                }
            }
        }
        true
    }
}
mod native;
pub use native::native_report;
