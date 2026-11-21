//! Window lifecycle, raw input and OS services.

mod gamepad;
pub use gamepad::Gamepads;

pub mod fs {
    pub use std::fs::{create_dir_all, metadata, read, read_dir, read_to_string, write, File};
}

pub mod threads {
    pub use std::thread::{available_parallelism, scope, Builder, JoinHandle};
}

use std::sync::Arc;
use std::time::{Duration, Instant};

use winit::event::{
    DeviceEvent, ElementState, Event, MouseButton as WinitMouseButton, MouseScrollDelta,
    TouchPhase as WinitTouchPhase, WindowEvent,
};
use winit::event_loop::{ControlFlow, EventLoop};
use winit::keyboard::PhysicalKey;
use winit::window::Window;

use winit::window::WindowAttributes;

/// Re-export key event types for consumers.
pub use winit::keyboard::KeyCode as WinitKeyCode;

/// A single raw input event captured from the OS window loop.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RawInputEvent {
    KeyPressed {
        key: KeyCode,
    },
    KeyReleased {
        key: KeyCode,
    },
    MouseMoved {
        x: f64,
        y: f64,
    },
    MousePressed {
        button: MouseButton,
    },
    MouseReleased {
        button: MouseButton,
    },
    MouseWheel {
        x: f64,
        y: f64,
        pixels: bool,
    },
    MouseMotion {
        dx: f64,
        dy: f64,
    },
    GamepadConnected {
        id: u32,
    },
    GamepadDisconnected {
        id: u32,
    },
    GamepadState {
        id: u32,
        buttons: u16,
        sticks: [i16; 4],
        triggers: [u8; 2],
    },
    Touch {
        id: u64,
        phase: TouchPhase,
        x: f64,
        y: f64,
    },
    FocusChanged {
        focused: bool,
    },
    WindowResized {
        width: u32,
        height: u32,
    },
    WindowClosed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TouchPhase {
    Started,
    Moved,
    Ended,
    Cancelled,
}

/// Tellurium keyboard key codes, mapped from physical key positions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Deserialize)]
pub enum KeyCode {
    Escape,
    Tab,
    Return,
    Space,
    Back,
    Delete,
    Insert,
    Home,
    End,
    PageUp,
    PageDown,
    Up,
    Down,
    Left,
    Right,
    Key1,
    Key2,
    Key3,
    Key4,
    Key5,
    Key6,
    Key7,
    Key8,
    Key9,
    Key0,
    A,
    B,
    C,
    D,
    E,
    F,
    G,
    H,
    I,
    J,
    K,
    L,
    M,
    N,
    O,
    P,
    Q,
    R,
    S,
    T,
    U,
    V,
    W,
    X,
    Y,
    Z,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
    LControl,
    RControl,
    LShift,
    RShift,
    LAlt,
    RAlt,
    LWin,
    RWin,
    Apostrophe,
    Semicolon,
    Equal,
    Comma,
    Minus,
    Period,
    Slash,
    Backslash,
    GraveAccent,
    LeftBracket,
    RightBracket,
    #[serde(skip)]
    Other(WinitKeyCode),
    Unknown,
}

/// Mouse button codes (Tellurium's own, distinct from winit's).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Deserialize)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
    Other(u16),
}

/// Buffer of raw input events accumulated between frames.
///
/// The platform layer pushes OS events here during the window event loop.
/// The input/core layer drains this buffer during ECS update so that
/// input is decoupled from the raw OS polling rate.
#[derive(Default)]
pub struct RawInputState {
    events: Vec<RawInputEvent>,
}

impl RawInputState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Push a raw event into the buffer (called from the window loop).
    pub fn push(&mut self, event: RawInputEvent) {
        self.events.push(event);
    }

    /// Drain all pending events into a Vec, clearing the buffer.
    /// Called once per frame by the input/core layer.
    pub fn drain(&mut self) -> Vec<RawInputEvent> {
        std::mem::take(&mut self.events)
    }

    /// Number of pending events.
    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}

/// Simple frame timer providing delta time and total elapsed time.
#[derive(Debug, Default)]
pub struct FrameTimer {
    last_instant: Option<Instant>,
    start_time: Option<Instant>,
}

