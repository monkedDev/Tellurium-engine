//! Typed Rust/C++ boundary for graphics and physics.

#[path = "ffi.rs"]
mod bindings;
mod transform_serde;

use std::{error::Error, fmt};

use cxx::UniquePtr;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

use bindings::ffi;
use tellurium_bullet as _;
use tellurium_miniaudio as _;

pub use cxx::{Exception as NativeError, UniquePtr as NativePtr};
pub use ffi::{
    create_audio_device, create_physics_world, raycast, step_physics, sync_physics_bodies,
    take_physics_frame, AudioClipId, AudioDevice, AudioVoiceId, BodyDesc, BodyKind, BodyState,
    ColliderKind, FrameParams, ListenerState, MaterialData, MaterialHandle, MeshHandle,
    PhysicsContact, PhysicsFrame, PhysicsWorld, Ray, RayHit, RenderCommand, RenderFlag,
    TextureHandle, Transform, Vertex, VoiceState, AABB,
};

pub const VISIBLE: u32 = RenderFlag::Visible.repr;

#[derive(Clone, Copy)]
#[repr(u8)]
pub enum TraceStage {
    Frame,
    Input,
    PreUpdate,
    Ecs,
    Physics,
    RenderPreparation,
    Render,
    PostFrame,
    HotReload,
}

pub struct TraceZone {
    #[cfg(feature = "profiling")]
    token: ffi::TraceToken,
    _thread: std::marker::PhantomData<std::rc::Rc<()>>,
}

impl TraceZone {
    pub fn new(_stage: TraceStage) -> Self {
        Self {
            #[cfg(feature = "profiling")]
            token: ffi::trace_begin(_stage as u8),
            _thread: std::marker::PhantomData,
        }
    }
}

impl Drop for TraceZone {
    fn drop(&mut self) {
        #[cfg(feature = "profiling")]
        ffi::trace_end(self.token);
    }
}

pub fn trace_frame() {
    #[cfg(feature = "profiling")]
    ffi::trace_frame();
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            position: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        }
    }
}

#[derive(Debug)]
pub enum RenderError {
    Window(raw_window_handle::HandleError),
    UnsupportedWindow,
    NullDevice,
    Backend(cxx::Exception),
}

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Window(error) => write!(f, "window handle: {error}"),
            Self::UnsupportedWindow => {
                f.write_str("the DirectX 12 backend requires a Win32 window")
            }
            Self::NullDevice => f.write_str("the graphics backend returned no device"),
            Self::Backend(error) => error.fmt(f),
        }
    }
}

impl Error for RenderError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Window(error) => Some(error),
            Self::Backend(error) => Some(error),
            Self::UnsupportedWindow | Self::NullDevice => None,
        }
    }
}

impl From<cxx::Exception> for RenderError {
    fn from(error: cxx::Exception) -> Self {
        Self::Backend(error)
    }
}

/// Owns the C++ device and keeps its window alive through device destruction.
pub struct RenderDevice<W> {
    device: UniquePtr<ffi::RenderDevice>,
    _window: W,
}

impl<W: HasWindowHandle> RenderDevice<W> {
    pub fn new(window: W, width: u32, height: u32) -> Result<Self, RenderError> {
        let handle = window.window_handle().map_err(RenderError::Window)?;
        let RawWindowHandle::Win32(handle) = handle.as_raw() else {
            return Err(RenderError::UnsupportedWindow);
        };
        // SAFETY: the handle comes from the retained window owner. The graphics
        // implementation neither owns the HWND nor receives a Rust pointer.
        let device = unsafe {
            ffi::create_render_device(
                ffi::NativeWindow {
                    hwnd: handle.hwnd.get() as usize,
                },
                width,
                height,
            )?
        };
        if device.is_null() {
            return Err(RenderError::NullDevice);
        }
        Ok(Self {
            device,
            _window: window,
        })
    }

    pub fn adapter_name(&self) -> String {
        self.device.adapter_name()
    }

    pub fn resize(&mut self, width: u32, height: u32) -> Result<(), RenderError> {
        Ok(self.device.pin_mut().resize(width, height)?)
    }

    pub fn create_mesh(
        &mut self,
        vertices: &[Vertex],
        indices: &[u32],
    ) -> Result<MeshHandle, RenderError> {
        Ok(self.device.pin_mut().create_mesh(vertices, indices)?)
    }

    pub fn load_texture_rgba8(
        &mut self,
        width: u32,
        height: u32,
        pixels: &[u8],
        name: &str,
    ) -> Result<TextureHandle, RenderError> {
        Ok(self
            .device
            .pin_mut()
            .load_texture_rgba8(width, height, pixels, name)?)
    }

    pub fn create_material(&mut self, data: MaterialData) -> Result<MaterialHandle, RenderError> {
        Ok(self.device.pin_mut().create_material(data)?)
    }

    pub fn replace_texture_rgba8(
        &mut self,
        handle: TextureHandle,
        width: u32,
        height: u32,
        pixels: &[u8],
        name: &str,
    ) -> Result<(), RenderError> {
        Ok(self
            .device
            .pin_mut()
            .replace_texture_rgba8(handle, width, height, pixels, name)?)
    }

    pub fn reload_mesh_shader(&mut self, source: &str) -> Result<(), RenderError> {
        Ok(self.device.pin_mut().reload_mesh_shader(source)?)
    }

    /// Submit the complete render command slice and present it in one call.
    pub fn render_frame(
        &mut self,
        params: &FrameParams,
        commands: &[RenderCommand],
    ) -> Result<(), RenderError> {
        Ok(self.device.pin_mut().render_frame(params, commands)?)
    }
}
