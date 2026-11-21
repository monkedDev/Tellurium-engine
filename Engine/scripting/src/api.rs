use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use mlua::{Error, Function, Lua, Result, Table};
use tellurium_core::{EntityId, EventBatch, Transform, World};
use tellurium_platform::{RawInputEvent, TouchPhase};

const MAX_EVENT_NAME: usize = 64;
const MAX_EVENT_PAYLOAD: usize = 1024;
const MAX_EVENTS_PER_FRAME: u8 = 32;
const MAX_ACTIONS_PER_SCRIPT: usize = 32;

#[derive(Debug, Clone)]
pub struct ScriptEvent {
    pub sender: EntityId,
    pub name: String,
    pub payload: String,
}

#[derive(Default)]
struct Resources {
    names: HashMap<String, u32>,
    text: Vec<String>,
}

#[derive(Clone, Copy)]
struct Invocation {
    entity: EntityId,
    emitted: u8,
}

pub(crate) struct EngineApi {
    pub(crate) world: Rc<RefCell<World>>,
    active: Cell<Option<Invocation>>,
    time: Cell<(f64, f64)>,
    resources: RefCell<Resources>,
    events: RefCell<HashMap<String, ScriptEvent>>,
    actions: RefCell<HashMap<EntityId, HashMap<String, Function>>>,
    #[cfg(feature = "editor")]
    staging: Cell<bool>,
}

impl EngineApi {
    pub(crate) fn new(world: Rc<RefCell<World>>) -> Self {
        Self {
            world,
            active: Cell::new(None),
            time: Cell::new((0.0, 0.0)),
            resources: RefCell::new(Resources::default()),
            events: RefCell::new(HashMap::new()),
            actions: RefCell::new(HashMap::new()),
            #[cfg(feature = "editor")]
            staging: Cell::new(false),
        }
    }

    pub(crate) fn register_text(&self, name: String, data: String) -> Result<u32> {
        let mut resources = self.resources.borrow_mut();
        if resources.names.contains_key(&name) {
            return Err(Error::runtime("resource name already registered"));
        }
        let handle = u32::try_from(resources.text.len())
            .map_err(|_| Error::runtime("resource IDs exhausted"))?;
        resources.names.insert(name, handle);
        resources.text.push(data);
        Ok(handle)
    }

    fn bind_action(&self, name: mlua::String, callback: Option<Function>) -> Result<()> {
        let entity = self.invocation()?.entity;
        if name.as_bytes().is_empty() || name.as_bytes().len() > MAX_EVENT_NAME {
            return Err(Error::runtime("invalid action name"));
        }
        let name = name.to_str()?;
        let mut actions = self.actions.borrow_mut();
        let bindings = actions.entry(entity).or_default();
        let name: &str = name.as_ref();
        if let Some(callback) = callback {
            if bindings.len() >= MAX_ACTIONS_PER_SCRIPT && !bindings.contains_key(name) {
                return Err(Error::runtime("script input binding limit reached"));
            }
            bindings.insert(name.to_owned(), callback);
        } else {
            bindings.remove(name);
        }
        Ok(())
    }

    pub(crate) fn action(&self, entity: EntityId, name: &str) -> Option<Function> {
        self.actions.borrow().get(&entity)?.get(name).cloned()
    }

    pub(crate) fn forget_actions(&self, entity: EntityId) {
        self.actions.borrow_mut().remove(&entity);
    }

    #[cfg(feature = "editor")]
    pub(crate) fn take_actions(&self, entity: EntityId) -> HashMap<String, Function> {
        self.actions
            .borrow_mut()
            .remove(&entity)
            .unwrap_or_default()
    }

    #[cfg(feature = "editor")]
    pub(crate) fn replace_actions(&self, entity: EntityId, bindings: HashMap<String, Function>) {
        self.actions.borrow_mut().insert(entity, bindings);
    }