impl FrameTimer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Call once at the beginning of each frame. Returns delta seconds
    /// since the previous call.
    pub fn tick(&mut self) -> f64 {
        let now = Instant::now();
        let delta = if let Some(last) = self.last_instant {
            now.duration_since(last).as_secs_f64()
        } else {
            0.0
        };
        if self.start_time.is_none() {
            self.start_time = Some(now);
        }
        self.last_instant = Some(now);
        delta
    }

    /// Total elapsed seconds since the first `tick()`.
    pub fn elapsed(&self) -> f64 {
        self.start_time
            .map(|start| start.elapsed().as_secs_f64())
            .unwrap_or(0.0)
    }
}

/// Platform context: window, input buffer, timer, event loop.
pub struct Platform {
    pub window: Arc<Window>,
    pub input: RawInputState,
    pub timer: FrameTimer,
    event_loop: EventLoop<()>,
}

impl Platform {
    /// Create a new OS window and platform instance.
    pub fn new() -> Result<Self, PlatformError> {
        let event_loop = EventLoop::new().map_err(PlatformError::EventLoop)?;
        let mut attributes = WindowAttributes::default();
        attributes.title = "Tellurium Engine".to_string();
        attributes.inner_size = Some(winit::dpi::LogicalSize::new(1280.0, 720.0).into());

        #[allow(deprecated)]
        let window = event_loop
            .create_window(attributes)
            .map_err(|e| PlatformError::Window(winit::error::ExternalError::Os(e)))?;

        log::info!(
            "Platform: OS window created {}x{}",
            window.inner_size().width,
            window.inner_size().height
        );

        Ok(Self {
            window: Arc::new(window),
            input: RawInputState::new(),
            timer: FrameTimer::new(),
            event_loop,
        })
    }

    /// Run the platform event loop with a per-frame callback.
    ///
    /// The callback `on_frame` is invoked once per frame from within
    /// `AboutToWait`. It receives mutable access to the input buffer and
    /// timer, and an immutable reference to the window. Return `false` from
    /// the callback to stop the event loop.
    pub fn run<F>(self, mut on_frame: F) -> Result<(), PlatformError>
    where
        F: FnMut(&mut PlatformFrameContext) -> bool,
    {
        let Platform {
            window,
            mut input,
            mut timer,
            event_loop,
        } = self;

        let mut ctx = PlatformFrameContext {
            window: &window,
            input: &mut input,
            timer: &mut timer,
        };

        let frame_interval = Duration::from_secs_f64(1.0 / 60.0);
        let mut next_frame = Instant::now();
        let mut gamepads = gamepad::Gamepads::default();
        let mut closing = false;

        #[allow(deprecated)]
        event_loop
            .run(move |event, elwt| match event {
                Event::WindowEvent { event, .. } => match event {
                    WindowEvent::CloseRequested => {
                        closing = true;
                        ctx.input.push(RawInputEvent::WindowClosed);
                    }
                    WindowEvent::KeyboardInput {
                        event: key_event, ..
                    } => {
                        let key = map_key(key_event.physical_key);
                        match key_event.state {
                            ElementState::Pressed => {
                                ctx.input.push(RawInputEvent::KeyPressed { key });
                            }
                            ElementState::Released => {
                                ctx.input.push(RawInputEvent::KeyReleased { key });
                            }
                        }
                    }
                    WindowEvent::CursorMoved { position, .. } => {
                        ctx.input.push(RawInputEvent::MouseMoved {
                            x: position.x,
                            y: position.y,
                        });
                    }
                    WindowEvent::MouseInput { state, button, .. } => {
                        let btn = map_mouse_button(button);
                        match state {
                            ElementState::Pressed => {
                                ctx.input.push(RawInputEvent::MousePressed { button: btn });
                            }
                            ElementState::Released => {
                                ctx.input.push(RawInputEvent::MouseReleased { button: btn });
                            }
                        }
                    }
                    WindowEvent::MouseWheel { delta, .. } => {
                        let (x, y, pixels) = match delta {
                            MouseScrollDelta::LineDelta(x, y) => {
                                (f64::from(x), f64::from(y), false)
                            }
                            MouseScrollDelta::PixelDelta(position) => {
                                (position.x, position.y, true)
                            }
                        };
                        ctx.input.push(RawInputEvent::MouseWheel { x, y, pixels });
                    }
                    WindowEvent::Focused(focused) => {
                        ctx.input.push(RawInputEvent::FocusChanged { focused })
                    }
                    WindowEvent::Touch(touch) => ctx.input.push(RawInputEvent::Touch {
                        id: touch.id,
                        phase: match touch.phase {
                            WinitTouchPhase::Started => TouchPhase::Started,
                            WinitTouchPhase::Moved => TouchPhase::Moved,
                            WinitTouchPhase::Ended => TouchPhase::Ended,
                            WinitTouchPhase::Cancelled => TouchPhase::Cancelled,
                        },
                        x: touch.location.x,
                        y: touch.location.y,
                    }),
                    WindowEvent::Resized(size) => {
                        ctx.input.push(RawInputEvent::WindowResized {
                            width: size.width,
                            height: size.height,
                        });
                    }
                    _ => {}
                },
                Event::DeviceEvent {
                    event: DeviceEvent::MouseMotion { delta: (dx, dy) },
                    ..
                } => {
                    ctx.input.push(RawInputEvent::MouseMotion { dx, dy });
                }
                Event::AboutToWait => {
                    let now = Instant::now();
                    if closing || now >= next_frame {
                        gamepads.poll(ctx.input);
                        if !on_frame(&mut ctx) || closing {
                            elwt.exit();
                        }
                        next_frame = now + frame_interval;
                    }
                    elwt.set_control_flow(ControlFlow::WaitUntil(next_frame));
                }
                _ => {}
            })
            .map_err(PlatformError::EventLoop)
    }

