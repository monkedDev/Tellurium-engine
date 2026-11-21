//! One sandboxed Luau VM with an isolated environment and coroutine per script.

mod api;
#[cfg(feature = "editor")]
mod plugin;
mod quota;
#[cfg(feature = "editor")]
mod reload;
mod state;

#[cfg(feature = "editor")]
pub use plugin::EditorPlugin;

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::Duration;

use mlua::{ChunkMode, Error, Function, IntoLuaMulti, Lua, Result, Table, Thread};
#[cfg(feature = "editor")]
use tellurium_assets::AssetRevision;
use tellurium_assets::{Handle, ScriptAsset};
use tellurium_core::{Component, ContactEvent, EntityId, EventBatch, World};
use tellurium_input::{ActionEvent, ActionValue};

use api::EngineApi;
pub use api::ScriptEvent;
pub use mlua::Error as ScriptError;
use quota::{Quota, LAST_CATEGORY};

#[derive(Debug, Clone)]
pub struct LuauConfig {
    /// Maximum live Luau object bytes attributed to an individual script.
    pub max_memory_bytes: usize,
    /// Cumulative execution time per script and frame, measured by a monotonic clock.
    pub max_cpu_seconds_per_frame: f64,
}

impl Default for LuauConfig {
    fn default() -> Self {
        Self {
            max_memory_bytes: 16 * 1024 * 1024,
            max_cpu_seconds_per_frame: 0.005,
        }
    }
}

/// A handle to a script instance owned by the main-thread scripting runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScriptComponent {
    instance: u64,
}

impl Component for ScriptComponent {}

struct Script {
    component: ScriptComponent,
    name: String,
    environment: Table,
    thread: Thread,
    category: i32,
    remaining: Duration,
    emitted: u8,
    started: bool,
    failed: bool,
    #[cfg(feature = "editor")]
    revision: Option<AssetRevision>,
}

pub struct Scripting {
    scripts: BTreeMap<EntityId, Script>,
    last_instance: u64,
    quota: Quota,
    api: Rc<EngineApi>,
    lua: Lua,
}

impl Scripting {
    /// Initialize the allocator limits, interrupt hook and engine bindings once.
    pub fn new(world: Rc<RefCell<World>>, config: LuauConfig) -> Result<Self> {
        let lua = tellurium_safety::luau()?;
        let quota = Quota::new(&lua, config)?;
        let api = Rc::new(EngineApi::new(world));
        api::register(&lua, &api)?;

        tellurium_safety::seal(&lua)?;

        Ok(Self {
            scripts: BTreeMap::new(),
            last_instance: 0,
            quota,
            api,
            lua,
        })
    }

    pub fn used_memory(&self) -> usize {
        self.lua.used_memory()
    }

    /// Register text supplied by trusted Rust code. Scripts receive numeric handles
    /// and can only read resources explicitly registered here.
    pub fn register_text(&mut self, name: String, data: String) -> Result<u32> {
        self.api.register_text(name, data)
    }

    /// Compile source, run its top-level chunk and attach the resulting instance.
    /// on_start runs in the next pre-update.
    /// The VM supports up to 255 concurrent instances, one per Luau memory category.
    pub fn attach(&mut self, entity: EntityId, name: &str, source: &str) -> Result<()> {
        if source.len() > self.quota.max_memory().min(1024 * 1024) {
            return Err(Error::runtime("Luau source is too large"));
        }
        self.attach_chunk(entity, name, source.as_bytes(), ChunkMode::Text)
    }

    /// Attach an asset verified by the importer against the local Luau compiler.
    pub fn attach_asset(
        &mut self,
        entity: EntityId,
        name: &str,
        source: &ScriptAsset,
    ) -> Result<()> {
        self.attach_chunk(entity, name, source.bytecode(), ChunkMode::Binary)
    }

    pub fn attach_handle(
        &mut self,
        entity: EntityId,
        name: &str,
        source: &Handle<ScriptAsset>,
    ) -> Result<()> {
        self.attach_asset(entity, name, source)?;
        #[cfg(feature = "editor")]
        if let Some(script) = self.scripts.get_mut(&entity) {
            script.revision = Some(source.revision());
        }
        Ok(())
    }

