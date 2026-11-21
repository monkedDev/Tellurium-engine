//! Install authored scenes in the runtime and capture their live ECS state.

use std::collections::HashMap;
use std::error::Error;
use std::io;
use std::path::Path;

use glam::{Quat, Vec3};
use raw_window_handle::HasWindowHandle;
use tellurium_assets::package::{Kind, Package};
use tellurium_assets::{Asset, Assets, AudioClip, Handle, Mesh, MeshVertex, ScriptAsset, Texture};
use tellurium_bridge::{BodyKind, ColliderKind};
use tellurium_core::{Component, Transform, World};
use tellurium_input::ActionMap;
use tellurium_scripting::{LuauConfig, ScriptComponent};
use tellurium_serialization::{
    AudioSourceData, BodyType, CameraData, ColliderData, ColliderShape, ComponentRegistry,
    GraphicsBackend, Hierarchy, MeshRendererData, Node, NodeKind, PackedScene, Project,
    RigidbodyData, SceneNode, ScriptData, WorldSnapshot,
};

use crate::audio::{AudioListener, AudioSource, AudioSystem};
use crate::physics::{Collider, PhysicsSystem, Rigidbody};
use crate::rendering::{MeshRenderer, SceneRenderer};
use crate::Runtime;

type SceneResult<T> = Result<T, Box<dyn Error>>;

pub(crate) struct LiveScene {
    pub(crate) hierarchy: Hierarchy,
}

pub struct PreparedScene {
    scene: PackedScene,
    meshes: HashMap<String, Handle<Mesh>>,
    textures: HashMap<String, Handle<Texture>>,
    scripts: HashMap<String, Handle<ScriptAsset>>,
    clips: HashMap<String, Handle<AudioClip>>,
}

impl PreparedScene {
    /// Import on the asset worker and retain the decoded cache handles for installation.
    pub fn load(project: &Project, scene: PackedScene) -> SceneResult<Self> {
        let assets = Assets::new(project.root())?;
        Self::prepare(project, scene, &SceneAssets::Source(&assets))
    }

    pub(crate) fn load_packaged(
        project: &Project,
        scene: PackedScene,
        package: &Package,
    ) -> SceneResult<Self> {
        Self::prepare(project, scene, &SceneAssets::Package(package))
    }

    fn prepare(
        project: &Project,
        scene: PackedScene,
        assets: &SceneAssets<'_>,
    ) -> SceneResult<Self> {
        scene.order()?;
        if project.config.graphics_backend != GraphicsBackend::Dx12 {
            return Err(
                io::Error::other("The runtime currently supports the DirectX 12 backend").into(),
            );
        }
        let mut prepared = Self {
            scene,
            meshes: HashMap::new(),
            textures: HashMap::new(),
            scripts: HashMap::new(),
            clips: HashMap::new(),
        };
        for node in &prepared.scene.nodes {
            if let Some(mesh) = &node.mesh_renderer {
                if let Some(path) = &mesh.mesh {
                    load_asset(path, &mut prepared.meshes, || {
                        assets.asset(Kind::Mesh, path)
                    })?;
                }
                if let Some(path) = &mesh.texture {
                    load_asset(path, &mut prepared.textures, || {
                        assets.asset(Kind::Texture, path)
                    })?;
                }
            }
            if let Some(script) = &node.script {
                load_asset(&script.path, &mut prepared.scripts, || {
                    assets.script(&script.path)
                })?;
            }
            if let Some(sound) = &node.audio_source {
                load_asset(&sound.clip, &mut prepared.clips, || {
                    assets.asset(Kind::Audio, &sound.clip)
                })?;
            }
        }
        Ok(prepared)
    }

