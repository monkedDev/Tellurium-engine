#[cxx::bridge(namespace = "tellurium")]
pub(crate) mod ffi {
    #[derive(Debug, Clone, Copy)]
    struct Vertex {
        position: [f32; 3],
        color: [f32; 4],
        uv: [f32; 2],
    }

    #[derive(Debug, Clone, Copy, PartialEq)]
    struct Transform {
        position: [f32; 3],
        /// Quaternion in x, y, z, w order.
        rotation: [f32; 4],
        scale: [f32; 3],
    }

    #[derive(Debug, Clone, Copy)]
    struct AABB {
        min: [f32; 3],
        max: [f32; 3],
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    struct TextureHandle {
        id: u32,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    struct MeshHandle {
        id: u32,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    struct MaterialHandle {
        id: u32,
    }

    #[derive(Debug, Clone, Copy)]
    struct MaterialData {
        texture: TextureHandle,
        tint: [f32; 4],
    }

    #[repr(u32)]
    enum RenderFlag {
        Visible = 1,
    }

    #[derive(Debug, Clone, Copy)]
    struct RenderCommand {
        mesh: MeshHandle,
        material: MaterialHandle,
        /// Column-major model matrix, matching HLSL's column_major layout.
        transform: [f32; 16],
        flags: u32,
    }

    #[derive(Debug, Clone, Copy)]
    struct FrameParams {
        /// Column-major world-to-clip matrix with DirectX depth in [0, 1].
        view_projection: [f32; 16],
        clear_color: [f32; 4],
    }

    #[derive(Debug, Clone, Copy)]
    struct Ray {
        origin: [f32; 3],
        /// Unit direction in world space.
        direction: [f32; 3],
        max_distance: f32,
    }

    #[derive(Debug, Clone, Copy)]
    struct RayHit {
        hit: bool,
        entity: u64,
        distance: f32,
        normal: [f32; 3],
    }

    #[repr(u8)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum BodyKind {
        Static = 0,
        Dynamic = 1,
        Kinematic = 2,
    }

    #[repr(u8)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum ColliderKind {
        Box = 0,
        Sphere = 1,
    }

    #[derive(Debug, Clone, Copy)]
    struct BodyDesc {
        entity: u64,
        pose: Transform,
        linear_velocity: [f32; 3],
        kind: BodyKind,
        collider: ColliderKind,
        /// Box half extents, or radius in x for a sphere, before applying scale.
        dimensions: [f32; 3],
        mass: f32,
        trigger: bool,
        override_pose: bool,
        override_velocity: bool,
    }

    #[derive(Debug, Clone, Copy)]
    struct BodyState {
        entity: u64,
        position: [f32; 3],
        rotation: [f32; 4],
        linear_velocity: [f32; 3],
    }

    #[derive(Debug, Clone, Copy)]
    struct PhysicsContact {
        first: u64,
        second: u64,
        trigger: bool,
    }

    struct PhysicsFrame {
        bodies: Vec<BodyState>,
        contacts: Vec<PhysicsContact>,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    struct AudioClipId {
        id: u32,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    struct AudioVoiceId {
        id: u32,
    }

    #[derive(Debug, Clone, Copy)]
    struct VoiceState {
        position: [f32; 3],
        velocity: [f32; 3],
        volume: f32,
        min_distance: f32,
        max_distance: f32,
        rolloff: f32,
        doppler: f32,
        looping: bool,
        spatial: bool,
    }

    #[derive(Debug, Clone, Copy)]
    struct ListenerState {
        position: [f32; 3],
        velocity: [f32; 3],
        forward: [f32; 3],
        up: [f32; 3],
    }

    struct NativeWindow {
        hwnd: usize,
    }

    #[cfg(feature = "profiling")]
    #[derive(Clone, Copy)]
    struct TraceToken {
        id: u32,
        active: i32,
    }

    unsafe extern "C++" {
        include!("tellurium/graphics.hpp");
        include!("tellurium/physics.hpp");
        include!("tellurium/audio.hpp");
        include!("tellurium/profile.hpp");

        #[cfg(feature = "profiling")]
        fn trace_begin(stage: u8) -> TraceToken;
        #[cfg(feature = "profiling")]
        fn trace_end(token: TraceToken);
        #[cfg(feature = "profiling")]
        fn trace_frame();

        type RenderDevice;
        // Caller must retain the native window until the device is dropped.
        unsafe fn create_render_device(
            window: NativeWindow,
            width: u32,
            height: u32,
        ) -> Result<UniquePtr<RenderDevice>>;
        fn adapter_name(self: &RenderDevice) -> String;
        fn resize(self: Pin<&mut RenderDevice>, width: u32, height: u32) -> Result<()>;
        fn create_mesh(
            self: Pin<&mut RenderDevice>,
            vertices: &[Vertex],
            indices: &[u32],
        ) -> Result<MeshHandle>;
        fn load_texture_rgba8(
            self: Pin<&mut RenderDevice>,
            width: u32,
            height: u32,
            pixels: &[u8],
            name: &str,
        ) -> Result<TextureHandle>;
        fn create_material(
            self: Pin<&mut RenderDevice>,
            material: MaterialData,
        ) -> Result<MaterialHandle>;
        fn replace_texture_rgba8(
            self: Pin<&mut RenderDevice>,
            handle: TextureHandle,
            width: u32,
            height: u32,
            pixels: &[u8],
            name: &str,
        ) -> Result<()>;
        fn reload_mesh_shader(self: Pin<&mut RenderDevice>, source: &str) -> Result<()>;
        fn render_frame(
            self: Pin<&mut RenderDevice>,
            params: &FrameParams,
            commands: &[RenderCommand],
        ) -> Result<()>;

        type PhysicsWorld;
        fn create_physics_world(gravity_y: f32) -> Result<UniquePtr<PhysicsWorld>>;
        fn sync_physics_bodies(world: Pin<&mut PhysicsWorld>, bodies: &[BodyDesc]) -> Result<()>;
        fn step_physics(world: Pin<&mut PhysicsWorld>, delta: f32) -> Result<()>;
        fn take_physics_frame(world: Pin<&mut PhysicsWorld>) -> Result<PhysicsFrame>;
        fn raycast(world: &PhysicsWorld, ray: &Ray) -> Result<RayHit>;

        type AudioDevice;
        fn create_audio_device(offline: bool) -> Result<UniquePtr<AudioDevice>>;
        fn output_name(self: &AudioDevice) -> String;
        fn upload_audio(
            self: Pin<&mut AudioDevice>,
            sample_rate: u32,
            channels: u32,
            samples: &[f32],
        ) -> Result<AudioClipId>;
        fn release_audio_clip(self: Pin<&mut AudioDevice>, handle: AudioClipId);
        fn play_audio(
            self: Pin<&mut AudioDevice>,
            handle: AudioClipId,
            state: &VoiceState,
        ) -> Result<AudioVoiceId>;
        fn update_audio_voice(
            self: Pin<&mut AudioDevice>,
            handle: AudioVoiceId,
            state: &VoiceState,
        ) -> Result<()>;
        fn release_audio_voice(self: Pin<&mut AudioDevice>, handle: AudioVoiceId);
        fn audio_voice_finished(self: Pin<&mut AudioDevice>, handle: AudioVoiceId) -> Result<bool>;
        fn update_audio_listener(self: Pin<&mut AudioDevice>, state: &ListenerState) -> Result<()>;
        fn set_audio_paused(self: Pin<&mut AudioDevice>, paused: bool) -> Result<()>;
        fn mix_offline(self: Pin<&mut AudioDevice>, stereo: &mut [f32]) -> Result<()>;
    }
}