    fn attach_chunk(
        &mut self,
        entity: EntityId,
        name: &str,
        source: &[u8],
        mode: ChunkMode,
    ) -> Result<()> {
        if !self.api.world.borrow().is_alive(entity) || self.scripts.contains_key(&entity) {
            return Err(Error::runtime(
                "attach requires a live entity without a script",
            ));
        }
        let category = self.free_category()?;
        let mut remaining = self.quota.frame_budget();
        let mut emitted = 0;
        let attachment = self.run(entity, category, &mut remaining, &mut emitted, || {
            self.load_chunk(category, name, source, mode)
        });
        let (environment, thread) = match attachment {
            Ok(attachment) => attachment,
            Err(error) => {
                self.api.forget_actions(entity);
                return Err(error);
            }
        };

        self.last_instance = self
            .last_instance
            .checked_add(1)
            .ok_or_else(|| Error::runtime("script IDs exhausted"))?;
        let component = ScriptComponent {
            instance: self.last_instance,
        };
        self.api.world.borrow_mut().add_component(entity, component);
        self.scripts.insert(
            entity,
            Script {
                component,
                name: name.to_owned(),
                environment,
                thread,
                category,
                remaining,
                emitted,
                started: false,
                failed: false,
                #[cfg(feature = "editor")]
                revision: None,
            },
        );
        Ok(())
    }

    fn load_chunk(
        &self,
        category: i32,
        name: &str,
        source: &[u8],
        mode: ChunkMode,
    ) -> Result<(Table, Thread)> {
        let environment = self.lua.create_table()?;
        let lookup = self.lua.create_table()?;
        lookup.raw_set("__index", self.lua.globals())?;
        lookup.set_readonly(true);
        environment.set_metatable(Some(lookup))?;
        environment.set_safeenv(true);
        let entry = self
            .lua
            .load(source)
            .set_mode(mode)
            .set_name(name)
            .set_environment(environment.clone())
            .into_function()?;
        let thread = self.lua.create_thread(entry)?;
        quota::set_category(thread.state(), category);
        thread.resume::<()>(())?;
        Ok((environment, thread))
    }

    /// Change the per-frame data visible to scripts and replenish execution budgets.
    pub fn begin_frame(&mut self, delta: f64, elapsed: f64, previous_events: &EventBatch) {
        self.api.begin_frame(delta, elapsed, previous_events);
        for script in self.scripts.values_mut() {
            script.remaining = self.quota.frame_budget();
            script.emitted = 0;
        }
    }

    pub fn pre_update(&mut self) {
        self.remove_detached();
        self.for_each(|runtime, entity, script| {
            if !script.started {
                script.started = true;
                runtime.call(entity, script, "on_start", ());
            }
        });
    }

    pub fn update(&mut self, delta: f64) {
        self.remove_detached();
        self.for_each(|runtime, entity, script| runtime.call(entity, script, "on_update", delta));
    }

    /// Invoke explicit input subscriptions in the owning script's coroutine and budget.
    pub fn dispatch_actions(&mut self, events: &[ActionEvent]) {
        if events.is_empty() {
            return;
        }
        self.for_each(|runtime, entity, script| {
            for event in events {
                let Some(callback) = runtime.api.action(entity, &event.name) else {
                    continue;
                };
                match event.value {
                    ActionValue::Digital(pressed) => runtime.call_with(
                        entity,
                        script,
                        &event.name,
                        |_| Ok(Some(callback)),
                        pressed,
                    ),
                    ActionValue::Axis([x, y]) => runtime.call_with(
                        entity,
                        script,
                        &event.name,
                        |_| Ok(Some(callback)),
                        (x, y),
                    ),
                    ActionValue::PassThrough([x, y]) => runtime.call_with(
                        entity,
                        script,
                        &event.name,
                        |_| Ok(Some(callback)),
                        (x, y),
                    ),
                }
            }
        });
    }

    pub fn fixed_update(&mut self, delta: f64) {
        self.for_each(|runtime, entity, script| {
            runtime.call(entity, script, "on_fixed_update", delta)
        });
    }

