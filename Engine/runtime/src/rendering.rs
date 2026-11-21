use std::collections::HashMap;

use glam::{Mat4, Quat, Vec3};
use raw_window_handle::HasWindowHandle;
use tellurium_assets::{AssetRevision, Handle, Mesh, Texture};
use tellurium_bridge::{
    FrameParams, MaterialData, MaterialHandle, MeshHandle, RenderCommand, RenderDevice,
    RenderError, TextureHandle, Vertex, VISIBLE,
};
use tellurium_core::{Component, Transform, World};
use tellurium_diagnostics::{TraceStage, TraceZone};

#[derive(Clone, Copy)]
pub(crate) struct MeshRenderer {
    mesh: MeshHandle,
    material: MaterialHandle,
    pub(crate) visible: bool,
}

impl Component for MeshRenderer {}

pub struct SceneRenderer<W> {
    device: RenderDevice<W>,
    commands: Vec<RenderCommand>,
    draw_calls: u32,
    size: (u32, u32),
    view: Mat4,
    half_height: f32,
    clear_color: [f32; 4],
    meshes: HashMap<(String, Option<AssetRevision>), MeshHandle>,
    textures: HashMap<String, (Option<AssetRevision>, TextureHandle)>,
    materials: HashMap<(MeshHandle, TextureHandle, [u32; 4]), MeshRenderer>,
}

impl<W: HasWindowHandle> SceneRenderer<W> {
    pub fn new(window: W, size: (u32, u32)) -> Result<Self, RenderError> {
        let device = RenderDevice::new(window, size.0, size.1)?;
        log::info!("DirectX 12 adapter: {}", device.adapter_name());
        Ok(Self {
            device,
            commands: Vec::new(),
            draw_calls: 0,
            size,
            view: Mat4::from_translation(Vec3::new(0.0, 0.0, 3.0)),
            half_height: 1.35,
            clear_color: [11.0 / 255.0, 14.0 / 255.0, 20.0 / 255.0, 1.0],
            meshes: HashMap::new(),
            textures: HashMap::new(),
            materials: HashMap::new(),
        })
    }

    #[cfg(all(feature = "editor", windows))]
    pub(crate) fn set_clear_color(&mut self, color: [f32; 4]) {
        self.clear_color = color;
    }

    pub(crate) fn set_camera(&mut self, eye: Vec3, target: Vec3, half_height: f32) -> Transform {
        self.view = Mat4::look_at_lh(eye, target, Vec3::Y);
        self.half_height = half_height;
        let (_, rotation, position) = self.view.inverse().to_scale_rotation_translation();
        Transform {
            position: position.to_array(),
            rotation: rotation.to_array(),
            ..Transform::default()
        }
    }

    pub(crate) fn set_camera_pose(&mut self, eye: Vec3, orientation: Quat, half_height: f32) {
        self.view = Mat4::from_rotation_translation(orientation, eye).inverse();
        self.half_height = half_height;
    }

    pub(crate) fn scene_mesh(
        &mut self,
        path: &str,
        asset: Option<&Handle<Mesh>>,
    ) -> Result<MeshHandle, RenderError> {
        let key = (path.to_owned(), asset.map(Handle::revision));
        if let Some(&handle) = self.meshes.get(&key) {
            return Ok(handle);
        }
        let handle = match asset {
            Some(mesh) => self.upload_mesh(mesh)?,
            None => self.upload_mesh(&crate::scene::primitive(path == ":quad"))?,
        };
        self.meshes.insert(key, handle);
        Ok(handle)
    }

    pub(crate) fn scene_texture(
        &mut self,
        path: &str,
        asset: Option<&Handle<Texture>>,
    ) -> Result<TextureHandle, RenderError> {
        let key = path.replace('\\', "/").to_lowercase();
        let revision = asset.map(Handle::revision);
        if let Some(&(previous, handle)) = self.textures.get(&key) {
            if previous != revision {
                if let Some(asset) = asset {
                    self.device.replace_texture_rgba8(
                        handle,
                        asset.width,
                        asset.height,
                        &asset.rgba8,
                        path,
                    )?;
                    self.textures.insert(key, (revision, handle));
                }
            }
            return Ok(handle);
        }
        let handle = match asset {
            Some(texture) => self.upload_texture(path, texture)?,
            None => self.device.load_texture_rgba8(1, 1, &[255; 4], "white")?,
        };
        self.textures.insert(key, (revision, handle));
        Ok(handle)
    }

    #[cfg(all(feature = "editor", windows))]
    pub(crate) fn reload_texture(
        &mut self,
        path: &str,
        asset: &Handle<Texture>,
    ) -> Result<bool, RenderError> {
        let key = path.replace('\\', "/").to_lowercase();
        if !matches!(self.textures.get(&key), Some((revision, _)) if *revision != Some(asset.revision()))
        {
            return Ok(false);
        }
        self.scene_texture(path, Some(asset))?;
        Ok(true)
    }