    #[cfg(feature = "editor")]
    pub(crate) fn stage_reload(&self) -> ReloadScope<'_> {
        self.staging.set(true);
        ReloadScope(&self.staging)
    }

    pub(crate) fn begin_frame(&self, delta: f64, elapsed: f64, batch: &EventBatch) {
        self.time.set((delta, elapsed));
        let mut events = self.events.borrow_mut();
        events.clear();
        for event in batch.of_type::<ScriptEvent>() {
            events.insert(event.name.clone(), event.clone());
        }
    }

    pub(crate) fn enter(&self, entity: EntityId, emitted: u8) -> InvocationScope<'_> {
        self.active.set(Some(Invocation { entity, emitted }));
        InvocationScope(&self.active)
    }

    fn invocation(&self) -> Result<Invocation> {
        self.active
            .get()
            .ok_or_else(|| Error::runtime("no active script"))
    }

    fn position(&self) -> Result<[f32; 3]> {
        let entity = self.invocation()?.entity;
        self.world
            .borrow()
            .get_component::<Transform>(entity)
            .map(|transform| transform.position)
            .ok_or_else(|| Error::runtime("entity has no Transform"))
    }

    fn set_position(&self, position: [f32; 3]) -> Result<()> {
        if !position.iter().all(|value| value.is_finite()) {
            return Err(Error::runtime("position must contain finite numbers"));
        }
        let entity = self.invocation()?.entity;
        #[cfg(feature = "editor")]
        if self.staging.get() {
            return Ok(());
        }
        let mut world = self.world.borrow_mut();
        let transform = world
            .get_component_mut::<Transform>(entity)
            .ok_or_else(|| Error::runtime("entity has no Transform"))?;
        transform.position = position;
        Ok(())
    }

    fn emit(&self, name: mlua::String, payload: mlua::String) -> Result<()> {
        let mut invocation = self.invocation()?;
        if invocation.emitted >= MAX_EVENTS_PER_FRAME {
            return Err(Error::runtime("script event budget exhausted"));
        }
        if name.as_bytes().is_empty()
            || name.as_bytes().len() > MAX_EVENT_NAME
            || payload.as_bytes().len() > MAX_EVENT_PAYLOAD
        {
            return Err(Error::runtime("invalid script event size"));
        }
        #[cfg(feature = "editor")]
        if self.staging.get() {
            return Ok(());
        }

        let event = ScriptEvent {
            sender: invocation.entity,
            name: name.to_str()?.to_owned(),
            payload: payload.to_str()?.to_owned(),
        };
        self.world.borrow_mut().event_bus_mut().publish(event);
        invocation.emitted += 1;
        self.active.set(Some(invocation));
        Ok(())
    }
}

#[cfg(feature = "editor")]
pub(crate) struct ReloadScope<'a>(&'a Cell<bool>);

#[cfg(feature = "editor")]
impl Drop for ReloadScope<'_> {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

pub(crate) struct InvocationScope<'a>(&'a Cell<Option<Invocation>>);

impl InvocationScope<'_> {
    pub(crate) fn emitted(&self) -> u8 {
        self.0.get().map_or(0, |invocation| invocation.emitted)
    }
}

impl Drop for InvocationScope<'_> {
    fn drop(&mut self) {
        self.0.set(None);
    }
}

pub(crate) fn register(lua: &Lua, api: &Rc<EngineApi>) -> Result<()> {
    let globals = lua.globals();

    let entity = lua.create_table()?;
    let state = Rc::clone(api);
    entity.set(
        "id",
        lua.create_function(move |_, ()| Ok(state.invocation()?.entity.0.to_string()))?,
    )?;
    let state = Rc::clone(api);
    entity.set(
        "exists",
        lua.create_function(move |_, ()| {
            Ok(state.world.borrow().is_alive(state.invocation()?.entity))
        })?,
    )?;
    globals.set("entity", entity)?;

    let transform = lua.create_table()?;
    let state = Rc::clone(api);
    transform.set(
        "get_position",
        lua.create_function(move |_, ()| {
            let [x, y, z] = state.position()?;
            Ok((x, y, z))
        })?,
    )?;
    let state = Rc::clone(api);
    transform.set(
        "set_position",
        lua.create_function(move |_, (x, y, z): (f32, f32, f32)| state.set_position([x, y, z]))?,
    )?;
    globals.set("transform", transform)?;

    let components = lua.create_table()?;
    let state = Rc::clone(api);
    components.set(
        "has",
        lua.create_function(move |_, name: mlua::String| {
            let world = state.world.borrow();
            let owner = state.invocation()?.entity;
            Ok(match name.to_str()?.as_ref() {
                "Transform" => world.has_component::<Transform>(owner),
                "ScriptComponent" => world.has_component::<crate::ScriptComponent>(owner),
                _ => false,
            })
        })?,
    )?;
    globals.set("components", components)?;

    let resources = lua.create_table()?;
    let state = Rc::clone(api);
    resources.set(
        "find",
        lua.create_function(move |_, name: mlua::String| {
            Ok(state
                .resources
                .borrow()
                .names
                .get(name.to_str()?.as_ref())
                .copied())
        })?,
    )?;
    let state = Rc::clone(api);
    resources.set(
        "read_text",
        lua.create_function(move |lua, handle: u32| {
            state
                .resources
                .borrow()
                .text
                .get(handle as usize)
                .map(|text| lua.create_string(text))
                .transpose()
        })?,
    )?;
    globals.set("resources", resources)?;

    let events = lua.create_table()?;
    let state = Rc::clone(api);
    events.set(
        "emit",
        lua.create_function(move |_, (name, data)| state.emit(name, data))?,
    )?;
    let state = Rc::clone(api);
    events.set(
        "latest",
        lua.create_function(move |lua, name: mlua::String| {
            if let Some(event) = state.events.borrow().get(name.to_str()?.as_ref()) {
                return Ok((
                    Some(lua.create_string(&event.payload)?),
                    Some(event.sender.0.to_string()),
                ));
            }
            Ok((None, None))
        })?,
    )?;
    globals.set("events", events)?;

    let time = lua.create_table()?;
    let state = Rc::clone(api);
    time.set(
        "delta",
        lua.create_function(move |_, ()| Ok(state.time.get().0))?,
    )?;
    let state = Rc::clone(api);
    time.set(
        "elapsed",
        lua.create_function(move |_, ()| Ok(state.time.get().1))?,
    )?;
    globals.set("time", time)?;

    let input = lua.create_table()?;
    let state = Rc::clone(api);
    input.set(
        "bind_action",
        lua.create_function(move |_, (name, callback)| state.bind_action(name, callback))?,
    )?;
    let state = Rc::clone(api);
    input.set(
        "count",
        lua.create_function(move |_, ()| Ok(state.world.borrow().frame_input().len()))?,
    )?;
    let state = Rc::clone(api);
    input.set(
        "event",
        lua.create_function(move |lua, index: usize| {
            index
                .checked_sub(1)
                .and_then(|offset| state.world.borrow().frame_input().get(offset).copied())
                .map(|event| input_event(lua, event))
                .transpose()
        })?,
    )?;
    globals.set("input", input)?;

    Ok(())
}