    pub fn install<W: HasWindowHandle>(
        self,
        project: &Project,
        renderer: &mut SceneRenderer<W>,
        registry: &ComponentRegistry,
    ) -> SceneResult<Runtime> {
        self.install_with(project, registry, |node, data, world| {
            let mesh_key = data
                .mesh
                .as_deref()
                .unwrap_or(if node.kind == NodeKind::Sprite {
                    ":quad"
                } else {
                    ":cube"
                });
            let mesh =
                renderer.scene_mesh(mesh_key, data.mesh.as_ref().map(|path| &self.meshes[path]))?;
            let texture_key = data.texture.as_deref().unwrap_or(":white");
            let texture = renderer.scene_texture(
                texture_key,
                data.texture.as_ref().map(|path| &self.textures[path]),
            )?;
            let mut component = renderer.scene_material(mesh, texture, data.tint)?;
            component.visible = data.visible;
            world.add_component(node.id, component);
            Ok(())
        })
    }

    pub fn install_headless(
        &self,
        project: &Project,
        registry: &ComponentRegistry,
    ) -> SceneResult<Runtime> {
        self.install_with(project, registry, |_, _, _| Ok(()))
    }

    fn install_with(
        &self,
        project: &Project,
        registry: &ComponentRegistry,
        mut draw: impl FnMut(&Node, &MeshRendererData, &mut World) -> SceneResult<()>,
    ) -> SceneResult<Runtime> {
        let mut runtime = Runtime::new(LuauConfig::default())?;
        runtime.input = ActionMap::from_project_toml(&project.input_config())?;
        let result = (|| {
            let mut world = runtime.world.borrow_mut();
            let hierarchy = Hierarchy::install(&self.scene, &mut world)?;
            for node in &self.scene.nodes {
                let id = node.id;
                world.add_component(
                    id,
                    NodeData {
                        kind: node.kind,
                        script: node.script.as_ref().map(|data| data.path.clone()),
                        audio: node.audio_source.as_ref().map(|data| data.clip.clone()),
                    },
                );
                if let Some(data) = &node.mesh_renderer {
                    draw(node, data, &mut world)?;
                    world.add_component(id, data.clone());
                }
                if let Some(data) = &node.camera {
                    world.add_component(id, data.clone());
                }
                if let Some(data) = &node.rigidbody {
                    world.add_component(
                        id,
                        Rigidbody {
                            kind: match data.kind {
                                BodyType::Static => BodyKind::Static,
                                BodyType::Dynamic => BodyKind::Dynamic,
                                BodyType::Kinematic => BodyKind::Kinematic,
                            },
                            mass: data.mass,
                            linear_velocity: data.linear_velocity,
                        },
                    );
                }
                if let Some(data) = &node.collider {
                    world.add_component(
                        id,
                        Collider {
                            shape: match data.shape {
                                ColliderShape::Box => ColliderKind::Box,
                                ColliderShape::Sphere => ColliderKind::Sphere,
                            },
                            dimensions: data.dimensions,
                            trigger: data.trigger,
                        },
                    );
                }
                if let Some(data) = &node.audio_source {
                    world.add_component(
                        id,
                        AudioSource {
                            clip: self.clips[&data.clip].clone(),
                            volume: data.volume,
                            looping: data.looping,
                            spatial: data.spatial,
                            transient: false,
                        },
                    );
                }
                if node.audio_listener {
                    world.add_component(id, AudioListener);
                }
                registry.apply(&mut world, id, &node.custom)?;
            }
            let has_audio = world.query::<AudioSource>().next().is_some();
            let has_physics = world.query2::<Rigidbody, Collider>().next().is_some();
            drop(world);
            runtime.scene = Some(LiveScene { hierarchy });
            for node in &self.scene.nodes {
                if let Some(script) = &node.script {
                    runtime.scripting.attach_handle(
                        node.id,
                        &script.path,
                        &self.scripts[&script.path],
                    )?;
                }
            }
            if self.scene.nodes.iter().any(|node| {
                node.script
                    .as_ref()
                    .is_some_and(|script| script.state.is_some())
            }) {
                // A restored script's top-level chunk can configure bindings. The saved ECS values win.
                let mut world = runtime.world.borrow_mut();
                let mut saved_poses = World::new();
                Hierarchy::install(&self.scene, &mut saved_poses)?;
                for (id, pose) in saved_poses.query::<Transform>() {
                    world.add_component(id, *pose);
                }
            }
            for node in &self.scene.nodes {
                if let Some(state) = node
                    .script
                    .as_ref()
                    .and_then(|script| script.state.as_ref())
                {
                    runtime.scripting.restore(node.id, state)?;
                }
            }
            runtime.synchronize_hierarchy();
            if has_physics {
                runtime.physics = Some(PhysicsSystem::new(-9.81)?);
            }
            if has_audio {
                runtime.audio = Some(AudioSystem::new()?);
            }
            Ok(())
        })();
        if let Err(error) = result {
            runtime.shutdown();
            return Err(error);
        }
        Ok(runtime)
    }
}