    #[cfg(all(feature = "editor", windows))]
    pub(crate) fn reload_shader(&mut self, source: &str) -> Result<(), RenderError> {
        self.device.reload_mesh_shader(source)
    }

    pub(crate) fn scene_material(
        &mut self,
        mesh: MeshHandle,
        texture: TextureHandle,
        tint: [f32; 4],
    ) -> Result<MeshRenderer, RenderError> {
        let key = (mesh, texture, tint.map(f32::to_bits));
        if let Some(&render) = self.materials.get(&key) {
            return Ok(render);
        }
        let render = self.material(mesh, texture, tint)?;
        self.materials.insert(key, render);
        Ok(render)
    }

    pub(crate) fn upload_mesh(&mut self, mesh: &Mesh) -> Result<MeshHandle, RenderError> {
        let vertices: Vec<_> = mesh
            .vertices
            .iter()
            .map(|vertex| Vertex {
                position: vertex.position,
                uv: vertex.uv,
                color: vertex.color,
            })
            .collect();
        self.device.create_mesh(&vertices, &mesh.indices)
    }

    pub(crate) fn upload_texture(
        &mut self,
        name: &str,
        texture: &Texture,
    ) -> Result<TextureHandle, RenderError> {
        self.device
            .load_texture_rgba8(texture.width, texture.height, &texture.rgba8, name)
    }

    pub(crate) fn material(
        &mut self,
        mesh: MeshHandle,
        texture: TextureHandle,
        tint: [f32; 4],
    ) -> Result<MeshRenderer, RenderError> {
        let material = self
            .device
            .create_material(MaterialData { texture, tint })?;
        Ok(MeshRenderer {
            mesh,
            material,
            visible: true,
        })
    }

    pub(crate) fn demo_triangle(&mut self) -> Result<MeshRenderer, RenderError> {
        let vertices = [
            Vertex {
                position: [0.0, 0.62, 0.0],
                color: [0.05, 1.0, 0.82, 1.0],
                uv: [0.5, 0.0],
            },
            Vertex {
                position: [0.68, -0.56, 0.0],
                color: [0.18, 0.56, 1.0, 1.0],
                uv: [1.0, 1.0],
            },
            Vertex {
                position: [-0.68, -0.56, 0.0],
                color: [0.95, 0.26, 0.52, 1.0],
                uv: [0.0, 1.0],
            },
        ];
        let mesh = self.device.create_mesh(&vertices, &[0, 1, 2])?;
        let texture = self.device.load_texture_rgba8(
            2,
            2,
            &[
                255, 255, 255, 255, 232, 245, 255, 255, 253, 235, 255, 255, 225, 248, 244, 255,
            ],
            "demo triangle",
        )?;
        self.material(mesh, texture, [1.0; 4])
    }

    pub fn render(
        &mut self,
        world: &World,
        size: (u32, u32),
        pre_present: impl FnOnce(),
    ) -> Result<(), RenderError> {
        self.draw_calls = 0;
        let preparation = TraceZone::new(TraceStage::RenderPreparation);
        let (width, height) = size;
        if width == 0 || height == 0 {
            return Ok(());
        }
        if self.size != size {
            self.device.resize(width, height)?;
            self.size = size;
        }

        // Render preparation: collect one linear command list from ECS pools.
        self.commands.clear();
        for (_, transform, mesh) in world.query2::<Transform, MeshRenderer>() {
            let matrix = model_matrix(transform);
            self.commands.push(RenderCommand {
                mesh: mesh.mesh,
                material: mesh.material,
                transform: matrix.to_cols_array(),
                flags: if mesh.visible { VISIBLE } else { 0 },
            });
        }

        let half_height = self.half_height;
        let half_width = half_height * width as f32 / height as f32;
        let projection = Mat4::orthographic_lh(
            -half_width,
            half_width,
            -half_height,
            half_height,
            0.1,
            10.0,
        );
        let params = FrameParams {
            view_projection: (projection * self.view).to_cols_array(),
            clear_color: self.clear_color,
        };
        drop(preparation);

        // One call covers command recording, submission and swap-chain present.
        let _render = TraceZone::new(TraceStage::Render);
        pre_present();
        self.device.render_frame(&params, &self.commands)?;
        self.draw_calls = self
            .commands
            .iter()
            .filter(|command| command.flags & VISIBLE != 0)
            .count() as u32;
        Ok(())
    }

    pub fn draw_calls(&self) -> u32 {
        self.draw_calls
    }
}

fn model_matrix(transform: &Transform) -> Mat4 {
    Mat4::from_scale_rotation_translation(
        Vec3::from_array(transform.scale),
        Quat::from_array(transform.rotation),
        Vec3::from_array(transform.position),
    )
}
