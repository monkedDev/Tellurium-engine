use mlua::{ChunkMode, Error, Function, Lua, Result, Thread};

use crate::{quota, LuauConfig};

/// A separate interpreter with no game ECS, filesystem or device bindings.
pub struct EditorPlugin {
    handler: Function,
    thread: Thread,
    quota: quota::Quota,
    lua: Lua,
}

impl EditorPlugin {
    pub const MAX_SOURCE: usize = 64 * 1024;

    pub fn load(name: &str, source: &str) -> Result<Self> {
        if source.len() > Self::MAX_SOURCE {
            return Err(Error::runtime("editor plugin source exceeds 64 KiB"));
        }
        let lua = tellurium_safety::luau()?;
        let quota = quota::Quota::new(
            &lua,
            LuauConfig {
                max_memory_bytes: 2 * 1024 * 1024,
                max_cpu_seconds_per_frame: 0.003,
            },
        )?;
        tellurium_safety::seal(&lua)?;
        let (handler, thread) = quota.run(&lua, 1, &mut quota.frame_budget(), || {
            let start = lua
                .load(source)
                .set_mode(ChunkMode::Text)
                .set_name(name)
                .into_function()?;
            let thread = lua.create_thread(start)?;
            quota::set_category(thread.state(), 1);
            let handler = thread.resume::<Function>(())?;
            Ok((handler, thread))
        })?;
        Ok(Self {
            handler,
            thread,
            quota,
            lua,
        })
    }

    pub fn dispatch(&self, kind: &str, path: &str) -> Result<Option<String>> {
        self.quota
            .run(&self.lua, 1, &mut self.quota.frame_budget(), || {
                self.thread.reset(self.handler.clone())?;
                quota::set_category(self.thread.state(), 1);
                let value = self.thread.resume::<Option<mlua::String>>((kind, path))?;
                match value {
                    Some(text) if text.as_bytes().len() > 1024 => {
                        Err(Error::runtime("editor plugin output exceeds 1 KiB"))
                    }
                    Some(text) => Ok(Some(text.to_str()?.to_owned())),
                    None => Ok(None),
                }
            })
    }
}
