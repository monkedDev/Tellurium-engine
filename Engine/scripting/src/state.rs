use std::collections::{BTreeMap, HashSet};
use std::ffi::c_void;

use mlua::{Error, Function, Lua, Result, Table, Value};
use tellurium_core::EntityId;
use tellurium_serialization::{ScriptState, ScriptValue};

use crate::{quota, Scripting};

const MAX_DEPTH: usize = 32;
const MAX_VALUES: usize = 100_000;

#[cfg(feature = "editor")]
pub(crate) fn reload_payload(value: Value, lua: &Lua) -> Result<Value> {
    if value.is_nil() {
        return Ok(value);
    }
    let mut remaining = MAX_VALUES;
    let state = capture(value, &mut HashSet::new(), &mut remaining, 0)?;
    let mut remaining = MAX_VALUES;
    restore(lua, &state, &mut remaining, 0)
}

impl Scripting {
    pub fn snapshot(&self, entity: EntityId) -> Result<Option<ScriptState>> {
        let Some(script) = self.scripts.get(&entity) else {
            return Ok(None);
        };
        let mut globals = BTreeMap::new();
        let mut visited = HashSet::new();
        let mut remaining = MAX_VALUES;
        for pair in script.environment.pairs::<String, Value>() {
            let (name, value) = pair?;
            if !matches!(value, Value::Function(_)) {
                globals.insert(name, capture(value, &mut visited, &mut remaining, 0)?);
            }
        }
        // Optional hooks let a script expose state stored in closure-local variables.
        let payload = match script
            .environment
            .raw_get::<Option<Function>>("on_snapshot")?
        {
            Some(function) if !script.failed => {
                let value = self.run(
                    entity,
                    script.category,
                    &mut self.quota.frame_budget(),
                    &mut 0,
                    || {
                        script.thread.reset(function)?;
                        quota::set_category(script.thread.state(), script.category);
                        script.thread.resume::<Value>(())
                    },
                )?;
                if value.is_nil() {
                    None
                } else {
                    Some(capture(value, &mut HashSet::new(), &mut remaining, 0)?)
                }
            }
            _ => None,
        };
        Ok(Some(ScriptState {
            started: script.started,
            failed: script.failed,
            globals,
            payload,
        }))
    }

    /// Call after attachment has recreated the script's functions and input subscriptions.
    pub fn restore(&mut self, entity: EntityId, state: &ScriptState) -> Result<()> {
        let script = self
            .scripts
            .get_mut(&entity)
            .ok_or_else(|| Error::runtime("No attached script"))?;
        let mut remaining = MAX_VALUES;
        let mut budget = self.quota.frame_budget();
        let _invocation = self.api.enter(entity, 0);
        self.quota
            .run(&self.lua, script.category, &mut budget, || {
                let mut remove = Vec::new();
                for pair in script.environment.pairs::<String, Value>() {
                    let (name, value) = pair?;
                    if !matches!(value, Value::Function(_)) && !state.globals.contains_key(&name) {
                        remove.push(name);
                    }
                }
                for name in remove {
                    script.environment.raw_set(name, Value::Nil)?;
                }
                for (name, value) in &state.globals {
                    if matches!(
                        script.environment.raw_get::<Value>(name.as_str())?,
                        Value::Function(_)
                    ) {
                        return Err(Error::runtime(format!(
                            "Snapshot overwrites script function {name}"
                        )));
                    }
                    script
                        .environment
                        .raw_set(name.as_str(), restore(&self.lua, value, &mut remaining, 0)?)?;
                }
                if state.started && !state.failed {
                    if let Some(function) = script
                        .environment
                        .raw_get::<Option<Function>>("on_restore")?
                    {
                        let value = state
                            .payload
                            .as_ref()
                            .map(|value| restore(&self.lua, value, &mut remaining, 0))
                            .transpose()?;
                        script.thread.reset(function)?;
                        quota::set_category(script.thread.state(), script.category);
                        script.thread.resume::<()>(value)?;
                    }
                }
                Ok(())
            })?;
        script.started = state.started;
        script.failed = state.failed;
        Ok(())
    }
}

fn capture(
    value: Value,
    visited: &mut HashSet<*const c_void>,
    remaining: &mut usize,
    depth: usize,
) -> Result<ScriptValue> {
    take(remaining, depth)?;
    Ok(match value {
        Value::Boolean(value) => ScriptValue::Bool(value),
        Value::Integer(value) => ScriptValue::Integer(value.into()),
        Value::Number(value) if value.is_finite() => ScriptValue::Number(value),
        Value::String(value) => ScriptValue::String(value.to_str()?.to_string()),
        Value::Table(table) => {
            let pointer = table.to_pointer();
            if !visited.insert(pointer) || table.metatable().is_some() {
                return Err(Error::runtime(
                    "Snapshot tables cannot contain shared references, cycles or metatables",
                ));
            }
            let mut entries = Vec::new();
            for entry in table.pairs::<Value, Value>() {
                let (key, value) = entry?;
                if matches!(key, Value::Table(_)) {
                    return Err(Error::runtime(
                        "Snapshot table keys must be boolean, numeric, or string",
                    ));
                }
                entries.push((
                    capture(key, visited, remaining, depth + 1)?,
                    capture(value, visited, remaining, depth + 1)?,
                ));
            }

            ScriptValue::Table(entries)
        }
        other => {
            return Err(Error::runtime(format!(
                "Snapshot cannot store Luau {}",
                other.type_name()
            )))
        }
    })
}

fn restore(lua: &Lua, value: &ScriptValue, remaining: &mut usize, depth: usize) -> Result<Value> {
    take(remaining, depth)?;
    Ok(match value {
        ScriptValue::Bool(value) => Value::Boolean(*value),
        ScriptValue::Integer(value) => {
            Value::Integer((*value).try_into().map_err(Error::external)?)
        }
        ScriptValue::Number(value) if value.is_finite() => Value::Number(*value),
        ScriptValue::Number(_) => {
            return Err(Error::runtime("Snapshot contains a non-finite number"))
        }
        ScriptValue::String(value) => Value::String(lua.create_string(value)?),
        ScriptValue::Table(entries) => {
            let table: Table = lua.create_table()?;
            for (key, value) in entries {
                if matches!(key, ScriptValue::Table(_)) {
                    return Err(Error::runtime("Snapshot contains an invalid table key"));
                }
                table.raw_set(
                    restore(lua, key, remaining, depth + 1)?,
                    restore(lua, value, remaining, depth + 1)?,
                )?;
            }
            Value::Table(table)
        }
    })
}

fn take(remaining: &mut usize, depth: usize) -> Result<()> {
    if depth >= MAX_DEPTH || *remaining == 0 {
        return Err(Error::runtime(
            "Luau snapshot exceeds table depth or size limit",
        ));
    }
    *remaining -= 1;
    Ok(())
}
