//! Windows editor host. Game View embeds the same DX12 swap chain used by the runtime.

use std::collections::HashMap;
use std::io;
use std::num::NonZeroIsize;
use std::path::PathBuf;
use std::ptr;
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Instant;

use raw_window_handle::{
    HandleError, HasWindowHandle, RawWindowHandle, Win32WindowHandle, WindowHandle,
};
use tellurium_assets::{Asset, Assets, AudioClip, Handle, Mesh, ScriptAsset, Texture};
use tellurium_core::EventBus;
use tellurium_diagnostics::{FrameMetrics, TraceStage, TraceZone};
use tellurium_editor::{eframe, egui, EditorEvent, EditorResult, GameHost, ResourceKind};
use tellurium_platform::{Gamepads, KeyCode, MouseButton, RawInputEvent, RawInputState};
use tellurium_scripting::EditorPlugin;
use tellurium_serialization::{
    ComponentRegistry, DocumentFormat, PackedScene, Project, WorldSnapshot,
};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, GetWindowLongPtrW, SetWindowLongPtrW, SetWindowPos, ShowWindow,
    GWL_STYLE, SWP_NOACTIVATE, SWP_NOZORDER, SWP_SHOWWINDOW, SW_HIDE, WS_CHILD, WS_CLIPCHILDREN,
    WS_CLIPSIBLINGS, WS_DISABLED,
};

use crate::editor_watch::{self, ProjectWatch};
use crate::scene::{self, PreparedScene};
use crate::{Runtime, SceneRenderer, FIXED_DELTA};

pub fn run(project: Option<PathBuf>) -> eframe::Result {
    tellurium_editor::run(Host::default(), project)
}

#[derive(Default)]
struct Host {
    watch: Option<ProjectWatch>,
    plugin_bus: EventBus,
    plugins: Vec<(String, EditorPlugin)>,
    pending: Option<Receiver<Result<Installation, String>>>,
    game: Option<Game>,
    surface: Option<Rc<Surface>>,
    registry: ComponentRegistry,
    import: Option<ImportJob>,
    preview_root: Option<PathBuf>,
    meshes: MeshPreviews,
    next_import: Option<(PathBuf, Vec<String>)>,
}

struct Installation {
    project: Project,
    scene: PreparedScene,
    clock: Option<WorldSnapshot>,
}

pub(crate) struct Game {
    pub(crate) renderer: SceneRenderer<Rc<Surface>>,
    pub(crate) runtime: Runtime,
    elapsed: f64,
    last_frame: Instant,
    paused: bool,
    input: GameInput,
    draw_calls: u32,
    fps: f64,
}

impl Drop for Game {
    fn drop(&mut self) {
        self.runtime.shutdown();
    }
}

pub(crate) struct Surface {
    handle: NonZeroIsize,
    placement: std::cell::Cell<Option<[i32; 4]>>,
}

impl Surface {
    fn new(parent: &impl HasWindowHandle) -> EditorResult<Rc<Self>> {
        let RawWindowHandle::Win32(parent) = parent.window_handle()?.as_raw() else {
            return Err("Game View requires a Windows window".into());
        };
        let class: Vec<u16> = "STATIC".encode_utf16().chain(Some(0)).collect();
        let parent = parent.hwnd.get() as HWND;
        // SAFETY: The parent is owned by eframe on this thread. Windows owns the
        // predefined class and does not retain the supplied class-name pointer.
        let hwnd = unsafe {
            let style = GetWindowLongPtrW(parent, GWL_STYLE);
            SetWindowLongPtrW(parent, GWL_STYLE, style | WS_CLIPCHILDREN as isize);
            CreateWindowExW(
                0,
                class.as_ptr(),
                ptr::null(),
                WS_CHILD | WS_CLIPSIBLINGS | WS_DISABLED,
                0,
                0,
                1,
                1,
                parent,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null(),
            )
        };
        let handle = NonZeroIsize::new(hwnd as isize).ok_or_else(io::Error::last_os_error)?;
        Ok(Rc::new(Self {
            handle,
            placement: std::cell::Cell::new(None),
        }))
    }

