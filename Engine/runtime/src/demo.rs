use std::error::Error;
use std::path::{Path, PathBuf};

use glam::Vec3;
use raw_window_handle::HasWindowHandle;
use tellurium_assets::{AssetRequest, Assets, AudioClip, Handle, Mesh, ScriptAsset, Texture};
use tellurium_bridge::ColliderKind;
use tellurium_core::{EntityId, EventBatch, Transform, World};
use tellurium_input::ActionMap;
use tellurium_platform::fs;
use tellurium_scripting::ScriptEvent;

use crate::audio::{AudioListener, AudioSource, AudioSystem};
use crate::physics::{Collider, PhysicsSystem, Rigidbody};
use crate::rendering::{MeshRenderer, SceneRenderer};
use crate::Runtime;

pub(crate) fn project_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("demo")
}

pub(crate) fn controls() -> Result<ActionMap, Box<dyn Error>> {
    let config = match fs::read_to_string(project_root().join("project.toml")) {
        Ok(config) => config,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            include_str!("../demo/project.toml").to_owned()
        }
        Err(error) => return Err(error.into()),
    };
    Ok(ActionMap::from_project_toml(&config)?)
}

pub(crate) struct DemoLoading {
    _assets: Assets,
    mesh: AssetRequest<Mesh>,
    texture: AssetRequest<Texture>,
    script: AssetRequest<ScriptAsset>,
    ambient: AssetRequest<AudioClip>,
    impact: AssetRequest<AudioClip>,
}

impl DemoLoading {
    pub(crate) fn request() -> std::io::Result<Self> {
        let assets = Assets::new(project_root())?;
        Ok(Self {
            mesh: assets.request("box.fbx")?,
            texture: assets.request("grid.png")?,
            script: assets.request("physics.luau")?,
            ambient: assets.request("orbit.ogg")?,
            impact: assets.request("impact.wav")?,
            _assets: assets,
        })
    }

    pub(crate) fn poll_install<W: HasWindowHandle>(
        pending: &mut Option<Self>,
        runtime: &mut Runtime,
        renderer: &mut SceneRenderer<W>,
    ) -> Result<Option<DemoScene>, Box<dyn Error>> {
        let Some(loading) = pending else {
            return Ok(None);
        };
        let ready = loading.mesh.poll()?.is_some()
            && loading.texture.poll()?.is_some()
            && loading.script.poll()?.is_some()
            && loading.ambient.poll()?.is_some()
            && loading.impact.poll()?.is_some();
        if ready {
            return pending
                .take()
                .map(|loading| loading.install(runtime, renderer))
                .transpose();
        }
        Ok(None)
    }