fn input_event(lua: &Lua, event: RawInputEvent) -> Result<Table> {
    let data = lua.create_table()?;
    let kind = match event {
        RawInputEvent::KeyPressed { key } | RawInputEvent::KeyReleased { key } => {
            data.set("key", format!("{key:?}"))?;
            if matches!(event, RawInputEvent::KeyPressed { .. }) {
                "key_pressed"
            } else {
                "key_released"
            }
        }
        RawInputEvent::MousePressed { button } | RawInputEvent::MouseReleased { button } => {
            data.set("button", format!("{button:?}"))?;
            if matches!(event, RawInputEvent::MousePressed { .. }) {
                "mouse_pressed"
            } else {
                "mouse_released"
            }
        }
        RawInputEvent::MouseMoved { x, y } => {
            data.set("x", x)?;
            data.set("y", y)?;
            "mouse_moved"
        }
        RawInputEvent::MouseMotion { dx, dy } => {
            data.set("dx", dx)?;
            data.set("dy", dy)?;
            "mouse_motion"
        }
        RawInputEvent::MouseWheel { x, y, pixels } => {
            data.set("x", x)?;
            data.set("y", y)?;
            data.set("pixels", pixels)?;
            "mouse_wheel"
        }
        RawInputEvent::GamepadConnected { id } | RawInputEvent::GamepadDisconnected { id } => {
            data.set("id", id)?;
            if matches!(event, RawInputEvent::GamepadConnected { .. }) {
                "gamepad_connected"
            } else {
                "gamepad_disconnected"
            }
        }
        RawInputEvent::GamepadState {
            id,
            buttons,
            sticks,
            triggers,
        } => {
            data.set("id", id)?;
            data.set("buttons", buttons)?;
            data.set("sticks", lua.create_sequence_from(sticks)?)?;
            data.set("triggers", lua.create_sequence_from(triggers)?)?;
            "gamepad_state"
        }
        RawInputEvent::Touch { id, phase, x, y } => {
            data.set("id", id.to_string())?;
            data.set(
                "phase",
                match phase {
                    TouchPhase::Started => "started",
                    TouchPhase::Moved => "moved",
                    TouchPhase::Ended => "ended",
                    TouchPhase::Cancelled => "cancelled",
                },
            )?;
            data.set("x", x)?;
            data.set("y", y)?;
            "touch"
        }
        RawInputEvent::FocusChanged { focused } => {
            data.set("focused", focused)?;
            "focus_changed"
        }
        RawInputEvent::WindowResized { width, height } => {
            data.set("width", width)?;
            data.set("height", height)?;
            "window_resized"
        }
        RawInputEvent::WindowClosed => "window_closed",
    };
    data.set("kind", kind)?;
    Ok(data)
}