    fn place(&self, view: Option<&egui::Response>) -> EditorResult<Option<(u32, u32)>> {
        let rect = view
            .map(|view| {
                let ppp = view.ctx.pixels_per_point();
                let x = (view.rect.min.x * ppp).round() as i32;
                let y = (view.rect.min.y * ppp).round() as i32;
                let width = (view.rect.max.x * ppp).round() as i32 - x;
                let height = (view.rect.max.y * ppp).round() as i32 - y;
                [x, y, width, height]
            })
            .filter(|rect| rect[2] > 0 && rect[3] > 0);
        if self.placement.get() != rect {
            if let Some([x, y, width, height]) = rect {
                // SAFETY: This surface owns the HWND; viewport coordinates are client pixels.
                if unsafe {
                    SetWindowPos(
                        self.handle.get() as HWND,
                        ptr::null_mut(),
                        x,
                        y,
                        width,
                        height,
                        SWP_NOACTIVATE | SWP_NOZORDER | SWP_SHOWWINDOW,
                    )
                } == 0
                {
                    return Err(io::Error::last_os_error().into());
                }
            } else {
                // SAFETY: The HWND belongs to this surface.
                unsafe {
                    ShowWindow(self.handle.get() as HWND, SW_HIDE);
                }
            }
            self.placement.set(rect);
        }
        Ok(rect.map(|[_, _, width, height]| (width as u32, height as u32)))
    }
}

impl HasWindowHandle for Surface {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let handle = RawWindowHandle::Win32(Win32WindowHandle::new(self.handle));
        // SAFETY: The HWND remains valid for the lifetime of the borrowed surface.
        Ok(unsafe { WindowHandle::borrow_raw(handle) })
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        // SAFETY: Creation and destruction occur on the editor's window thread.
        unsafe {
            DestroyWindow(self.handle.get() as HWND);
        }
    }
}

impl Host {
    fn queue(
        &mut self,
        project: Project,
        scene: PackedScene,
        clock: Option<WorldSnapshot>,
    ) -> EditorResult<()> {
        scene.order()?;
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("tellurium-load-scene".into())
            .spawn(move || {
                let result = PreparedScene::load(&project, scene)
                    .map(|scene| Installation {
                        project,
                        scene,
                        clock,
                    })
                    .map_err(|error| error.to_string());
                let _ = tx.send(result);
            })?;
        self.pending = Some(rx);
        Ok(())
    }

    fn poll(&mut self, frame: &eframe::Frame) -> EditorResult<()> {
        let Some(pending) = &self.pending else {
            return Ok(());
        };
        let job = match pending.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return Ok(()),
            Err(TryRecvError::Disconnected) => Err("Scene loader stopped".into()),
        };
        self.pending = None;
        let job = job?;
        // DXGI permits only one flip-model swap chain for a given HWND at a time.
        // Create a new surface so the previous game stays intact until installation succeeds.
        let surface = Surface::new(frame)?;
        let mut renderer = SceneRenderer::new(Rc::clone(&surface), (1, 1))?;
        renderer.set_clear_color(tellurium_editor::game_view_clear_color());
        let mut runtime = job
            .scene
            .install(&job.project, &mut renderer, &self.registry)?;
        let elapsed = if let Some(clock) = &job.clock {
            runtime.restore_clock(clock);
            clock.elapsed
        } else {
            0.0
        };
        let mut game = Game {
            renderer,
            runtime,
            elapsed,
            last_frame: Instant::now(),
            paused: false,
            input: GameInput::default(),
            draw_calls: 0,
            fps: 0.0,
        };
        if let Some(watch) = &self.watch {
            if let Err(error) = watch.initial_shader(&mut game) {
                log::warn!("Shader reload: {error}");
            }
        }
        self.game = Some(game);
        self.surface = Some(surface);
        log::info!("Game View loaded {}", job.project.config.name);
        Ok(())
    }

    fn service_import(&mut self) {
        if let Some(job) = &self.import {
            match job.result.try_recv() {
                Ok(meshes) => {
                    if self.preview_root.as_ref() == Some(&job.root) {
                        self.meshes = meshes;
                    }
                    self.import = None;
                }
                Err(TryRecvError::Disconnected) => {
                    self.import = None;
                }
                Err(TryRecvError::Empty) => return,
            }
        }
        if let Some((root, paths)) = self.next_import.take() {
            let (done, result) = mpsc::channel();
            let job_root = root.clone();
            match std::thread::Builder::new()
                .name("tellurium-project-import".into())
                .spawn(move || {
                    let previews = import_paths(&root, paths);
                    let _ = done.send(previews);
                }) {
                Ok(_) => {
                    self.import = Some(ImportJob {
                        root: job_root,
                        result,
                    });
                }
                Err(error) => log::warn!("Could not start project import: {error}"),
            }
        }
    }
}

