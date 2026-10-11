//! Preview rendering into an egui texture: wgpu compositor when eframe runs
//! on wgpu, CPU reference rasterizer otherwise. Same `build_plan` either way.

use std::cell::Cell;
use std::sync::Arc;

use cutroom_core::model::DocSnapshot;
use cutroom_core::{MediaId, SeqId, Ticks};
use cutroom_media::VideoFrame;
use cutroom_media::cache::Quality;
use cutroom_media::server::FrameServer;
use cutroom_render::gpu::Compositor;
use cutroom_render::{FrameProvider, RenderPlan, build_plan};
use eframe::egui_wgpu;

/// Non-blocking frame source that records misses (dropped frames).
pub struct PreviewFrames<'a> {
    pub server: &'a FrameServer,
    pub quality: Quality,
    pub requested: Cell<u32>,
    pub missed: Cell<u32>,
}

impl FrameProvider for PreviewFrames<'_> {
    fn frame(&self, media: MediaId, index: i64) -> Option<VideoFrame> {
        self.requested.set(self.requested.get() + 1);
        let f = self.server.request(media, index, self.quality);
        let exact = f.as_ref().is_some_and(|f| {
            f.index == index
                || self
                    .server
                    .info(media)
                    .is_some_and(|i| i.kind == cutroom_core::model::MediaKind::Image)
        });
        if !exact {
            self.missed.set(self.missed.get() + 1);
        }
        f
    }
}

struct Gpu {
    comp: Compositor,
    renderer: Arc<egui::mutex::RwLock<egui_wgpu::Renderer>>,
    device: wgpu::Device,
    tex: Option<(egui::TextureId, u32, u32)>,
}

/// What the CPU path last rendered (skip identical frames).
type CpuKey = (Ticks, u32, u32, Vec<Option<(MediaId, i64)>>, usize);

pub struct Preview {
    gpu: Option<Gpu>,
    cpu: Option<egui::TextureHandle>,
    /// Frames rendered / frames whose sources were not yet decoded.
    pub rendered: u64,
    pub dropped: u64,
    last_cpu_key: Option<CpuKey>,
}

impl Preview {
    pub fn new(rs: Option<&egui_wgpu::RenderState>) -> Self {
        let gpu = rs.map(|rs| Gpu {
            comp: Compositor::new(rs.device.clone(), rs.queue.clone()),
            renderer: rs.renderer.clone(),
            device: rs.device.clone(),
            tex: None,
        });
        Preview {
            gpu,
            cpu: None,
            rendered: 0,
            dropped: 0,
            last_cpu_key: None,
        }
    }

    pub fn backend_name(&self) -> &'static str {
        if self.gpu.is_some() { "GPU" } else { "CPU" }
    }

    /// Renders `seq` at `tick`; returns the texture and its pixel size.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        ctx: &egui::Context,
        snap: &DocSnapshot,
        seq: SeqId,
        tick: Ticks,
        scale: f64,
        frames: &FrameServer,
        quality: Quality,
        playing: bool,
    ) -> Option<(egui::TextureId, egui::Vec2)> {
        let provider = PreviewFrames {
            server: frames,
            quality,
            requested: Cell::new(0),
            missed: Cell::new(0),
        };
        let plan = build_plan(snap, seq, tick, &provider);
        if playing {
            self.rendered += 1;
            if provider.missed.get() > 0 {
                self.dropped += 1;
            }
        }
        // CPU rendering is slow: cap its resolution.
        let scale = if self.gpu.is_some() {
            scale
        } else {
            scale.min(0.5)
        };
        let plan = if scale < 1.0 {
            plan.scaled(scale)
        } else {
            plan
        };
        match self.gpu.as_mut() {
            Some(g) => Some(Self::render_gpu(g, &plan)),
            None => Some(self.render_cpu(ctx, &plan, tick)),
        }
    }

    fn render_gpu(g: &mut Gpu, plan: &RenderPlan) -> (egui::TextureId, egui::Vec2) {
        let tex = g.comp.render(plan);
        let view = tex.create_view(&Default::default());
        let (w, h) = (plan.width, plan.height);
        let mut r = g.renderer.write();
        let id = match g.tex {
            Some((id, tw, th)) if tw == w && th == h => {
                r.update_egui_texture_from_wgpu_texture(
                    &g.device,
                    &view,
                    wgpu::FilterMode::Linear,
                    id,
                );
                id
            }
            Some((id, _, _)) => {
                r.free_texture(&id);
                let id = r.register_native_texture(&g.device, &view, wgpu::FilterMode::Linear);
                g.tex = Some((id, w, h));
                id
            }
            None => {
                let id = r.register_native_texture(&g.device, &view, wgpu::FilterMode::Linear);
                g.tex = Some((id, w, h));
                id
            }
        };
        (id, egui::vec2(w as f32, h as f32))
    }

    fn render_cpu(
        &mut self,
        ctx: &egui::Context,
        plan: &RenderPlan,
        tick: Ticks,
    ) -> (egui::TextureId, egui::Vec2) {
        let key = (
            tick,
            plan.width,
            plan.height,
            plan.layers.iter().map(|l| l.source_key).collect::<Vec<_>>(),
            plan.layers.len(),
        );
        let size = egui::vec2(plan.width as f32, plan.height as f32);
        if let Some(t) = &self.cpu
            && self.last_cpu_key.as_ref() == Some(&key)
        {
            return (t.id(), size);
        }
        let rgba = cutroom_render::cpu::render(plan);
        let img = egui::ColorImage::from_rgba_unmultiplied(
            [plan.width as usize, plan.height as usize],
            &rgba,
        );
        match &mut self.cpu {
            Some(t) => t.set(img, egui::TextureOptions::LINEAR),
            None => self.cpu = Some(ctx.load_texture("preview", img, egui::TextureOptions::LINEAR)),
        }
        self.last_cpu_key = Some(key);
        (self.cpu.as_ref().expect("set").id(), size)
    }

    /// Full-resolution still of the current frame (Snapshot action).
    pub fn snapshot_rgba(
        &mut self,
        snap: &DocSnapshot,
        seq: SeqId,
        tick: Ticks,
        frames: &FrameServer,
    ) -> (u32, u32, Vec<u8>) {
        struct Blocking<'a>(&'a FrameServer);
        impl FrameProvider for Blocking<'_> {
            fn frame(&self, m: MediaId, i: i64) -> Option<VideoFrame> {
                self.0
                    .get_blocking(m, i, Quality::Full, std::time::Duration::from_secs(5))
            }
        }
        let plan = build_plan(snap, seq, tick, &Blocking(frames));
        let rgba = match self.gpu.as_mut() {
            Some(g) => {
                g.comp.render(&plan);
                g.comp.read_output().unwrap_or_default()
            }
            None => cutroom_render::cpu::render(&plan),
        };
        (plan.width, plan.height, rgba)
    }

    /// Ratio of rendered frames whose sources were not ready in time.
    pub fn drop_ratio(&self) -> f64 {
        if self.rendered == 0 {
            0.0
        } else {
            self.dropped as f64 / self.rendered as f64
        }
    }
}
