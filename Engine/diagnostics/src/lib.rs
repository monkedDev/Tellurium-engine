//! Frame instruments shared by the runtime and the editor console.

pub use tellurium_bridge::{trace_frame, TraceStage, TraceZone};

#[derive(Clone, Copy, Default)]
pub struct FrameMetrics {
    pub fps: f64,
    pub frame_ms: f64,
    pub entities: usize,
    pub luau_bytes: usize,
    pub draw_calls: u32,
    pub physics_ms: f64,
    pub ecs_ms: f64,
}