struct NodeData {
    kind: NodeKind,
    script: Option<String>,
    audio: Option<String>,
}

impl Component for NodeData {}

impl Runtime {
    pub fn restore_clock(&mut self, snapshot: &WorldSnapshot) {
        self.fixed_accumulator = snapshot.fixed_accumulator;
    }

    pub fn snapshot(
        &mut self,
        elapsed: f64,
        registry: &ComponentRegistry,
    ) -> SceneResult<WorldSnapshot> {
        let script_entities: Vec<_> = self
            .world
            .borrow()
            .query::<ScriptComponent>()
            .map(|(id, _)| id)
            .collect();
        let mut script_states = HashMap::new();
        for id in script_entities {
            if let Some(state) = self.scripting.snapshot(id)? {
                script_states.insert(id, state);
            }
        }
        self.finish_physics()?;
        let hierarchy = &self
            .scene
            .as_ref()
            .ok_or_else(|| io::Error::other("No loaded scene"))?
            .hierarchy;
        let world = self.world.borrow();
        let mut ids: Vec<_> = world.entities().collect();
        ids.sort_unstable();
        let mut scene = PackedScene {
            last_entity_id: world.last_entity_id(),
            ..PackedScene::default()
        };
        for id in ids {
            let node = world.get_component::<SceneNode>(id);
            let data = world.get_component::<NodeData>(id);
            let parent = node
                .and_then(|node| node.parent)
                .filter(|parent| world.has_component::<Transform>(*parent));
            let pose = world
                .get_component::<Transform>(id)
                .copied()
                .unwrap_or_default();
            let transform = if parent.is_some() {
                hierarchy.local_transform(id).unwrap_or(pose)
            } else {
                pose
            };
            let script = if world.has_component::<ScriptComponent>(id) {
                let path = data.and_then(|data| data.script.clone()).ok_or_else(|| {
                    io::Error::other("Attached script has no authored asset path")
                })?;
                Some(ScriptData {
                    path,
                    state: script_states.remove(&id),
                })
            } else {
                None
            };
            let audio_source = world
                .get_component::<AudioSource>(id)
                .map(|source| {
                    let clip = data.and_then(|data| data.audio.clone()).ok_or_else(|| {
                        io::Error::other("Audio source has no authored asset path")
                    })?;
                    Ok::<_, io::Error>(AudioSourceData {
                        clip,
                        volume: source.volume,
                        looping: source.looping,
                        spatial: source.spatial,
                    })
                })
                .transpose()?;
            let mut mesh_renderer = world.get_component::<MeshRendererData>(id).cloned();
            if let Some((data, native)) = mesh_renderer
                .as_mut()
                .zip(world.get_component::<MeshRenderer>(id))
            {
                data.visible = native.visible;
            }
            scene.nodes.push(Node {
                id,
                parent,
                transform,
                script,
                audio_source,
                mesh_renderer,
                name: node.map_or_else(|| format!("Entity {}", id.0), |node| node.name.clone()),
                kind: data.map_or(NodeKind::Node3D, |data| data.kind),
                camera: world.get_component::<CameraData>(id).cloned(),
                audio_listener: world.has_component::<AudioListener>(id),
                rigidbody: world
                    .get_component::<Rigidbody>(id)
                    .map(|body| RigidbodyData {
                        kind: match body.kind {
                            BodyKind::Dynamic => BodyType::Dynamic,
                            BodyKind::Kinematic => BodyType::Kinematic,
                            _ => BodyType::Static,
                        },
                        mass: body.mass,
                        linear_velocity: body.linear_velocity,
                    }),
                collider: world
                    .get_component::<Collider>(id)
                    .map(|collider| ColliderData {
                        shape: if collider.shape == ColliderKind::Sphere {
                            ColliderShape::Sphere
                        } else {
                            ColliderShape::Box
                        },
                        dimensions: collider.dimensions,
                        trigger: collider.trigger,
                    }),
                custom: registry.capture(&world, id)?,
            });
        }
        Ok(WorldSnapshot {
            elapsed,
            fixed_accumulator: self.fixed_accumulator,
            scene,
        })
    }
}