    fn install<W: HasWindowHandle>(
        self,
        runtime: &mut Runtime,
        renderer: &mut SceneRenderer<W>,
    ) -> Result<DemoScene, Box<dyn Error>> {
        let mesh = self.mesh.wait()?;
        let texture = self.texture.wait()?;
        let script = self.script.wait()?;
        let ambient = self.ambient.wait()?;
        let impact = self.impact.wait()?;

        log::info!("Demo assets from .telcache: mesh {} vertices/{} indices, texture {}x{}, WAV and Ogg/Vorbis PCM",
            mesh.vertices.len(), mesh.indices.len(), texture.width, texture.height);
        let mesh = renderer.upload_mesh(&mesh)?;
        let texture = renderer.upload_texture("grid.png", &texture)?;
        let floor_material = renderer.material(mesh, texture, [0.28, 0.43, 0.60, 1.0])?;
        let cube_material = renderer.material(mesh, texture, [0.13, 1.0, 0.81, 1.0])?;
        let accent_material = renderer.material(mesh, texture, [1.0, 0.57, 0.22, 1.0])?;
        let trigger_material = renderer.material(mesh, texture, [0.74, 0.44, 0.25, 1.0])?;
        let camera =
            renderer.set_camera(Vec3::new(2.4, 2.0, -5.1), Vec3::new(0.0, 0.14, 0.0), 1.57);

        let cube = {
            let mut world = runtime.world.borrow_mut();
            let floor = spawn_mesh(
                &mut world,
                floor_material,
                [0.0, -0.70, 0.0],
                [4.25, 0.24, 2.50],
            );
            world.add_component(floor, Rigidbody::fixed());
            world.add_component(floor, box_collider(false));

            spawn_mesh(
                &mut world,
                trigger_material,
                [1.22, -0.555, 0.1],
                [0.62, 0.05, 0.64],
            );
            let trigger = world.spawn();
            world.add_component(
                trigger,
                Transform {
                    position: [1.22, -0.28, 0.1],
                    scale: [0.62, 0.55, 0.64],
                    ..Transform::default()
                },
            );
            world.add_component(trigger, Rigidbody::fixed());
            world.add_component(trigger, box_collider(true));

            let cube = spawn_mesh(&mut world, cube_material, [-0.75, 1.05, 0.0], [0.56; 3]);
            world.add_component(cube, Rigidbody::dynamic(1.0));
            world.add_component(cube, box_collider(false));
            world.add_component(
                cube,
                AudioSource {
                    clip: ambient,
                    volume: 0.32,
                    looping: true,
                    spatial: true,
                    transient: false,
                },
            );

            let listener = world.spawn();
            world.add_component(listener, camera);
            world.add_component(listener, AudioListener);
            cube
        };
        runtime
            .scripting
            .attach_asset(cube, "physics.luau", &script)?;
        runtime.physics = Some(PhysicsSystem::new(-9.81)?);
        runtime.audio = Some(AudioSystem::new()?);
        log::info!("Demo controls: WASD or left stick to move, Space or A to jump, left click or X to shoot, Tab toggles UIContext");
        Ok(DemoScene {
            cube,
            cube_material,
            accent_material,
            impact,
            highlight: 0.0,
        })
    }
}

pub(crate) struct DemoScene {
    cube: EntityId,
    cube_material: MeshRenderer,
    accent_material: MeshRenderer,
    impact: Handle<AudioClip>,
    highlight: f64,
}

impl DemoScene {
    pub(crate) fn update(&mut self, world: &mut World, events: &EventBatch, delta: f64) {
        if self.highlight > 0.0 {
            self.highlight -= delta;
            if self.highlight <= 0.0 {
                world.add_component(self.cube, self.cube_material);
            }
        }
        let cube = self.cube;
        for event in events
            .of_type::<ScriptEvent>()
            .filter(|event| event.sender == cube)
        {
            match event.name.as_str() {
                "demo.collision" => {
                    log::info!(
                        "Bullet collision handled by Luau: cube {} with entity {}",
                        self.cube.0,
                        event.payload
                    );
                    self.play_impact(world, 0.7);
                }
                "demo.trigger" => {
                    log::info!("Bullet trigger handled by Luau: entity {}", event.payload);
                    self.flash(world);
                }
                "demo.jump" => log::info!("Luau action Jump"),
                "demo.confirm" => log::info!("Luau action Confirm in UIContext"),
                "demo.shoot" => {
                    self.play_impact(world, 0.49);
                    self.flash(world);
                }
                _ => {}
            }
        }
    }

    fn flash(&mut self, world: &mut World) {
        self.highlight = 0.25;
        world.add_component(self.cube, self.accent_material);
    }

    fn play_impact(&self, world: &mut World, volume: f32) {
        if let Some(&position) = world.get_component::<Transform>(self.cube) {
            let sound = world.spawn();
            world.add_component(sound, position);
            world.add_component(
                sound,
                AudioSource {
                    clip: self.impact.clone(),
                    volume,
                    looping: false,
                    spatial: true,
                    transient: true,
                },
            );
        }
    }
}

fn box_collider(trigger: bool) -> Collider {
    Collider {
        shape: ColliderKind::Box,
        dimensions: [0.5; 3],
        trigger,
    }
}

fn spawn_mesh(
    world: &mut World,
    mesh: MeshRenderer,
    position: [f32; 3],
    scale: [f32; 3],
) -> EntityId {
    let entity = world.spawn();
    world.add_component(
        entity,
        Transform {
            position,
            scale,
            ..Transform::default()
        },
    );
    world.add_component(entity, mesh);
    entity
}