impl GameHost for Host {
    fn open_project(&mut self, project: &Project) {
        self.watch = match ProjectWatch::new(project) {
            Ok(watch) => Some(watch),
            Err(error) => {
                log::warn!("Cannot watch project files: {error}");
                None
            }
        };
        self.plugins = load_plugins(project);
    }

    fn close_project(&mut self) {
        self.watch = None;
        self.plugins.clear();
        self.plugin_bus = EventBus::new();
    }

    fn reload(&mut self) -> Vec<EditorEvent> {
        let _zone = TraceZone::new(TraceStage::HotReload);
        self.watch.as_mut().map_or_else(Vec::new, |watch| {
            watch.poll(self.game.as_mut(), self.pending.is_some())
        })
    }

    fn editor_event(&mut self, event: EditorEvent) {
        self.plugin_bus.publish(event);
    }

    fn metrics(&self) -> Option<FrameMetrics> {
        self.game.as_ref().map(|game| FrameMetrics {
            fps: game.fps,
            draw_calls: game.draw_calls,
            ..game.runtime.metrics()
        })
    }

    fn script_globals(&self, entity: tellurium_core::EntityId) -> Vec<(String, String)> {
        self.game.as_ref().map_or_else(Vec::new, |game| {
            game.runtime.scripting.inspect_globals(entity)
        })
    }

    fn export(
        &mut self,
        project: Project,
        output: PathBuf,
    ) -> EditorResult<Receiver<io::Result<PathBuf>>> {
        let (sender, result) = mpsc::channel();
        std::thread::Builder::new()
            .name("tellurium-export".into())
            .spawn(move || {
                let _ = sender.send(tellurium_builder::export_windows(&project, &output));
            })?;
        Ok(result)
    }

    fn play(&mut self, project: &Project, scene: PackedScene) -> EditorResult<()> {
        self.queue(project.clone(), scene, None)
    }

    fn stop(&mut self) {
        self.pending = None;
        self.game = None;
        self.surface = None;
    }

    fn frame(
        &mut self,
        frame: &eframe::Frame,
        viewport: Option<&egui::Response>,
        paused: bool,
        step: bool,
    ) -> EditorResult<()> {
        self.service_import();
        self.dispatch_plugins();
        if let Err(error) = self.poll(frame) {
            if self.game.is_none() {
                return Err(error);
            }
            log::warn!("Game View kept the previous scene: {error}");
        }
        let Some((game, surface)) = self.game.as_mut().zip(self.surface.as_ref()) else {
            return Ok(());
        };
        let size = surface.place(viewport)?;
        let leaving_play = paused && !game.paused;
        if leaving_play {
            game.runtime.finish_physics()?;
        }
        if game.paused != paused {
            if let Some(audio) = &mut game.runtime.audio {
                audio.set_paused(paused)?;
            }
            game.paused = paused;
        }
        let now = Instant::now();
        let interval = now.duration_since(game.last_frame).as_secs_f64();
        game.fps = if interval > 0.0 { 1.0 / interval } else { 0.0 };
        let delta = interval.min(0.1);
        game.last_frame = now;
        game.input.collect(viewport.filter(|_| !paused || step));
        let renderer = &mut game.renderer;
        let mut render = |world: &tellurium_core::World| {
            if let Some(size) = size {
                scene::camera(world, renderer);
                renderer.render(world, size, || {})?;
            }
            Ok::<_, tellurium_bridge::RenderError>(())
        };
        if !paused || step {
            let delta = if step { FIXED_DELTA } else { delta };
            game.elapsed += delta;
            let input = game.input.events.drain();
            game.runtime.frame(input, delta, game.elapsed, render)??;
            if step {
                game.runtime.finish_physics()?;
            }
        } else {
            render(&game.runtime.world().borrow())?;
        }
        game.draw_calls = size.map_or(0, |_| game.renderer.draw_calls());
        Ok(())
    }

    fn save(&mut self, path: PathBuf, format: DocumentFormat) -> EditorResult<()> {
        let game = self
            .game
            .as_mut()
            .ok_or("Wait for Game View to finish loading")?;
        game.runtime
            .snapshot(game.elapsed, &self.registry)?
            .save(&path.with_extension("telsave"), format)?;
        Ok(())
    }

    fn load(&mut self, project: &Project, path: PathBuf) -> EditorResult<()> {
        let snapshot = WorldSnapshot::load(&path)?;
        self.queue(project.clone(), snapshot.scene.clone(), Some(snapshot))
    }

