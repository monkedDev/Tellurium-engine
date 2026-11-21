//! Native project manager and scene editor.

mod inspector;
mod projects;
pub mod samples;
mod scene_view;
mod theme;
mod workspace;

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

pub use eframe;
pub use eframe::egui;
use tellurium_diagnostics::FrameMetrics;
use tellurium_serialization::{DocumentFormat, PackedScene, Project};

use crate::projects::Manager;
use crate::theme::Icons;
use crate::workspace::Workspace;

pub type EditorResult<T> = Result<T, Box<dyn std::error::Error>>;

#[derive(Clone, Copy)]
pub enum ResourceKind {
    Script,
    Texture,
    Shader,
    Scene,
}

#[derive(Clone)]
pub enum EditorEvent {
    SceneOpened(String),
    SceneChanged(String),
    EntitySelected(Option<u64>),
    ResourceReloaded(ResourceKind, String),
}

pub fn game_view_clear_color() -> [f32; 4] {
    theme::rgba(theme::BACKGROUND)
}

/// The editor owns documents and widgets. The runtime owns ECS, simulation, and rendering.
pub trait GameHost {
    fn open_project(&mut self, project: &Project);
    fn close_project(&mut self);
    fn reload(&mut self) -> Vec<EditorEvent>;
    fn editor_event(&mut self, event: EditorEvent);
    fn metrics(&self) -> Option<FrameMetrics>;
    fn script_globals(&self, entity: tellurium_serialization::EntityId) -> Vec<(String, String)>;
    fn play(&mut self, project: &Project, scene: PackedScene) -> EditorResult<()>;
    fn stop(&mut self);
    fn frame(
        &mut self,
        frame: &eframe::Frame,
        viewport: Option<&egui::Response>,
        paused: bool,
        step: bool,
    ) -> EditorResult<()>;
    fn save(&mut self, path: PathBuf, format: DocumentFormat) -> EditorResult<()>;
    fn load(&mut self, project: &Project, path: PathBuf) -> EditorResult<()>;
    fn import(&mut self, project: &Project, paths: Vec<String>);
    fn preview_mesh(&self, path: &str) -> Option<&tellurium_assets::Mesh>;
    fn export(
        &mut self,
        project: Project,
        output: PathBuf,
    ) -> EditorResult<Receiver<std::io::Result<PathBuf>>>;
}

pub fn run(host: impl GameHost + 'static, project: Option<PathBuf>) -> eframe::Result {
    let messages = Messages::default();
    if log::set_boxed_logger(Box::new(messages.clone())).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Tellurium — Projects")
            .with_icon(theme::window_icon())
            .with_inner_size([1240.0, 780.0])
            .with_min_inner_size([940.0, 620.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Tellurium",
        options,
        Box::new(|ctx| {
            theme::apply(&ctx.egui_ctx);
            let mut app = Editor {
                host,
                icons: Icons::new(&ctx.egui_ctx),
                manager: Manager::new(),
                workspace: None,
                messages,
                manager_error: None,
            };
            if let Some(project) = project {
                match Project::load(&project) {
                    Ok(project) => app.open(project, &ctx.egui_ctx),
                    Err(error) => app.manager_error = Some(error.to_string()),
                }
            }
            Ok(Box::new(app))
        }),
    )
}

struct Editor<H> {
    host: H,
    icons: Icons,
    manager: Manager,
    workspace: Option<Workspace>,
    messages: Messages,
    manager_error: Option<String>,
}

impl<H: GameHost> Editor<H> {
    fn open(&mut self, project: Project, ctx: &egui::Context) {
        match Workspace::open(project, &mut self.host) {
            Ok(workspace) => {
                if let Err(error) = self.manager.record(&workspace.project) {
                    log::warn!("Could not update recent projects: {error}");
                }
                ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!(
                    "{} — Tellurium",
                    workspace.project.config.name
                )));
                self.manager_error = None;
                self.workspace = Some(workspace);
            }
            Err(error) => self.manager_error = Some(error.to_string()),
        }
    }
}

impl<H: GameHost> eframe::App for Editor<H> {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if let Some(workspace) = &mut self.workspace {
            if workspace.show(ctx, frame, &self.icons, &self.messages, &mut self.host) {
                self.host.stop();
                self.host.close_project();
                self.workspace = None;
                ctx.send_viewport_cmd(egui::ViewportCommand::Title("Tellurium — Projects".into()));
            }
        } else if let Some(project) = self.manager.show(ctx, &self.icons, &mut self.manager_error) {
            self.open(project, ctx);
        }
    }
}

#[derive(Clone, Default)]
struct Messages(Arc<Mutex<VecDeque<(log::Level, String)>>>);

impl Messages {
    fn recent(&self) -> Vec<(log::Level, String)> {
        self.0.lock().unwrap().iter().cloned().collect()
    }

    fn clear(&self) {
        self.0.lock().unwrap().clear();
    }
}

impl log::Log for Messages {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::Level::Info
            && (metadata.level() <= log::Level::Warn || metadata.target().starts_with("tellurium"))
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            let mut entries = self.0.lock().unwrap();
            if entries.len() == 500 {
                entries.pop_front();
            }
            entries.push_back((record.level(), record.args().to_string()));
        }
    }

    fn flush(&self) {}
}
