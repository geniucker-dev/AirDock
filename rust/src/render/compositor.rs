// SPDX-License-Identifier: MPL-2.0
//! Application-owned compositor using Iced's public renderer/engine interfaces.
//! One device serves both UI and video; no second video window or swapchain.
use iced_wgpu::{
    Engine, core,
    graphics::{self, Shell, Viewport, compositor},
    wgpu,
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering},
    },
    time::{Duration, Instant},
};
pub static STATUS: AtomicU8 = AtomicU8::new(0); // 1 GPU, 2 software GPU, 3 software UI only, 4 recovering, 5 failed
pub static PREFERENCE: AtomicU8 = AtomicU8::new(0); // balanced, low power, high performance
pub static ADAPTER_NAME: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());
pub static DEVICE: AtomicU32 = AtomicU32::new(0);
pub static VENDOR: AtomicU32 = AtomicU32::new(0);
pub type Renderer = iced_renderer::fallback::Renderer<GpuRenderer, iced_tiny_skia::Renderer>;
pub struct GpuRenderer(
    pub iced_wgpu::Renderer,
    // Iced batches weak paragraph references. Keep the submitted text alive
    // until reset: a newer UI layout can otherwise invalidate a pending
    // screenshot/refresh before the old renderer batch is consumed.
    Vec<<iced_wgpu::Renderer as core::text::Renderer>::Paragraph>,
);
impl std::fmt::Debug for GpuRenderer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AirPlay shared wgpu renderer")
    }
}
impl core::Renderer for GpuRenderer {
    fn start_layer(&mut self, r: core::Rectangle) {
        self.0.start_layer(r)
    }
    fn end_layer(&mut self) {
        self.0.end_layer()
    }
    fn start_transformation(&mut self, t: core::Transformation) {
        self.0.start_transformation(t)
    }
    fn end_transformation(&mut self) {
        self.0.end_transformation()
    }
    fn fill_quad(&mut self, q: core::renderer::Quad, b: impl Into<core::Background>) {
        self.0.fill_quad(q, b)
    }
    fn reset(&mut self, r: core::Rectangle) {
        self.1.clear();
        self.0.reset(r)
    }
    fn allocate_image(
        &mut self,
        h: &core::image::Handle,
        c: impl FnOnce(Result<core::image::Allocation, core::image::Error>) + Send + 'static,
    ) {
        self.0.allocate_image(h, c)
    }
}
impl core::text::Renderer for GpuRenderer {
    type Font = core::Font;
    type Paragraph = <iced_wgpu::Renderer as core::text::Renderer>::Paragraph;
    type Editor = <iced_wgpu::Renderer as core::text::Renderer>::Editor;
    const ICON_FONT: core::Font = <iced_wgpu::Renderer as core::text::Renderer>::ICON_FONT;
    const CHECKMARK_ICON: char = <iced_wgpu::Renderer as core::text::Renderer>::CHECKMARK_ICON;
    const ARROW_DOWN_ICON: char = <iced_wgpu::Renderer as core::text::Renderer>::ARROW_DOWN_ICON;
    const ICED_LOGO: char = <iced_wgpu::Renderer as core::text::Renderer>::ICED_LOGO;
    const SCROLL_UP_ICON: char = <iced_wgpu::Renderer as core::text::Renderer>::SCROLL_UP_ICON;
    const SCROLL_DOWN_ICON: char = <iced_wgpu::Renderer as core::text::Renderer>::SCROLL_DOWN_ICON;
    const SCROLL_LEFT_ICON: char = <iced_wgpu::Renderer as core::text::Renderer>::SCROLL_LEFT_ICON;
    const SCROLL_RIGHT_ICON: char =
        <iced_wgpu::Renderer as core::text::Renderer>::SCROLL_RIGHT_ICON;
    fn default_font(&self) -> core::Font {
        self.0.default_font()
    }
    fn default_size(&self) -> core::Pixels {
        self.0.default_size()
    }
    fn fill_paragraph(
        &mut self,
        p: &Self::Paragraph,
        pos: core::Point,
        c: core::Color,
        r: core::Rectangle,
    ) {
        self.1.push(p.clone());
        self.0.fill_paragraph(p, pos, c, r)
    }
    fn fill_editor(
        &mut self,
        p: &Self::Editor,
        pos: core::Point,
        c: core::Color,
        r: core::Rectangle,
    ) {
        self.0.fill_editor(p, pos, c, r)
    }
    fn fill_text(
        &mut self,
        t: core::Text<String, core::Font>,
        pos: core::Point,
        c: core::Color,
        r: core::Rectangle,
    ) {
        self.0.fill_text(t, pos, c, r)
    }
}
impl core::image::Renderer for GpuRenderer {
    type Handle = core::image::Handle;
    fn load_image(&self, h: &Self::Handle) -> Result<core::image::Allocation, core::image::Error> {
        self.0.load_image(h)
    }
    fn measure_image(&self, h: &Self::Handle) -> Option<core::Size<u32>> {
        self.0.measure_image(h)
    }
    fn draw_image(&mut self, i: core::Image, r: core::Rectangle, c: core::Rectangle) {
        self.0.draw_image(i, r, c)
    }
}
impl iced_wgpu::primitive::Renderer for GpuRenderer {
    fn draw_primitive(&mut self, b: core::Rectangle, p: impl iced_wgpu::primitive::Primitive) {
        self.0.draw_primitive(b, p)
    }
}
impl compositor::Default for GpuRenderer {
    type Compositor = GpuCompositor;
}
pub struct Surface {
    raw: wgpu::Surface<'static>,
    width: u32,
    height: u32,
}
pub struct GpuCompositor {
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    engine: Engine,
    format: wgpu::TextureFormat,
    settings: graphics::Settings,
    shell: Shell,
    lost: Arc<AtomicBool>,
    last_retry: Instant,
    retries: u8,
    test_loss: Option<Instant>,
}
fn adapter_rank(preference: u8, kind: wgpu::DeviceType, display_match: bool) -> u8 {
    use wgpu::DeviceType::*;
    match (preference, kind) {
        // Balanced follows the display, then uses integrated as an economical fallback.
        (0, IntegratedGpu | DiscreteGpu) if display_match => 0,
        (2, DiscreteGpu) | (1, IntegratedGpu) => 0,
        (_, IntegratedGpu) => 1,
        (_, DiscreteGpu) => 2,
        (_, VirtualGpu) => 3,
        (_, Cpu) => 10,
        _ => 5,
    }
}
impl GpuCompositor {
    async fn engine(
        adapter: &wgpu::Adapter,
        format: wgpu::TextureFormat,
        settings: graphics::Settings,
        shell: Shell,
        lost: Arc<AtomicBool>,
    ) -> Result<(wgpu::Device, Engine), wgpu::RequestDeviceError> {
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("AirPlay UI and video device"),
                required_limits: wgpu::Limits::default(),
                memory_hints: wgpu::MemoryHints::MemoryUsage,
                ..Default::default()
            })
            .await?;
        let error_lost = lost.clone();
        device.on_uncaptured_error(Arc::new(move |error: wgpu::Error| {
            tracing::error!("GPU resource error: {error}");
            error_lost.store(true, Ordering::Release);
            STATUS.store(4, Ordering::Release);
        }));
        device.set_device_lost_callback(move |reason, _| {
            if reason != wgpu::DeviceLostReason::Destroyed {
                lost.store(true, Ordering::Release);
                STATUS.store(4, Ordering::Release);
            }
        });
        let engine = Engine::new(
            adapter,
            device.clone(),
            queue,
            format,
            settings.antialiasing,
            shell,
        );
        Ok((device, engine))
    }
}
impl graphics::Compositor for GpuCompositor {
    type Renderer = GpuRenderer;
    type Surface = Surface;
    async fn with_backend(
        settings: graphics::Settings,
        _: impl compositor::Display + Clone,
        window: impl compositor::Window + Clone,
        shell: Shell,
        backend: Option<&str>,
    ) -> Result<Self, graphics::Error> {
        let init=async {
            if backend.is_some_and(|b|b!="wgpu"){return Err("Requested software UI renderer".to_owned())}
            let backends=wgpu::Backends::from_env().unwrap_or(if cfg!(windows){wgpu::Backends::DX12|wgpu::Backends::VULKAN}else{wgpu::Backends::VULKAN|wgpu::Backends::GL});
            let instance=wgpu::Instance::new(&wgpu::InstanceDescriptor{backends,..Default::default()});
            #[cfg(windows)]
            let display_adapter=crate::platform::window_adapter(&window);
            #[cfg(not(windows))]
            let display_adapter=None::<(u32,u32)>;
            let surface=instance.create_surface(window).map_err(|e|e.to_string())?;
            let mut adapters=instance.enumerate_adapters(backends);adapters.retain(|a|a.is_surface_supported(&surface));adapters.sort_by_key(|a|{let i=a.get_info();adapter_rank(PREFERENCE.load(Ordering::Relaxed),i.device_type,display_adapter==Some((i.vendor,i.device)))});
            for adapter in adapters {
                let capabilities=surface.get_capabilities(&adapter);
                let format=capabilities.formats.iter().copied().find(|f|f.is_srgb()==graphics::color::GAMMA_CORRECTION).or(capabilities.formats.first().copied());
                let Some(format)=format else{continue};let lost=Arc::new(AtomicBool::new(false));
                match Self::engine(&adapter,format,settings,shell.clone(),lost.clone()).await {
                    Ok((device,engine))=>{let info=adapter.get_info();*ADAPTER_NAME.lock().unwrap()=format!("{} ({:?})",info.name,info.backend);VENDOR.store(info.vendor,Ordering::Release);DEVICE.store(info.device,Ordering::Release);STATUS.store(if info.device_type==wgpu::DeviceType::Cpu{2}else{1},Ordering::Release);tracing::info!("Render adapter: {} ({:?}, vendor {:x})",info.name,info.backend,info.vendor);return Ok(Self{instance,adapter,device,engine,format,settings,shell,lost,last_retry:Instant::now()-Duration::from_secs(5),retries:0,test_loss:std::env::var("AIRPLAY_GPU_TEST_LOSS_AFTER_MS").ok().and_then(|s|s.parse::<u64>().ok()).map(|ms|Instant::now()+Duration::from_millis(ms))});},
                    Err(e)=>tracing::warn!("GPU initialization failed on {}: {e}",adapter.get_info().name),
                }
            }
            Err("No usable wgpu adapter; custom YUV video requires a compatible GPU. Software UI remains available.".to_owned())
        }.await;
        init.map_err(|message| {
            STATUS.store(3, Ordering::Release);
            graphics::Error::GraphicsAdapterNotFound {
                backend: "wgpu",
                reason: graphics::error::Reason::RequestFailed(message),
            }
        })
    }
    fn create_renderer(&self) -> GpuRenderer {
        GpuRenderer(
            iced_wgpu::Renderer::new(
                self.engine.clone(),
                self.settings.default_font,
                self.settings.default_text_size,
            ),
            Vec::new(),
        )
    }
    fn create_surface<W: compositor::Window + Clone>(
        &mut self,
        window: W,
        width: u32,
        height: u32,
    ) -> Surface {
        let mut s = Surface {
            raw: self
                .instance
                .create_surface(window)
                .expect("Window surface"),
            width,
            height,
        };
        if width > 0 && height > 0 {
            self.configure_surface(&mut s, width, height)
        }
        s
    }
    fn configure_surface(&mut self, s: &mut Surface, width: u32, height: u32) {
        s.width = width;
        s.height = height;
        if width == 0 || height == 0 || self.lost.load(Ordering::Acquire) {
            return;
        }
        let caps = s.raw.get_capabilities(&self.adapter);
        s.raw.configure(
            &self.device,
            &wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format: self.format,
                width,
                height,
                present_mode: if self.settings.vsync {
                    // Mailbox retains the latest submitted frame instead of FIFO backlog.
                    if caps.present_modes.contains(&wgpu::PresentMode::Mailbox) {
                        wgpu::PresentMode::Mailbox
                    } else {
                        wgpu::PresentMode::AutoVsync
                    }
                } else {
                    wgpu::PresentMode::AutoNoVsync
                },
                alpha_mode: caps
                    .alpha_modes
                    .first()
                    .copied()
                    .unwrap_or(wgpu::CompositeAlphaMode::Auto),
                view_formats: vec![],
                desired_maximum_frame_latency: 1,
            },
        );
    }
    fn information(&self) -> compositor::Information {
        let info = self.adapter.get_info();
        compositor::Information {
            adapter: info.name,
            backend: format!("{:?}", info.backend),
        }
    }
    fn present(
        &mut self,
        r: &mut GpuRenderer,
        s: &mut Surface,
        v: &Viewport,
        c: core::Color,
        pre: impl FnOnce(),
    ) -> Result<(), compositor::SurfaceError> {
        if self.test_loss.is_some_and(|at| Instant::now() >= at) {
            self.test_loss = None;
            tracing::warn!("Synthetic acceptance fault: destroying the wgpu device");
            self.device.destroy();
            self.lost.store(true, Ordering::Release);
            STATUS.store(4, Ordering::Release);
        }
        if self.lost.load(Ordering::Acquire) {
            if self.retries >= 3 {
                STATUS.store(5, Ordering::Release);
                return Err(compositor::SurfaceError::Timeout);
            }
            if self.last_retry.elapsed() < Duration::from_secs(1) {
                return Err(compositor::SurfaceError::Timeout);
            }
            self.last_retry = Instant::now();
            self.retries += 1;
            match futures::executor::block_on(Self::engine(
                &self.adapter,
                self.format,
                self.settings,
                self.shell.clone(),
                self.lost.clone(),
            )) {
                Ok((device, engine)) => {
                    tracing::warn!("GPU device reconstructed; rebuilding renderer resources");
                    self.device = device;
                    self.engine = engine;
                    self.lost.store(false, Ordering::Release);
                    *r = self.create_renderer();
                    self.configure_surface(s, s.width, s.height);
                    STATUS.store(
                        if self.adapter.get_info().device_type == wgpu::DeviceType::Cpu {
                            2
                        } else {
                            1
                        },
                        Ordering::Release,
                    );
                    return Err(compositor::SurfaceError::Lost);
                }
                Err(e) => {
                    tracing::error!("GPU recovery failed: {e}");
                    if self.retries >= 3 {
                        STATUS.store(5, Ordering::Release);
                    }
                    return Err(compositor::SurfaceError::Timeout);
                }
            }
        }
        if self.last_retry.elapsed() > Duration::from_secs(30) {
            self.retries = 0;
        }
        iced_wgpu::window::compositor::present(&mut r.0, &mut s.raw, v, c, pre)
    }
    fn screenshot(&mut self, r: &mut GpuRenderer, v: &Viewport, c: core::Color) -> Vec<u8> {
        r.0.screenshot(v, c)
    }
}
impl core::renderer::Headless for GpuRenderer {
    async fn new(font: core::Font, size: core::Pixels, backend: Option<&str>) -> Option<Self> {
        <iced_wgpu::Renderer as core::renderer::Headless>::new(font, size, backend)
            .await
            .map(|renderer| Self(renderer, Vec::new()))
    }
    fn name(&self) -> String {
        self.0.name()
    }
    fn screenshot(&mut self, size: core::Size<u32>, scale: f32, color: core::Color) -> Vec<u8> {
        <iced_wgpu::Renderer as core::renderer::Headless>::screenshot(
            &mut self.0,
            size,
            scale,
            color,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn balanced_follows_direct_display_and_explicit_preferences_win() {
        use wgpu::DeviceType::*;
        assert!(adapter_rank(0, DiscreteGpu, true) < adapter_rank(0, IntegratedGpu, false));
        assert!(adapter_rank(0, IntegratedGpu, true) < adapter_rank(0, DiscreteGpu, false));
        assert!(adapter_rank(1, IntegratedGpu, false) < adapter_rank(1, DiscreteGpu, true));
        assert!(adapter_rank(2, DiscreteGpu, false) < adapter_rank(2, IntegratedGpu, true));
        assert!(adapter_rank(0, IntegratedGpu, false) < adapter_rank(0, Cpu, true));
    }
}