enum SceneAssets<'a> {
    Source(&'a Assets),
    Package(&'a Package),
}

impl SceneAssets<'_> {
    fn asset<T: Asset>(&self, kind: Kind, path: &str) -> io::Result<Handle<T>> {
        match self {
            Self::Source(assets) => assets.request(path)?.wait(),
            Self::Package(package) => package.asset(kind, path),
        }
    }

    fn script(&self, path: &str) -> io::Result<Handle<ScriptAsset>> {
        match self {
            Self::Source(assets) => assets.request(path)?.wait(),
            Self::Package(package) => package.script(path),
        }
    }
}

fn load_asset<T: Asset>(
    path: &str,
    loaded: &mut HashMap<String, Handle<T>>,
    load: impl FnOnce() -> io::Result<Handle<T>>,
) -> SceneResult<()> {
    if !loaded.contains_key(path) {
        let asset = load().map_err(|error| io::Error::other(format!("{path}: {error}")))?;
        loaded.insert(path.to_owned(), asset);
    }
    Ok(())
}

pub(crate) fn camera<W: HasWindowHandle>(world: &World, renderer: &mut SceneRenderer<W>) {
    if let Some((_, transform, camera)) = world
        .query2::<Transform, CameraData>()
        .filter(|(_, _, camera)| camera.active)
        .min_by_key(|(id, _, _)| *id)
    {
        let eye = Vec3::from_array(transform.position);
        let rotation = Quat::from_array(transform.rotation);
        renderer.set_camera_pose(eye, rotation, camera.half_height);
    } else {
        renderer.set_camera(Vec3::new(4.0, 3.0, -6.0), Vec3::ZERO, 2.5);
    }
}

pub(crate) fn primitive(quad: bool) -> Mesh {
    let corners = [
        [-0.5, -0.5, -0.5],
        [0.5, -0.5, -0.5],
        [0.5, 0.5, -0.5],
        [-0.5, 0.5, -0.5],
        [-0.5, -0.5, 0.5],
        [0.5, -0.5, 0.5],
        [0.5, 0.5, 0.5],
        [-0.5, 0.5, 0.5],
    ];
    let faces = [
        [0, 1, 2, 3],
        [5, 4, 7, 6],
        [4, 0, 3, 7],
        [1, 5, 6, 2],
        [3, 2, 6, 7],
        [4, 5, 1, 0],
    ];
    let uv = [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];
    let mut mesh = Mesh {
        vertices: Vec::new(),
        indices: Vec::new(),
    };
    for (face, shade) in faces
        .into_iter()
        .zip([0.86, 0.62, 0.68, 0.76, 1.0, 0.54])
        .take(if quad { 1 } else { 6 })
    {
        let base = mesh.vertices.len() as u32;
        for (corner, uv) in face.into_iter().zip(uv) {
            let mut position = corners[corner];
            if quad {
                position[2] = 0.0;
            }
            mesh.vertices.push(MeshVertex {
                position,
                uv,
                color: [shade, shade, shade, 1.0],
            });
        }
        mesh.indices
            .extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    mesh
}

pub fn load_project_scene(project: &Project, scene: Option<&Path>) -> SceneResult<PackedScene> {
    let path = match scene {
        Some(path) => path.to_path_buf(),
        None => project.path(&project.config.main_scene)?,
    };
    Ok(PackedScene::load(&path)?)
}
