//! Entry point and frame scheduling for the engine runtime.

mod audio;
mod demo;
mod launch;
mod physics;
mod rendering;
pub mod scene;

#[cfg(all(feature = "editor", windows))]
mod editor_host;
#[cfg(all(feature = "editor", windows))]
mod editor_watch;

pub use launch::run;
pub use rendering::SceneRenderer;

#[cfg(test)]
mod tests;

use std::cell::RefCell;
use std::error::Error;
use std::rc::Rc;
use std::time::Instant;

use tellurium_core::{EntityId, EventBatch, Transform, World};
use tellurium_diagnostics::{trace_frame, FrameMetrics, TraceStage, TraceZone};
use tellurium_input::{ActionMap, ActionValue};
use tellurium_platform::RawInputEvent;
use tellurium_scripting::{LuauConfig, ScriptError, Scripting};

use audio::AudioSystem;
use demo::DemoScene;
use physics::PhysicsSystem;

const FIXED_DELTA: f64 = 1.0 / 60.0;
const MAX_FIXED_STEPS: f64 = 4.0;
#[cfg(test)]
const DEMO_SOURCE: &str = include_str!("demo.luau");

pub struct Runtime {
    metrics: FrameMetrics,
    world: Rc<RefCell<World>>,
    scripting: Scripting,
    previous_events: EventBatch,
    fixed_accumulator: f64,
    input: ActionMap,
    physics: Option<PhysicsSystem>,
    audio: Option<AudioSystem>,
    demo: Option<DemoScene>,
    scene: Option<scene::LiveScene>,
}

impl Runtime {
    pub fn new(config: LuauConfig) -> Result<Self, ScriptError> {
        let world = Rc::new(RefCell::new(World::new()));
        let scripting = Scripting::new(Rc::clone(&world), config)?;
        Ok(Self {
            metrics: FrameMetrics::default(),
            world,
            scripting,
            previous_events: EventBatch::default(),
            fixed_accumulator: 0.0,
            input: ActionMap::default(),
            physics: None,
            audio: None,
            demo: None,
            scene: None,
        })
    }

    pub fn spawn(&mut self, name: &str, source: &str) -> Result<EntityId, ScriptError> {
        let entity = {
            let mut world = self.world.borrow_mut();
            let entity = world.spawn();
            world.add_component(entity, Transform::default());
            entity
        };
        if let Err(error) = self.scripting.attach(entity, name, source) {
            self.world.borrow_mut().despawn(entity);
            return Err(error);
        }
        Ok(entity)
    }

    pub fn frame<R>(
        &mut self,
        input: Vec<RawInputEvent>,
        delta: f64,
        elapsed: f64,
        render: impl FnOnce(&World) -> R,
    ) -> Result<R, Box<dyn Error>> {
        let frame_start = Instant::now();
        let frame_zone = TraceZone::new(TraceStage::Frame);
        let input_zone = TraceZone::new(TraceStage::Input);
        let actions = self.input.update(&input);
        {
            let mut world = self.world.borrow_mut();
            world.set_frame_input(input);
            for action in &actions {
                world.event_bus().publish_immediate(action);
                if action.name == "ToggleUI" && action.value == ActionValue::Digital(true) {
                    if let Some(active) = self.input.toggle("UIContext") {
                        log::info!("UIContext {}", if active { "active" } else { "inactive" });
                    }
                }
            }
            if let Some(demo) = &mut self.demo {
                demo.update(&mut world, &self.previous_events, delta);
            }
        }
        drop(input_zone);
        let pre_update = TraceZone::new(TraceStage::PreUpdate);
        self.scripting
            .begin_frame(delta, elapsed, &self.previous_events);
        self.scripting.pre_update();
        self.scripting.dispatch_actions(&actions);
        drop(pre_update);

        // ECS update and fixed-step logic. Cap catch-up after a stall.
        let ecs = TraceZone::new(TraceStage::Ecs);
        let ecs_start = Instant::now();
        self.world.borrow_mut().run_systems();
        self.scripting.update(delta);
        self.fixed_accumulator =
            (self.fixed_accumulator + delta).min(FIXED_DELTA * MAX_FIXED_STEPS);
        let mut steps = 0;
        while self.fixed_accumulator >= FIXED_DELTA {
            self.scripting.fixed_update(FIXED_DELTA);
            self.fixed_accumulator -= FIXED_DELTA;
            steps += 1;
        }

        self.synchronize_hierarchy();
        let ecs_ms = ecs_start.elapsed().as_secs_f64() * 1000.0;
        drop(ecs);

        // Synchronize the latest completed Bullet snapshot without blocking the frame.
        let physics_zone = TraceZone::new(TraceStage::Physics);
        let mut physics_ms = 0.0;
        if let Some(physics) = &mut self.physics {
            physics.synchronize(&mut self.world.borrow_mut(), steps, FIXED_DELTA as f32)?;
            physics_ms = physics.last_step_ms();
        }
        drop(physics_zone);
        self.scripting.deliver_contacts(&self.previous_events);
        self.synchronize_hierarchy();
        if let Some(audio) = &mut self.audio {
            audio.update(&mut self.world.borrow_mut(), delta)?;
        }

        let render_result = render(&self.world.borrow());

        // Post-frame publishes deferred events and releases the input snapshot.
        let post_frame = TraceZone::new(TraceStage::PostFrame);
        self.scripting.remove_detached();
        let mut world = self.world.borrow_mut();
        self.previous_events = world.event_bus_mut().dispatch();
        world.set_frame_input(Vec::new());
        self.metrics = FrameMetrics {
            fps: if delta > 0.0 { 1.0 / delta } else { 0.0 },
            frame_ms: frame_start.elapsed().as_secs_f64() * 1000.0,
            entities: world.entity_count(),
            luau_bytes: self.scripting.used_memory(),
            draw_calls: 0,
            physics_ms,
            ecs_ms,
        };
        drop(post_frame);
        drop(frame_zone);
        trace_frame();
        Ok(render_result)
    }

    pub fn metrics(&self) -> FrameMetrics {
        self.metrics
    }

    pub fn world(&self) -> &Rc<RefCell<World>> {
        &self.world
    }

    fn synchronize_hierarchy(&mut self) {
        if let Some(scene) = &mut self.scene {
            scene.hierarchy.synchronize(&mut self.world.borrow_mut());
        }
    }

    fn finish_physics(&mut self) -> std::io::Result<()> {
        if let Some(physics) = &mut self.physics {
            physics.finish(&mut self.world.borrow_mut(), FIXED_DELTA as f32)?;
        }
        self.synchronize_hierarchy();
        Ok(())
    }

    pub fn shutdown(&mut self) {
        self.scripting.shutdown();
        self.world.borrow_mut().event_bus_mut().dispatch();
    }
}