    fn import(&mut self, project: &Project, paths: Vec<String>) {
        let root = project.root().to_path_buf();
        if self.preview_root.as_ref() != Some(&root) {
            self.meshes.clear();
            self.preview_root = Some(root.clone());
        }
        self.next_import = Some((root, paths));
        self.service_import();
    }
    fn preview_mesh(&self, path: &str) -> Option<&Mesh> {
        self.meshes.get(path).map(std::ops::Deref::deref)
    }
}

impl Host {
    fn dispatch_plugins(&mut self) {
        let batch = self.plugin_bus.dispatch();
        let mut remaining = 32;
        for event in batch.of_type::<EditorEvent>() {
            let (kind, payload) = match event {
                EditorEvent::SceneOpened(path) => ("SceneOpened", path.clone()),
                EditorEvent::SceneChanged(path) => ("SceneChanged", path.clone()),
                EditorEvent::EntitySelected(id) => (
                    "EntitySelected",
                    id.map_or_else(String::new, |id| id.to_string()),
                ),
                EditorEvent::ResourceReloaded(kind, path) => (
                    match kind {
                        ResourceKind::Scene => "SceneReloaded",
                        ResourceKind::Script => "ScriptReloaded",
                        ResourceKind::Texture => "TextureReloaded",
                        ResourceKind::Shader => "ShaderReloaded",
                    },
                    path.clone(),
                ),
            };
            for (name, plugin) in &self.plugins {
                if remaining == 0 {
                    log::warn!("Editor plugin event batch exceeded the frame budget");
                    return;
                }
                remaining -= 1;
                match plugin.dispatch(kind, &payload) {
                    Ok(Some(message)) => log::info!("[{name}] {message}"),
                    Ok(None) => {}
                    Err(error) => log::warn!("Editor plugin {name}: {error}"),
                }
            }
        }
    }
}

fn load_plugins(project: &Project) -> Vec<(String, EditorPlugin)> {
    let path = match project.path("plugins") {
        Ok(path) => path,
        Err(error) => {
            log::warn!("Editor plugin folder: {error}");
            return Vec::new();
        }
    };
    let files = match std::fs::read_dir(path) {
        Ok(files) => files,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Vec::new(),
        Err(error) => {
            log::warn!("Editor plugins: {error}");
            return Vec::new();
        }
    };
    let mut names: Vec<_> = files
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("luau"))
        })
        .map(|entry| entry.file_name())
        .collect();
    names.sort();
    if names.len() > 8 {
        log::warn!("Only the first eight editor plugins will load for this project");
    }
    names
        .into_iter()
        .take(8)
        .filter_map(|name| {
            let name = name.to_str()?.to_owned();
            let result = (|| -> EditorResult<_> {
                let path = project.path(&format!("plugins/{name}"))?;
                let source = editor_watch::read_text(&path, EditorPlugin::MAX_SOURCE as u64)?;
                Ok(EditorPlugin::load(&name, &source)?)
            })();
            match result {
                Ok(plugin) => Some((name, plugin)),
                Err(error) => {
                    log::warn!("Editor plugin {name}: {error}");
                    None
                }
            }
        })
        .collect()
}

type MeshPreviews = HashMap<String, Handle<Mesh>>;

struct ImportJob {
    root: PathBuf,
    result: Receiver<MeshPreviews>,
}

