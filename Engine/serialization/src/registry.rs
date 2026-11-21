use std::collections::BTreeMap;
use std::io;

use serde::{de::DeserializeOwned, Serialize};
use tellurium_core::{Component, EntityId, World};

use crate::invalid;

type Encode = fn(&World, EntityId) -> io::Result<Option<String>>;
type Decode = fn(&mut World, EntityId, &str) -> io::Result<()>;

/// Register user-defined ECS components under stable scene names.
#[derive(Default)]
pub struct ComponentRegistry {
    codecs: BTreeMap<String, (Encode, Decode)>,
}

impl ComponentRegistry {
    pub fn register<T: Component + Serialize + DeserializeOwned>(
        &mut self,
        name: &str,
    ) -> io::Result<()> {
        if name.trim().is_empty() || self.codecs.contains_key(name) {
            return Err(invalid(format!(
                "Invalid or duplicate component name {name:?}"
            )));
        }
        self.codecs.insert(
            name.to_owned(),
            (
                |world, id| {
                    world
                        .get_component::<T>(id)
                        .map(serde_json::to_string)
                        .transpose()
                        .map_err(invalid)
                },
                |world, id, data| {
                    let value = serde_json::from_str::<T>(data).map_err(invalid)?;
                    if !world.add_component(id, value) {
                        return Err(invalid(format!("Entity {} does not exist", id.0)));
                    }
                    Ok(())
                },
            ),
        );
        Ok(())
    }

    pub fn capture(&self, world: &World, entity: EntityId) -> io::Result<BTreeMap<String, String>> {
        let mut result = BTreeMap::new();
        for (name, (encode, _)) in &self.codecs {
            if let Some(data) = encode(world, entity)? {
                result.insert(name.clone(), data);
            }
        }
        Ok(result)
    }

    pub fn apply(
        &self,
        world: &mut World,
        entity: EntityId,
        components: &BTreeMap<String, String>,
    ) -> io::Result<()> {
        for (name, data) in components {
            let (_, decode) = self
                .codecs
                .get(name)
                .ok_or_else(|| invalid(format!("Unregistered component {name}")))?;
            decode(world, entity, data)?;
        }
        Ok(())
    }
}
