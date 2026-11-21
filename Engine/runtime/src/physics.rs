use std::collections::HashMap;
use std::io;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use tellurium_bridge::{
    create_physics_world, step_physics, sync_physics_bodies, take_physics_frame, BodyDesc,
    BodyKind, ColliderKind, PhysicsFrame,
};
use tellurium_core::{Component, ContactEvent, EntityId, Transform, World};

#[derive(Clone, Copy)]
pub(crate) struct Rigidbody {
    pub kind: BodyKind,
    pub mass: f32,
    pub linear_velocity: [f32; 3],
}

impl Component for Rigidbody {}

impl Rigidbody {
    pub(crate) fn fixed() -> Self {
        Self {
            kind: BodyKind::Static,
            mass: 0.0,
            linear_velocity: [0.0; 3],
        }
    }

    pub(crate) fn dynamic(mass: f32) -> Self {
        Self {
            kind: BodyKind::Dynamic,
            mass,
            linear_velocity: [0.0; 3],
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Collider {
    pub shape: ColliderKind,
    pub dimensions: [f32; 3],
    pub trigger: bool,
}

impl Component for Collider {}

#[derive(Clone, Copy)]
struct BodyInput {
    pose: Transform,
    velocity: [f32; 3],
}

struct Work {
    bodies: Vec<BodyDesc>,
    steps: u8,
    delta: f32,
}

/// Only the worker owns Bullet. Snapshots cross once at fixed-step boundaries.
pub(crate) struct PhysicsSystem {
    work: Option<Sender<Work>>,
    results: Receiver<io::Result<(PhysicsFrame, f64)>>,
    worker: Option<JoinHandle<()>>,
    in_flight: Option<HashMap<EntityId, BodyInput>>,
    known: HashMap<EntityId, BodyInput>,
    waiting_steps: u8,
    last_step_ms: f64,
}

impl PhysicsSystem {
    pub(crate) fn new(gravity: f32) -> io::Result<Self> {
        let (work, jobs) = mpsc::channel::<Work>();
        let (complete, results) = mpsc::channel();
        let (initialized, ready) = mpsc::sync_channel(0);
        let worker = thread::Builder::new()
            .name("tellurium-bullet".into())
            .spawn(move || {
                let mut world = match create_physics_world(gravity) {
                    Ok(world) => world,
                    Err(error) => {
                        let _ = initialized.send(Err(io::Error::other(error)));
                        return;
                    }
                };
                if initialized.send(Ok(())).is_err() {
                    return;
                }
                for work in jobs {
                    let result = (|| {
                        sync_physics_bodies(world.pin_mut(), &work.bodies)?;
                        let start = Instant::now();
                        for _ in 0..work.steps {
                            step_physics(world.pin_mut(), work.delta)?;
                        }
                        let step_ms = start.elapsed().as_secs_f64() * 1000.0;
                        Ok((take_physics_frame(world.pin_mut())?, step_ms))
                    })()
                    .map_err(|error: tellurium_bridge::NativeError| io::Error::other(error));
                    if complete.send(result).is_err() {
                        return;
                    }
                }
            })?;
        ready.recv().map_err(|_| disconnected())??;
        Ok(Self {
            work: Some(work),
            results,
            worker: Some(worker),
            in_flight: None,
            known: HashMap::new(),
            waiting_steps: 0,
            last_step_ms: 0.0,
        })
    }

    pub(crate) fn last_step_ms(&self) -> f64 {
        self.last_step_ms
    }

    pub(crate) fn synchronize(
        &mut self,
        world: &mut World,
        steps: u8,
        fixed_delta: f32,
    ) -> io::Result<()> {
        self.waiting_steps = self.waiting_steps.saturating_add(steps).min(4);
        match self.results.try_recv() {
            Ok(result) => {
                let (frame, step_ms) = result?;
                self.last_step_ms = step_ms;
                self.apply(world, frame);
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => return Err(disconnected()),
        }
        if self.in_flight.is_some() || self.waiting_steps == 0 {
            return Ok(());
        }

        let mut bodies = Vec::new();
        let mut submitted = HashMap::new();
        for (entity, body, collider) in world.query2::<Rigidbody, Collider>() {
            let Some(&pose) = world.get_component::<Transform>(entity) else {
                continue;
            };
            let previous = self.known.get(&entity);
            bodies.push(BodyDesc {
                entity: entity.0,
                pose,
                linear_velocity: body.linear_velocity,
                kind: body.kind,
                collider: collider.shape,
                dimensions: collider.dimensions,
                mass: body.mass,
                trigger: collider.trigger,
                override_pose: previous.is_none_or(|input| input.pose != pose),
                override_velocity: previous
                    .is_none_or(|input| input.velocity != body.linear_velocity),
            });
            submitted.insert(
                entity,
                BodyInput {
                    pose,
                    velocity: body.linear_velocity,
                },
            );
        }
        self.work
            .as_ref()
            .ok_or_else(disconnected)?
            .send(Work {
                bodies,
                steps: self.waiting_steps,
                delta: fixed_delta,
            })
            .map_err(|_| disconnected())?;
        self.waiting_steps = 0;
        self.in_flight = Some(submitted);
        Ok(())
    }

    /// Drain submitted and queued fixed steps at a pause or snapshot boundary.
    pub(crate) fn finish(&mut self, world: &mut World, fixed_delta: f32) -> io::Result<()> {
        while self.in_flight.is_some() || self.waiting_steps != 0 {
            if self.in_flight.is_some() {
                let (frame, step_ms) = self.results.recv().map_err(|_| disconnected())??;
                self.last_step_ms = step_ms;
                self.apply(world, frame);
            }
            self.synchronize(world, 0, fixed_delta)?;
        }
        Ok(())
    }

    fn apply(&mut self, world: &mut World, frame: PhysicsFrame) {
        let Some(submitted) = self.in_flight.take() else {
            return;
        };
        self.known = submitted;
        for state in frame.bodies {
            let entity = EntityId(state.entity);
            let Some(input) = self.known.get_mut(&entity) else {
                continue;
            };
            if !has_body(world, entity) {
                continue;
            }
            if world
                .get_component::<Rigidbody>(entity)
                .is_some_and(|body| body.kind != BodyKind::Dynamic)
            {
                continue;
            }
            // Keep edits made by gameplay while this snapshot was on the worker.
            if let Some(pose) = world.get_component_mut::<Transform>(entity) {
                for (axis, position) in pose.position.iter_mut().enumerate() {
                    if *position == input.pose.position[axis] {
                        *position = state.position[axis];
                    }
                }
                if pose.rotation == input.pose.rotation {
                    pose.rotation = state.rotation;
                }
            }
            if let Some(body) = world.get_component_mut::<Rigidbody>(entity) {
                if body.kind == BodyKind::Dynamic && body.linear_velocity == input.velocity {
                    body.linear_velocity = state.linear_velocity;
                }
            }
            input.pose.position = state.position;
            input.pose.rotation = state.rotation;
            input.velocity = state.linear_velocity;
        }
        for contact in frame.contacts {
            let a = EntityId(contact.first);
            let b = EntityId(contact.second);
            if has_body(world, a) && has_body(world, b) {
                let event = if contact.trigger {
                    ContactEvent::Trigger(a, b)
                } else {
                    ContactEvent::Collision(a, b)
                };
                world.event_bus_mut().publish(event);
            }
        }
    }
}

impl Drop for PhysicsSystem {
    fn drop(&mut self) {
        self.work.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn has_body(world: &World, entity: EntityId) -> bool {
    world.has_component::<Rigidbody>(entity)
        && world.has_component::<Collider>(entity)
        && world.has_component::<Transform>(entity)
}

fn disconnected() -> io::Error {
    io::Error::new(
        io::ErrorKind::BrokenPipe,
        "Bullet physics worker is unavailable",
    )
}
