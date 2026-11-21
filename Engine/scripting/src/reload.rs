use mlua::{ChunkMode, Function, Result, Value};
use tellurium_assets::{Handle, ScriptAsset};
use tellurium_core::EntityId;

use crate::{quota, Script, Scripting};

impl Scripting {
    /// Swap each matching script in place. A failing candidate leaves that instance active.
    pub fn reload_asset(&mut self, name: &str, asset: &Handle<ScriptAsset>) -> usize {
        let normalized = name.replace('\\', "/");
        let mut updated = 0;
        self.for_each(|runtime, entity, script| {
            if script.revision == Some(asset.revision())
                || !script
                    .name
                    .replace('\\', "/")
                    .eq_ignore_ascii_case(&normalized)
            {
                return;
            }
            match runtime.reload_one(entity, script, asset) {
                Ok(()) => {
                    script.revision = Some(asset.revision());
                    updated += 1;
                }
                Err(error) => log::warn!("Luau reload {name} on entity {}: {error}", entity.0),
            }
        });
        updated
    }

    pub fn inspect_globals(&self, entity: EntityId) -> Vec<(String, String)> {
        let Some(script) = self.scripts.get(&entity) else {
            return Vec::new();
        };
        let mut fields: Vec<_> = script
            .environment
            .pairs::<Value, Value>()
            .take(128)
            .filter_map(|entry| {
                let (Value::String(name), value) = entry.ok()? else {
                    return None;
                };
                if matches!(value, Value::Function(_)) {
                    return None;
                }
                let name = name.to_str().ok()?.chars().take(64).collect();
                let value = match value {
                    Value::Boolean(value) => value.to_string(),
                    Value::Integer(value) => value.to_string(),
                    Value::Number(value) => value.to_string(),
                    Value::String(value) => value.to_str().ok()?.chars().take(128).collect(),
                    other => other.type_name().to_owned(),
                };
                Some((name, value))
            })
            .take(32)
            .collect();
        fields.sort_unstable();
        fields
    }

    fn reload_one(&self, entity: EntityId, script: &mut Script, asset: &ScriptAsset) -> Result<()> {
        let _staging = self.api.stage_reload();
        let old_actions = self.api.take_actions(entity);
        let result = self.run(
            entity,
            script.category,
            &mut self.quota.frame_budget(),
            &mut 0,
            || {
                let callback = |env: &mlua::Table, name| env.raw_get::<Option<Function>>(name);
                let invoke = |thread: &mlua::Thread, function: Function| {
                    thread.reset(function)?;
                    quota::set_category(thread.state(), script.category);
                    Ok::<_, mlua::Error>(())
                };
                let payload = if !script.failed {
                    if let Some(snapshot) = callback(&script.environment, "on_snapshot")? {
                        // Use a disposable thread. An error cannot invalidate the old thread.
                        let thread = self.lua.create_thread(snapshot)?;
                        quota::set_category(thread.state(), script.category);
                        super::state::reload_payload(thread.resume::<Value>(())?, &self.lua)?
                    } else {
                        Value::Nil
                    }
                } else {
                    Value::Nil
                };
                self.api.forget_actions(entity);
                let (environment, thread) = self.load_chunk(
                    script.category,
                    &script.name,
                    asset.bytecode(),
                    ChunkMode::Binary,
                )?;
                if script.started {
                    if let Some(restore) = callback(&environment, "on_restore")? {
                        invoke(&thread, restore)?;
                        thread.resume::<()>(payload)?;
                    }
                    if let Some(reload) = callback(&environment, "on_reload")? {
                        invoke(&thread, reload)?;
                        thread.resume::<()>(())?;
                    }
                }
                // Preserve live Luau values, including cyclic tables, without touching the old table.
                for pair in script.environment.pairs::<Value, Value>() {
                    let (key, value) = pair?;
                    if !matches!(value, Value::Function(_))
                        && !matches!(
                            environment.raw_get::<Value>(key.clone())?,
                            Value::Function(_)
                        )
                    {
                        environment.raw_set(key, value)?;
                    }
                }
                Ok((environment, thread))
            },
        );
        match result {
            Ok((environment, thread)) => {
                script.environment = environment;
                script.thread = thread;
                script.failed = false;
                Ok(())
            }
            Err(error) => {
                self.api.replace_actions(entity, old_actions);
                Err(error)
            }
        }
    }
}