    /// Request a redraw / present. Called at end of frame.
    pub fn request_redraw(&self) {
        self.window.request_redraw();
    }

    /// Get window inner size.
    pub fn inner_size(&self) -> (u32, u32) {
        let size = self.window.inner_size();
        (size.width, size.height)
    }

    /// Inject a raw input event (for testing or external event pump wiring).
    pub fn inject_event(&mut self, event: RawInputEvent) {
        self.input.push(event);
    }
}

impl Default for Platform {
    fn default() -> Self {
        Self::new().expect("failed to create platform")
    }
}

/// Context passed to the per-frame callback during `Platform::run`.
pub struct PlatformFrameContext<'a> {
    pub window: &'a Window,
    pub input: &'a mut RawInputState,
    pub timer: &'a mut FrameTimer,
}

fn map_key(key: PhysicalKey) -> KeyCode {
    let PhysicalKey::Code(code) = key else {
        return KeyCode::Unknown;
    };
    match code {
        WinitKeyCode::Enter => KeyCode::Return,
        WinitKeyCode::Backspace => KeyCode::Back,
        WinitKeyCode::ArrowUp => KeyCode::Up,
        WinitKeyCode::ArrowDown => KeyCode::Down,
        WinitKeyCode::ArrowLeft => KeyCode::Left,
        WinitKeyCode::ArrowRight => KeyCode::Right,
        WinitKeyCode::ControlLeft => KeyCode::LControl,
        WinitKeyCode::ControlRight => KeyCode::RControl,
        WinitKeyCode::ShiftLeft => KeyCode::LShift,
        WinitKeyCode::ShiftRight => KeyCode::RShift,
        WinitKeyCode::AltLeft => KeyCode::LAlt,
        WinitKeyCode::AltRight => KeyCode::RAlt,
        WinitKeyCode::SuperLeft => KeyCode::LWin,
        WinitKeyCode::SuperRight => KeyCode::RWin,
        WinitKeyCode::Quote => KeyCode::Apostrophe,
        WinitKeyCode::Backquote => KeyCode::GraveAccent,
        WinitKeyCode::BracketLeft => KeyCode::LeftBracket,
        WinitKeyCode::BracketRight => KeyCode::RightBracket,
        WinitKeyCode::KeyA => KeyCode::A,
        WinitKeyCode::KeyB => KeyCode::B,
        WinitKeyCode::KeyC => KeyCode::C,
        WinitKeyCode::KeyD => KeyCode::D,
        WinitKeyCode::KeyE => KeyCode::E,
        WinitKeyCode::KeyF => KeyCode::F,
        WinitKeyCode::KeyG => KeyCode::G,
        WinitKeyCode::KeyH => KeyCode::H,
        WinitKeyCode::KeyI => KeyCode::I,
        WinitKeyCode::KeyJ => KeyCode::J,
        WinitKeyCode::KeyK => KeyCode::K,
        WinitKeyCode::KeyL => KeyCode::L,
        WinitKeyCode::KeyM => KeyCode::M,
        WinitKeyCode::KeyN => KeyCode::N,
        WinitKeyCode::KeyO => KeyCode::O,
        WinitKeyCode::KeyP => KeyCode::P,
        WinitKeyCode::KeyQ => KeyCode::Q,
        WinitKeyCode::KeyR => KeyCode::R,
        WinitKeyCode::KeyS => KeyCode::S,
        WinitKeyCode::KeyT => KeyCode::T,
        WinitKeyCode::KeyU => KeyCode::U,
        WinitKeyCode::KeyV => KeyCode::V,
        WinitKeyCode::KeyW => KeyCode::W,
        WinitKeyCode::KeyX => KeyCode::X,
        WinitKeyCode::KeyY => KeyCode::Y,
        WinitKeyCode::KeyZ => KeyCode::Z,
        WinitKeyCode::Digit0 => KeyCode::Key0,
        WinitKeyCode::Digit1 => KeyCode::Key1,
        WinitKeyCode::Digit2 => KeyCode::Key2,
        WinitKeyCode::Digit3 => KeyCode::Key3,
        WinitKeyCode::Digit4 => KeyCode::Key4,
        WinitKeyCode::Digit5 => KeyCode::Key5,
        WinitKeyCode::Digit6 => KeyCode::Key6,
        WinitKeyCode::Digit7 => KeyCode::Key7,
        WinitKeyCode::Digit8 => KeyCode::Key8,
        WinitKeyCode::Digit9 => KeyCode::Key9,
        WinitKeyCode::Escape => KeyCode::Escape,
        WinitKeyCode::Tab => KeyCode::Tab,
        WinitKeyCode::Space => KeyCode::Space,
        WinitKeyCode::Delete => KeyCode::Delete,
        WinitKeyCode::Insert => KeyCode::Insert,
        WinitKeyCode::Home => KeyCode::Home,
        WinitKeyCode::End => KeyCode::End,
        WinitKeyCode::PageUp => KeyCode::PageUp,
        WinitKeyCode::PageDown => KeyCode::PageDown,
        WinitKeyCode::Semicolon => KeyCode::Semicolon,
        WinitKeyCode::Equal => KeyCode::Equal,
        WinitKeyCode::Comma => KeyCode::Comma,
        WinitKeyCode::Minus => KeyCode::Minus,
        WinitKeyCode::Period => KeyCode::Period,
        WinitKeyCode::Slash => KeyCode::Slash,
        WinitKeyCode::Backslash => KeyCode::Backslash,
        WinitKeyCode::F1 => KeyCode::F1,
        WinitKeyCode::F2 => KeyCode::F2,
        WinitKeyCode::F3 => KeyCode::F3,
        WinitKeyCode::F4 => KeyCode::F4,
        WinitKeyCode::F5 => KeyCode::F5,
        WinitKeyCode::F6 => KeyCode::F6,
        WinitKeyCode::F7 => KeyCode::F7,
        WinitKeyCode::F8 => KeyCode::F8,
        WinitKeyCode::F9 => KeyCode::F9,
        WinitKeyCode::F10 => KeyCode::F10,
        WinitKeyCode::F11 => KeyCode::F11,
        WinitKeyCode::F12 => KeyCode::F12,
        other => KeyCode::Other(other),
    }
}

/// Map winit `MouseButton` to our `MouseButton`.
fn map_mouse_button(button: WinitMouseButton) -> MouseButton {
    match button {
        WinitMouseButton::Left => MouseButton::Left,
        WinitMouseButton::Right => MouseButton::Right,
        WinitMouseButton::Middle => MouseButton::Middle,
        WinitMouseButton::Back => MouseButton::Back,
        WinitMouseButton::Forward => MouseButton::Forward,
        WinitMouseButton::Other(id) => MouseButton::Other(id),
    }
}

/// Platform creation errors.
#[derive(Debug)]
pub enum PlatformError {
    EventLoop(winit::error::EventLoopError),
    Window(winit::error::ExternalError),
}

impl std::fmt::Display for PlatformError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlatformError::EventLoop(e) => write!(f, "event loop error: {e}"),
            PlatformError::Window(e) => write!(f, "window error: {e}"),
        }
    }
}

impl std::error::Error for PlatformError {}