fn import_paths(root: &std::path::Path, paths: Vec<String>) -> MeshPreviews {
    let assets = match Assets::new(root) {
        Ok(assets) => assets,
        Err(error) => {
            log::warn!("Could not start asset import: {error}");
            return MeshPreviews::new();
        }
    };
    let mut imported = 0;
    let mut meshes = MeshPreviews::new();
    for path in paths {
        let extension = std::path::Path::new(&path)
            .extension()
            .map(|ext| ext.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        let result = match extension.as_str() {
            "png" => import::<Texture>(&assets, &path),
            "fbx" => assets
                .request::<Mesh>(&path)
                .and_then(|request| request.wait())
                .map(|mesh| {
                    meshes.insert(path.clone(), mesh);
                }),
            "luau" => import::<ScriptAsset>(&assets, &path),
            "wav" | "ogg" => import::<AudioClip>(&assets, &path),
            _ => continue,
        };
        match result {
            Ok(()) => imported += 1,
            Err(error) => log::warn!("Import {path}: {error}"),
        }
    }
    if imported > 0 {
        log::info!("Imported {imported} assets into .telcache");
    }
    meshes
}

fn import<T: Asset>(assets: &Assets, path: &str) -> io::Result<()> {
    assets.request::<T>(path)?.wait().map(|_| ())
}

#[derive(Default)]
struct GameInput {
    events: RawInputState,
    pads: Gamepads,
    focused: bool,
    buttons: [Option<MouseButton>; egui::NUM_POINTER_BUTTONS],
}

impl GameInput {
    fn collect(&mut self, view: Option<&egui::Response>) {
        let focused =
            view.is_some_and(|view| view.has_focus() && view.ctx.input(|input| input.focused));
        if focused != self.focused {
            self.focused = focused;
            self.events.push(RawInputEvent::FocusChanged { focused });
            if !focused {
                self.pads = Gamepads::default();
                self.buttons.fill(None);
            }
        }
        let Some(view) = view.filter(|_| focused) else {
            return;
        };
        let ppp = f64::from(view.ctx.pixels_per_point());
        view.ctx.input(|input| {
            for event in &input.events {
                match *event {
                    egui::Event::Key {
                        key,
                        physical_key,
                        pressed,
                        repeat: false,
                        ..
                    } => {
                        let Some(key) = key_code(physical_key.unwrap_or(key)) else {
                            continue;
                        };
                        self.events.push(if pressed {
                            RawInputEvent::KeyPressed { key }
                        } else {
                            RawInputEvent::KeyReleased { key }
                        });
                    }
                    egui::Event::PointerMoved(point) if view.rect.contains(point) => {
                        self.events.push(RawInputEvent::MouseMoved {
                            x: f64::from(point.x - view.rect.left()) * ppp,
                            y: f64::from(point.y - view.rect.top()) * ppp,
                        });
                    }
                    egui::Event::MouseMoved(delta) if view.hovered() => {
                        self.events.push(RawInputEvent::MouseMotion {
                            dx: delta.x.into(),
                            dy: delta.y.into(),
                        });
                    }
                    egui::Event::PointerButton {
                        pos,
                        button,
                        pressed,
                        ..
                    } => {
                        if pressed && view.rect.contains(pos) {
                            let actual = match button {
                                egui::PointerButton::Primary => MouseButton::Left,
                                egui::PointerButton::Secondary => MouseButton::Right,
                                egui::PointerButton::Middle => MouseButton::Middle,
                                egui::PointerButton::Extra1 => MouseButton::Back,
                                egui::PointerButton::Extra2 => MouseButton::Forward,
                            };
                            self.buttons[button as usize] = Some(actual);
                            self.events
                                .push(RawInputEvent::MousePressed { button: actual });
                        } else if !pressed {
                            if let Some(button) = self.buttons[button as usize].take() {
                                self.events.push(RawInputEvent::MouseReleased { button });
                            }
                        }
                    }
                    egui::Event::MouseWheel { unit, delta, .. } if view.hovered() => {
                        self.events.push(RawInputEvent::MouseWheel {
                            x: delta.x.into(),
                            y: delta.y.into(),
                            pixels: unit == egui::MouseWheelUnit::Point,
                        });
                    }
                    _ => {}
                }
            }
        });
        self.pads.poll(&mut self.events);
    }
}

fn key_code(key: egui::Key) -> Option<KeyCode> {
    use egui::Key;
    macro_rules! map {
        ($($key:ident),*; $($from:ident => $to:ident),* $(,)?) => {
            match key { $(Key::$key => Some(KeyCode::$key),)* $(Key::$from => Some(KeyCode::$to),)* _ => None }
        };
    }
    map!(A, B, C, D, E, F, G, H, I, J, K, L, M, N, O, P, Q, R, S, T, U, V, W, X, Y, Z,
        Escape, Space, Tab, Delete, Insert, Home, End, PageUp, PageDown, F1, F2, F3, F4, F5, F6,
        F7, F8, F9, F10, F11, F12, Comma, Minus, Period, Slash, Backslash, Semicolon;
        ArrowUp => Up, ArrowDown => Down, ArrowLeft => Left, ArrowRight => Right, Enter => Return,
        Backspace => Back, Num0 => Key0, Num1 => Key1, Num2 => Key2, Num3 => Key3, Num4 => Key4,
        Num5 => Key5, Num6 => Key6, Num7 => Key7, Num8 => Key8, Num9 => Key9,
        Equals => Equal, OpenBracket => LeftBracket, CloseBracket => RightBracket, Backtick => GraveAccent, Quote => Apostrophe,
    )
}
