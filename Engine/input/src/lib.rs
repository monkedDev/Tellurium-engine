//! Raw device state, action bindings and priority-ordered input contexts.

use std::collections::{BTreeMap, HashSet};

use serde::Deserialize;
use tellurium_platform::{KeyCode, MouseButton, RawInputEvent};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ActionValue {
    Digital(bool),
    Axis([f32; 2]),
    PassThrough([f64; 2]),
}

impl ActionValue {
    fn neutral(self) -> Self {
        match self {
            Self::Digital(_) => Self::Digital(false),
            Self::Axis(_) => Self::Axis([0.0; 2]),
            Self::PassThrough(_) => Self::PassThrough([0.0; 2]),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ActionEvent {
    pub name: String,
    pub value: ActionValue,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Button {
    Key(KeyCode),
    Mouse(MouseButton),
    Gamepad(u16),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GamepadAxis {
    LeftStick,
    RightStick,
    LeftTrigger,
    RightTrigger,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Pointer {
    Position,
    Motion,
    Wheel,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Button(Button),
    AxisPair {
        negative: Button,
        positive: Button,
    },
    Directions {
        left: Button,
        right: Button,
        down: Button,
        up: Button,
    },
    GamepadAxis(GamepadAxis),
    Pointer(Pointer),
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Control {
    Button(Button),
    GamepadAxis(GamepadAxis),
    Pointer(Pointer),
}

impl Source {
    fn controls(self) -> Vec<Control> {
        use Control::Button as InputButton;
        match self {
            Self::Button(button) => vec![InputButton(button)],
            Self::AxisPair { negative, positive } => {
                vec![InputButton(negative), InputButton(positive)]
            }
            Self::Directions {
                left,
                right,
                down,
                up,
            } => {
                vec![
                    InputButton(left),
                    InputButton(right),
                    InputButton(down),
                    InputButton(up),
                ]
            }
            Self::GamepadAxis(axis) => vec![Control::GamepadAxis(axis)],
            Self::Pointer(pointer) => vec![Control::Pointer(pointer)],
        }
    }

    fn sample(self, state: &DeviceState) -> Option<ActionValue> {
        let axis = |negative, positive| {
            f32::from(state.button(positive) as u8) - f32::from(state.button(negative) as u8)
        };
        match self {
            Self::Button(button) => Some(ActionValue::Digital(state.button(button))),
            Self::AxisPair { negative, positive } => {
                Some(ActionValue::Axis([axis(negative, positive), 0.0]))
            }
            Self::Directions {
                left,
                right,
                down,
                up,
            } => Some(ActionValue::Axis([axis(left, right), axis(down, up)])),
            Self::GamepadAxis(control) => Some(ActionValue::Axis(state.gamepad_axis(control))),
            Self::Pointer(_) => None,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    action: String,
    source: Source,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Context {
    pub name: String,
    pub priority: i32,
    #[serde(default = "enabled")]
    pub active: bool,
    bindings: Vec<Binding>,
}

fn enabled() -> bool {
    true
}

impl Context {
    pub fn new(name: impl Into<String>, priority: i32) -> Self {
        Self {
            name: name.into(),
            priority,
            active: true,
            bindings: Vec::new(),
        }
    }

    pub fn bind(&mut self, action: impl Into<String>, source: Source) {
        self.bindings.push(Binding {
            action: action.into(),
            source,
        });
    }
}

struct Pad {
    buttons: u16,
    sticks: [i16; 4],
    triggers: [u8; 2],
}

struct DeviceState {
    focused: bool,
    keys: HashSet<KeyCode>,
    mouse: HashSet<MouseButton>,
    pads: BTreeMap<u32, Pad>,
}

impl Default for DeviceState {
    fn default() -> Self {
        Self {
            focused: true,
            keys: HashSet::new(),
            mouse: HashSet::new(),
            pads: BTreeMap::new(),
        }
    }
}

impl DeviceState {
    fn update(&mut self, event: RawInputEvent) -> bool {
        match event {
            RawInputEvent::KeyPressed { key } => self.focused && self.keys.insert(key),
            RawInputEvent::KeyReleased { key } => self.keys.remove(&key),
            RawInputEvent::MousePressed { button } => self.focused && self.mouse.insert(button),
            RawInputEvent::MouseReleased { button } => self.mouse.remove(&button),
            RawInputEvent::GamepadState {
                id,
                buttons,
                sticks,
                triggers,
            } => {
                self.pads.insert(
                    id,
                    Pad {
                        buttons,
                        sticks,
                        triggers,
                    },
                );
                true
            }
            RawInputEvent::GamepadDisconnected { id } => self.pads.remove(&id).is_some(),
            RawInputEvent::FocusChanged { focused } => {
                self.focused = focused;
                if !focused {
                    self.keys.clear();
                    self.mouse.clear();
                }
                true
            }
            _ => false,
        }
    }

    fn button(&self, button: Button) -> bool {
        self.focused
            && match button {
                Button::Key(key) => self.keys.contains(&key),
                Button::Mouse(button) => self.mouse.contains(&button),
                Button::Gamepad(mask) => {
                    mask != 0 && self.pads.values().any(|pad| pad.buttons & mask == mask)
                }
            }
    }

    fn gamepad_axis(&self, axis: GamepadAxis) -> [f32; 2] {
        if !self.focused {
            return [0.0; 2];
        }
        let mut value = [0.0_f32; 2];
        for pad in self.pads.values() {
            let sample = match axis {
                GamepadAxis::LeftStick => stick(pad.sticks[0], pad.sticks[1]),
                GamepadAxis::RightStick => stick(pad.sticks[2], pad.sticks[3]),
                GamepadAxis::LeftTrigger => [f32::from(pad.triggers[0]) / 255.0, 0.0],
                GamepadAxis::RightTrigger => [f32::from(pad.triggers[1]) / 255.0, 0.0],
            };
            for (target, sample) in value.iter_mut().zip(sample) {
                if sample.abs() > target.abs() {
                    *target = sample;
                }
            }
        }
        value
    }
}

fn stick(x: i16, y: i16) -> [f32; 2] {
    let x = (f32::from(x) / 32767.0).clamp(-1.0, 1.0);
    let y = (f32::from(y) / 32767.0).clamp(-1.0, 1.0);
    let magnitude = x.hypot(y);
    const DEAD_ZONE: f32 = 0.16;
    if magnitude <= DEAD_ZONE {
        return [0.0; 2];
    }
    let scale = (magnitude.min(1.0) - DEAD_ZONE) / ((1.0 - DEAD_ZONE) * magnitude);
    [x * scale, y * scale]
}

#[derive(Default)]
pub struct ActionMap {
    contexts: Vec<Context>,
    devices: DeviceState,
    previous: BTreeMap<String, ActionValue>,
}

impl ActionMap {
    pub fn from_project_toml(source: &str) -> Result<Self, toml::de::Error> {
        #[derive(Default, Deserialize)]
        struct InputConfig {
            contexts: Vec<Context>,
        }
        #[derive(Deserialize)]
        struct ProjectConfig {
            #[serde(default)]
            input: InputConfig,
        }
        let project: ProjectConfig = toml::from_str(source)?;
        let mut map = Self::default();
        for context in project.input.contexts {
            map.add_context(context);
        }
        Ok(map)
    }

    pub fn add_context(&mut self, context: Context) {
        self.contexts
            .retain(|existing| existing.name != context.name);
        self.contexts.push(context);
        // Stable priority order: ties follow context registration order.
        self.contexts
            .sort_by_key(|context| std::cmp::Reverse(context.priority));
    }

    pub fn set_active(&mut self, name: &str, active: bool) -> bool {
        let Some(context) = self
            .contexts
            .iter_mut()
            .find(|context| context.name == name)
        else {
            return false;
        };
        context.active = active;
        true
    }

    pub fn toggle(&mut self, name: &str) -> Option<bool> {
        let context = self
            .contexts
            .iter_mut()
            .find(|context| context.name == name)?;
        context.active = !context.active;
        Some(context.active)
    }

    /// Reconcile held controls when contexts change and retain both edges of
    /// quick taps that arrive in the same frame. Repeated key-downs add no edge.
    pub fn update(&mut self, input: &[RawInputEvent]) -> Vec<ActionEvent> {
        let bindings = active_bindings(&self.contexts);
        let mut events = Vec::new();
        reconcile(&bindings, &self.devices, &mut self.previous, &mut events);
        for &event in input {
            if self.devices.update(event) {
                reconcile(&bindings, &self.devices, &mut self.previous, &mut events);
            }
            if self.devices.focused {
                forward_pointer(&bindings, event, &mut events);
            }
        }
        events
    }
}

fn active_bindings(contexts: &[Context]) -> Vec<(&str, Source)> {
    let mut blocked = HashSet::new();
    let mut result = Vec::new();
    for context in contexts.iter().filter(|context| context.active) {
        let mut consumed = HashSet::new();
        for Binding { action, source } in &context.bindings {
            let controls = source.controls();
            if controls.iter().all(|control| !blocked.contains(control)) {
                result.push((action.as_str(), *source));
                consumed.extend(controls);
            }
        }
        blocked.extend(consumed);
    }
    result
}

fn reconcile(
    bindings: &[(&str, Source)],
    devices: &DeviceState,
    previous: &mut BTreeMap<String, ActionValue>,
    events: &mut Vec<ActionEvent>,
) {
    let mut current = BTreeMap::<&str, ActionValue>::new();
    for &(name, source) in bindings {
        if let Some(value) = source.sample(devices) {
            current
                .entry(name)
                .and_modify(|existing| match (existing, value) {
                    (ActionValue::Digital(a), ActionValue::Digital(b)) => *a |= b,
                    (ActionValue::Axis(a), ActionValue::Axis(b)) => {
                        for (a, b) in a.iter_mut().zip(b) {
                            *a = (*a + b).clamp(-1.0, 1.0);
                        }
                    }
                    _ => {}
                })
                .or_insert(value);
        }
    }
    let old = std::mem::take(previous);
    for (name, value) in &old {
        if !current.contains_key(name.as_str()) {
            events.push(ActionEvent {
                name: name.clone(),
                value: value.neutral(),
            });
        }
    }
    for (name, value) in current {
        let last = old.get(name).copied().unwrap_or_else(|| value.neutral());
        if value != last {
            events.push(ActionEvent {
                name: name.to_owned(),
                value,
            });
        }
        if value != value.neutral() {
            previous.insert(name.to_owned(), value);
        }
    }
}

fn forward_pointer(
    bindings: &[(&str, Source)],
    event: RawInputEvent,
    events: &mut Vec<ActionEvent>,
) {
    let (pointer, axes) = match event {
        RawInputEvent::MouseMoved { x, y } => (Pointer::Position, [x, y]),
        RawInputEvent::MouseMotion { dx, dy } => (Pointer::Motion, [dx, dy]),
        RawInputEvent::MouseWheel { x, y, .. } => (Pointer::Wheel, [x, y]),
        _ => return,
    };
    for &(name, source) in bindings {
        if matches!(source, Source::Pointer(bound) if bound == pointer) {
            events.push(ActionEvent {
                name: name.to_owned(),
                value: ActionValue::PassThrough(axes),
            });
        }
    }
}
