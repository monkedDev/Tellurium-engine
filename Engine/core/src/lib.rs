//! Entity storage, systems and frame events.

use std::any::{Any, TypeId};
use std::collections::{HashMap, HashSet};

use tellurium_platform::RawInputEvent;

pub use tellurium_bridge::Transform;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
pub struct EntityId(pub u64);

impl EntityId {
    pub const NULL: Self = Self(0);
}

/// Data stored directly in a typed ECS pool.
pub trait Component: Send + Sync + 'static {}

pub type System = dyn Fn(&mut World) + Send + Sync;

impl Component for Transform {}

/// Dense pool with constant-time lookup. Removal may change iteration order.
pub struct ComponentStorage<T: Component> {
    entries: Vec<(EntityId, T)>,
    indices: HashMap<EntityId, usize>,
}

impl<T: Component> Default for ComponentStorage<T> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            indices: HashMap::new(),
        }
    }
}

impl<T: Component> ComponentStorage<T> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, entity: EntityId, component: T) -> Option<T> {
        if let Some(index) = self.indices.get(&entity) {
            return Some(std::mem::replace(&mut self.entries[*index].1, component));
        }
        self.indices.insert(entity, self.entries.len());
        self.entries.push((entity, component));
        None
    }

    pub fn get(&self, entity: EntityId) -> Option<&T> {
        self.indices
            .get(&entity)
            .map(|index| &self.entries[*index].1)
    }

    pub fn get_mut(&mut self, entity: EntityId) -> Option<&mut T> {
        let index = *self.indices.get(&entity)?;
        Some(&mut self.entries[index].1)
    }

    pub fn remove(&mut self, entity: EntityId) -> Option<T> {
        let index = self.indices.remove(&entity)?;
        let (_, component) = self.entries.swap_remove(index);
        if let Some((moved, _)) = self.entries.get(index) {
            self.indices.insert(*moved, index);
        }
        Some(component)
    }

    pub fn contains(&self, entity: EntityId) -> bool {
        self.indices.contains_key(&entity)
    }

    pub fn iter(&self) -> impl Iterator<Item = (EntityId, &T)> {
        self.entries.iter().map(|(id, value)| (*id, value))
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = (EntityId, &mut T)> {
        self.entries.iter_mut().map(|(id, value)| (*id, value))
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

trait ErasedStorage: Send + Sync {
    fn remove_entity(&mut self, entity: EntityId);
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

impl<T: Component> ErasedStorage for ComponentStorage<T> {
    fn remove_entity(&mut self, entity: EntityId) {
        self.remove(entity);
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityLifecycle {
    Spawned(EntityId),
    Despawned(EntityId),
}

/// Contact events use ECS IDs and can be published by any physics backend.
#[derive(Debug, Clone, Copy)]
pub enum ContactEvent {
    Collision(EntityId, EntityId),
    Trigger(EntityId, EntityId),
}

#[derive(Default)]
pub struct World {
    last_entity_id: u64,
    entities: HashSet<EntityId>,
    storages: HashMap<TypeId, Box<dyn ErasedStorage>>,
    systems: Vec<Box<System>>,
    event_bus: EventBus,
    frame_input: Vec<RawInputEvent>,
}

impl World {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn spawn(&mut self) -> EntityId {
        let id = EntityId(
            self.last_entity_id
                .checked_add(1)
                .expect("entity IDs exhausted"),
        );
        self.spawn_with_id(id);
        id
    }

    /// Restore a specific entity ID, preserving serialized references.
    pub fn spawn_with_id(&mut self, id: EntityId) -> bool {
        if id == EntityId::NULL || !self.entities.insert(id) {
            return false;
        }
        self.reserve_entity_ids(id.0);
        self.event_bus.publish(EntityLifecycle::Spawned(id));
        true
    }

    pub fn reserve_entity_ids(&mut self, last_id: u64) {
        self.last_entity_id = self.last_entity_id.max(last_id);
    }

    pub fn last_entity_id(&self) -> u64 {
        self.last_entity_id
    }

    pub fn entities(&self) -> impl Iterator<Item = EntityId> + '_ {
        self.entities.iter().copied()
    }

    pub fn entity_count(&self) -> usize {
        self.entities.len()
    }

    pub fn is_alive(&self, entity: EntityId) -> bool {
        self.entities.contains(&entity)
    }

    pub fn despawn(&mut self, entity: EntityId) -> bool {
        if !self.entities.remove(&entity) {
            return false;
        }
        for storage in self.storages.values_mut() {
            storage.remove_entity(entity);
        }
        self.event_bus.publish(EntityLifecycle::Despawned(entity));
        true
    }

    /// Insert or replace a component on a live entity.
    pub fn add_component<T: Component>(&mut self, entity: EntityId, component: T) -> bool {
        if !self.is_alive(entity) {
            return false;
        }
        let storage = self
            .storages
            .entry(TypeId::of::<T>())
            .or_insert_with(|| Box::new(ComponentStorage::<T>::new()));
        storage
            .as_any_mut()
            .downcast_mut::<ComponentStorage<T>>()
            .expect("storage must match its type ID")
            .insert(entity, component);
        true
    }

    pub fn get_component<T: Component>(&self, entity: EntityId) -> Option<&T> {
        self.storage::<T>()?.get(entity)
    }

    pub fn get_component_mut<T: Component>(&mut self, entity: EntityId) -> Option<&mut T> {
        self.storage_mut::<T>()?.get_mut(entity)
    }

    pub fn remove_component<T: Component>(&mut self, entity: EntityId) -> Option<T> {
        self.storage_mut::<T>()?.remove(entity)
    }

    pub fn has_component<T: Component>(&self, entity: EntityId) -> bool {
        self.get_component::<T>(entity).is_some()
    }

    /// Iterate the pool directly. Use query2 to intersect two pools.
    pub fn query<T: Component>(&self) -> impl Iterator<Item = (EntityId, &T)> {
        self.storage::<T>()
            .into_iter()
            .flat_map(ComponentStorage::iter)
    }

    pub fn query_mut<T: Component>(&mut self) -> impl Iterator<Item = (EntityId, &mut T)> {
        self.storage_mut::<T>()
            .into_iter()
            .flat_map(ComponentStorage::iter_mut)
    }

    pub fn query2<A: Component, B: Component>(&self) -> impl Iterator<Item = (EntityId, &A, &B)> {
        let other = self.storage::<B>();
        self.query::<A>()
            .filter_map(move |(entity, a)| Some((entity, a, other?.get(entity)?)))
    }

    fn storage<T: Component>(&self) -> Option<&ComponentStorage<T>> {
        self.storages
            .get(&TypeId::of::<T>())?
            .as_any()
            .downcast_ref()
    }

    fn storage_mut<T: Component>(&mut self) -> Option<&mut ComponentStorage<T>> {
        self.storages
            .get_mut(&TypeId::of::<T>())?
            .as_any_mut()
            .downcast_mut()
    }

    pub fn add_system(&mut self, system: impl Fn(&mut World) + Send + Sync + 'static) {
        self.systems.push(Box::new(system));
    }

    /// Existing systems run in registration order. New ones start next frame.
    pub fn run_systems(&mut self) {
        let systems = std::mem::take(&mut self.systems);
        for system in &systems {
            system(self);
        }
        let added = std::mem::replace(&mut self.systems, systems);
        self.systems.extend(added);
    }

    /// Replace the input snapshot once, before any frame systems run.
    pub fn set_frame_input(&mut self, events: Vec<RawInputEvent>) {
        for event in &events {
            self.event_bus.publish_immediate(event);
        }
        self.frame_input = events;
    }

    pub fn frame_input(&self) -> &[RawInputEvent] {
        &self.frame_input
    }

    pub fn event_bus(&self) -> &EventBus {
        &self.event_bus
    }

    pub fn event_bus_mut(&mut self) -> &mut EventBus {
        &mut self.event_bus
    }
}

type Event = dyn Any + Send + Sync;
type Subscriber = dyn Fn(&dyn Any) + Send + Sync;

/// Snapshot of deferred events in publication order.
#[derive(Default)]
pub struct EventBatch(Vec<Box<Event>>);

impl EventBatch {
    pub fn of_type<E: 'static>(&self) -> impl Iterator<Item = &E> {
        self.0.iter().filter_map(|event| event.downcast_ref::<E>())
    }
}

#[derive(Default)]
pub struct EventBus {
    subscribers: HashMap<TypeId, Vec<Box<Subscriber>>>,
    pending: EventBatch,
}

impl EventBus {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn subscribe<E: Send + Sync + 'static, F: Fn(&E) + Send + Sync + 'static>(
        &mut self,
        callback: F,
    ) {
        self.subscribers
            .entry(TypeId::of::<E>())
            .or_default()
            .push(Box::new(move |event| {
                if let Some(event) = event.downcast_ref::<E>() {
                    callback(event);
                }
            }));
    }

    pub fn publish<E: Send + Sync + 'static>(&mut self, event: E) {
        self.pending.0.push(Box::new(event));
    }

    pub fn publish_immediate<E: Send + Sync + 'static>(&self, event: &E) {
        self.notify(event);
    }

    fn notify(&self, event: &dyn Any) {
        if let Some(subscribers) = self.subscribers.get(&event.type_id()) {
            for subscriber in subscribers {
                subscriber(event);
            }
        }
    }

    pub fn dispatch(&mut self) -> EventBatch {
        let events = std::mem::take(&mut self.pending);
        for event in &events.0 {
            self.notify(event.as_ref());
        }
        events
    }
}