    /// Deliver synchronized physics events to the two participating entities.
    pub fn deliver_contacts(&mut self, events: &EventBatch) {
        for event in events.of_type::<ContactEvent>() {
            let (handler, a, b) = match *event {
                ContactEvent::Collision(a, b) => ("on_collision", a, b),
                ContactEvent::Trigger(a, b) => ("on_trigger", a, b),
            };
            self.deliver_contact(a, b, handler);
            if a != b {
                self.deliver_contact(b, a, handler);
            }
        }
    }

    fn deliver_contact(&mut self, entity: EntityId, other: EntityId, handler: &str) {
        if let Some(mut script) = self.scripts.remove(&entity) {
            self.call(entity, &mut script, handler, other.0.to_string());
            self.scripts.insert(entity, script);
        }
    }

    /// Finish callbacks for removed ScriptComponents before the next frame.
    pub fn remove_detached(&mut self) {
        self.for_each(|_, _, _| {});
    }

    /// Run on_destroy for all remaining scripts while the world is still accessible.
    pub fn shutdown(&mut self) {
        for (entity, mut script) in std::mem::take(&mut self.scripts) {
            script.remaining = self.quota.frame_budget();
            script.emitted = 0;
            self.destroy(entity, &mut script);
            self.api
                .world
                .borrow_mut()
                .remove_component::<ScriptComponent>(entity);
        }
    }

    fn free_category(&self) -> Result<i32> {
        let occupied = |category| {
            self.scripts
                .values()
                .any(|script| script.category == category)
        };
        for collect in [false, true] {
            if collect {
                self.lua.gc_collect()?;
            }
            if let Some(category) = (1..=LAST_CATEGORY)
                .find(|id| !occupied(*id) && self.quota.used_by(&self.lua, *id) == 0)
            {
                return Ok(category);
            }
        }
        Err(Error::runtime("no free Luau script memory category"))
    }

    fn for_each(&mut self, mut callback: impl FnMut(&Self, EntityId, &mut Script)) {
        let mut scripts = std::mem::take(&mut self.scripts);
        scripts.retain(|entity, script| {
            let attached = self
                .api
                .world
                .borrow()
                .get_component::<ScriptComponent>(*entity)
                == Some(&script.component);
            if attached {
                callback(self, *entity, script);
            } else {
                self.destroy(*entity, script);
            }
            attached
        });
        self.scripts = scripts;
    }

    fn destroy(&self, entity: EntityId, script: &mut Script) {
        script.failed = false;
        self.call(entity, script, "on_destroy", ());
        self.api.forget_actions(entity);
    }

    fn call(&self, entity: EntityId, script: &mut Script, handler: &str, args: impl IntoLuaMulti) {
        self.call_with(
            entity,
            script,
            handler,
            |environment| environment.raw_get(handler),
            args,
        );
    }

    fn call_with(
        &self,
        entity: EntityId,
        script: &mut Script,
        handler: &str,
        callback: impl FnOnce(&Table) -> Result<Option<Function>>,
        args: impl IntoLuaMulti,
    ) {
        if script.failed || script.remaining.is_zero() {
            return;
        }
        let result = self.run(
            entity,
            script.category,
            &mut script.remaining,
            &mut script.emitted,
            || {
                let function = callback(&script.environment)?;
                if let Some(function) = function {
                    script.thread.reset(function)?;
                    quota::set_category(script.thread.state(), script.category);
                    script.thread.resume::<()>(args)?;
                }
                Ok(())
            },
        );
        if let Err(error) = result {
            script.failed = true;
            log::error!(
                "Luau {} on entity {} ({handler}): {error}",
                script.name,
                entity.0
            );
        }
    }

    fn run<T>(
        &self,
        entity: EntityId,
        category: i32,
        remaining: &mut Duration,
        emitted: &mut u8,
        action: impl FnOnce() -> Result<T>,
    ) -> Result<T> {
        let invoke = self.api.enter(entity, *emitted);
        let result = self.quota.run(&self.lua, category, remaining, action);
        *emitted = invoke.emitted();
        result
    }
}
