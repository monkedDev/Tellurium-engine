use std::collections::HashMap;
use std::io;

use glam::{Quat, Vec3};
use tellurium_core::{Component, EntityId, Transform, World};

use crate::PackedScene;

#[derive(Debug, Clone)]
pub struct SceneNode {
    pub name: String,
    pub parent: Option<EntityId>,
}

impl Component for SceneNode {}

/// ECS transforms are world-space. Scene documents store local transforms.
pub struct Hierarchy {
    nodes: Vec<(EntityId, Option<EntityId>)>,
    local: HashMap<EntityId, Transform>,
    last_world: HashMap<EntityId, Transform>,
}

impl Hierarchy {
    pub fn install(scene: &PackedScene, world: &mut World) -> io::Result<Self> {
        let order = scene.order()?;
        if world.entities().next().is_some() {
            return Err(crate::invalid("Scene installation requires an empty world"));
        }
        let mut hierarchy = Self {
            nodes: Vec::with_capacity(order.len()),
            local: HashMap::with_capacity(order.len()),
            last_world: HashMap::with_capacity(order.len()),
        };
        for index in order {
            let node = &scene.nodes[index];
            let pose = node.parent.map_or(node.transform, |parent| {
                compose(hierarchy.last_world[&parent], node.transform)
            });
            if !Vec3::from_array(pose.position).is_finite()
                || !Vec3::from_array(pose.scale).is_finite()
                || pose.scale.contains(&0.0)
            {
                return Err(crate::invalid("Hierarchy transform overflow or underflow"));
            }
            hierarchy.nodes.push((node.id, node.parent));
            hierarchy.local.insert(node.id, node.transform);
            hierarchy.last_world.insert(node.id, pose);
        }
        for node in &scene.nodes {
            world.spawn_with_id(node.id);
            world.add_component(
                node.id,
                SceneNode {
                    name: node.name.clone(),
                    parent: node.parent,
                },
            );
            world.add_component(node.id, hierarchy.last_world[&node.id]);
        }
        world.reserve_entity_ids(scene.last_id());
        Ok(hierarchy)
    }

    /// Detect world-space writes by physics and scripts, then propagate unchanged children.
    pub fn synchronize(&mut self, world: &mut World) {
        for (entity, parent) in &mut self.nodes {
            let Some(pose) = world.get_component_mut::<Transform>(*entity) else {
                self.last_world.remove(entity);
                continue;
            };
            let local = self
                .local
                .get_mut(entity)
                .expect("installed node has a local transform");
            let previous = self.last_world.get(entity);
            let parent_pose = parent.and_then(|id| self.last_world.get(&id).copied());
            if let Some(parent_pose) = parent_pose {
                if previous != Some(pose) {
                    *local = relative(parent_pose, *pose);
                } else {
                    *pose = compose(parent_pose, *local);
                }
            } else {
                *local = *pose;
            }
            self.last_world.insert(*entity, *pose);
        }
    }

    pub fn local_transform(&self, id: EntityId) -> Option<Transform> {
        self.last_world.get(&id)?;
        self.local.get(&id).copied()
    }
}

pub fn compose(parent: Transform, local: Transform) -> Transform {
    let parent_rotation = Quat::from_array(parent.rotation);
    let parent_scale = Vec3::from_array(parent.scale);
    Transform {
        position: (Vec3::from_array(parent.position)
            + parent_rotation * (parent_scale * Vec3::from_array(local.position)))
        .to_array(),
        rotation: (parent_rotation * Quat::from_array(local.rotation))
            .normalize()
            .to_array(),
        scale: (parent_scale * Vec3::from_array(local.scale)).to_array(),
    }
}

pub fn relative(parent: Transform, world: Transform) -> Transform {
    let inverse_rotation = Quat::from_array(parent.rotation).conjugate();
    let parent_scale = Vec3::from_array(parent.scale);
    Transform {
        position: ((inverse_rotation
            * (Vec3::from_array(world.position) - Vec3::from_array(parent.position)))
            / parent_scale)
            .to_array(),
        rotation: (inverse_rotation * Quat::from_array(world.rotation))
            .normalize()
            .to_array(),
        scale: (Vec3::from_array(world.scale) / parent_scale).to_array(),
    }
}
