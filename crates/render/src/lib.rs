//! Cutroom rendering.
//!
//! Rendering is a pure function of `(doc_snapshot, tick)`:
//!
//! 1. [`plan::build_plan`] turns a [`cutroom_core::DocSnapshot`] and a tick
//!    into a [`plan::RenderPlan`] — an ordered list of layers with resolved
//!    source frames and pixel-space affine transforms. This step is shared by
//!    preview and export.
//! 2. A rasterizer executes the plan: [`gpu::Compositor`] (wgpu/WGSL) for
//!    preview and export, or [`cpu::render`] — a bit-for-bit deterministic
//!    reference used by tests, convergence checks and GPU-less machines.

pub mod color;
pub mod cpu;
pub mod gpu;
pub mod plan;

pub use plan::{FrameProvider, Layer, LayerSource, RenderPlan, build_plan};

/// Stable hash of a rendered RGBA frame (convergence tests).
pub fn frame_hash(rgba: &[u8]) -> blake3::Hash {
    blake3::hash(rgba)
}

#[cfg(test)]
mod shader_tests {
    /// Validates every WGSL shader with naga, so shader errors are caught
    /// even on machines without a GPU adapter.
    #[test]
    fn shaders_validate() {
        for (name, src) in [
            ("composite", include_str!("shaders/composite.wgsl")),
            ("yuv", include_str!("shaders/yuv.wgsl")),
        ] {
            let module = naga::front::wgsl::parse_str(src)
                .unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(src)));
            naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::empty(),
            )
            .validate(&module)
            .unwrap_or_else(|e| panic!("{name}: {e:?}"));
        }
    }
}
