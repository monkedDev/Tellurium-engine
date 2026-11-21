use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use eframe::egui::{self, RichText};
use glam::{Mat4, Vec3};
use serde::{Deserialize, Serialize};
use tellurium_serialization::{
    atomic_write, is_project_file, BodyType, ColliderData, ColliderShape, Node, NodeKind,
    PackedScene, Project, ProjectConfig, RigidbodyData, Transform,
};

use crate::samples::{Sample, CATALOG};
use crate::theme::{rgba, Icons, ASH_ORANGE, ASH_YELLOW, BACKGROUND};

#[derive(Default, Clone, Copy, PartialEq)]
enum Template {
    Empty,
    TwoD,
    #[default]
    ThreeD,
}

#[derive(Serialize, Deserialize, Default)]
struct Recents {
    projects: Vec<Recent>,
}

#[derive(Serialize, Deserialize)]
struct Recent {
    path: PathBuf,
    opened: Option<u64>,
}

pub struct Manager {
    data_directories: Vec<PathBuf>,
    active_directory: Option<PathBuf>,
    recent: Recents,
    selected: Option<usize>,
    creation: Option<Creation>,
    scan_root: Option<PathBuf>,
}

#[derive(Default)]
struct Creation {
    name: String,
    description: String,
    folder: Option<PathBuf>,
    template: Template,
}

impl Manager {
    pub fn new() -> Self {
        Self::with_directories(data_directories())
    }

    fn with_directories(data_directories: Vec<PathBuf>) -> Self {
        let saved = data_directories
            .iter()
            .enumerate()
            .filter_map(|(index, folder)| {
                let file = folder.join("projects.toml");
                let data = fs::read_to_string(&file).ok()?;
                let recent: Recents = toml::from_str(&data).ok()?;
                let time = fs::metadata(&file)
                    .and_then(|metadata| metadata.modified())
                    .unwrap_or(UNIX_EPOCH);
                Some((time, std::cmp::Reverse(index), folder, recent))
            })
            .max_by_key(|(time, priority, _, _)| (*time, *priority));
        let (active_directory, recent) = match saved {
            Some((_, _, directory, recent)) => (Some(directory.clone()), recent),
            None => (None, Recents::default()),
        };
        Self {
            data_directories,
            active_directory,
            recent,
            selected: None,
            creation: None,
            scan_root: None,
        }
    }

    pub fn record(&mut self, project: &Project) -> io::Result<()> {
        self.recent
            .projects
            .retain(|item| item.path != project.file);
        let opened = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|elapsed| elapsed.as_secs());
        self.recent.projects.insert(
            0,
            Recent {
                path: project.file.clone(),
                opened,
            },
        );
        self.selected = Some(0);
        self.persist()
    }

    fn persist(&mut self) -> io::Result<()> {
        let text = toml::to_string_pretty(&self.recent).map_err(io::Error::other)?;
        self.write_data(|directory| atomic_write(&directory.join("projects.toml"), text.as_bytes()))
    }

    fn write_data<T>(&mut self, mut write: impl FnMut(&Path) -> io::Result<T>) -> io::Result<T> {
        let mut failure = None;
        for directory in self.active_directory.iter().chain(
            self.data_directories
                .iter()
                .filter(|path| Some(*path) != self.active_directory.as_ref()),
        ) {
            match write(directory) {
                Ok(result) => {
                    self.active_directory = Some(directory.clone());
                    return Ok(result);
                }
                Err(error) => {
                    failure = Some(io::Error::new(
                        error.kind(),
                        format!("{}: {error}", directory.display()),
                    ));
                }
            }
        }
        Err(failure.unwrap_or_else(|| io::Error::other("Tellurium data folder is unavailable")))
    }

    fn open_sample(&mut self, sample: &Sample) -> io::Result<Project> {
        // An already materialized project is authoritative even if settings have
        // since moved to another data folder.
        for directory in &self.data_directories {
            let root = directory.join("Samples");
            if sample.has_copy(&root) {
                return sample.open(&root);
            }
        }
        self.write_data(|directory| sample.open(&directory.join("Samples")))
    }

    pub fn show(
        &mut self,
        ctx: &egui::Context,
        icons: &Icons,
        error: &mut Option<String>,
    ) -> Option<Project> {
        let mut open = None;
        egui::TopBottomPanel::top("manager_header")
            .frame(
                egui::Frame::new()
                    .fill(BACKGROUND)
                    .inner_margin(egui::Margin::symmetric(24, 15)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.add(icons.image("tellurium", 32.0));
                    ui.label(RichText::new("Tellurium").strong().size(22.0));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!("Engine {}", env!("CARGO_PKG_VERSION")))
                                .size(12.0),
                        );
                    });
                });
            });
        egui::TopBottomPanel::bottom("manager_footer")
            .frame(
                egui::Frame::new()
                    .fill(BACKGROUND)
                    .inner_margin(egui::Margin::symmetric(24, 12)),
            )
            .show(ctx, |ui| {
                if let Some(error) = error.as_ref() {
                    ui.horizontal(|ui| {
                        ui.add(icons.image("console-error", 17.0));
                        ui.label(error);
                    });
                } else {
                    ui.label(format!("{} projects", self.recent.projects.len()));
                }
            });
        egui::SidePanel::right("project_details")
            .resizable(false)
            .exact_width(300.0)
            .frame(
                egui::Frame::new()
                    .fill(crate::theme::PANEL)
                    .inner_margin(egui::Margin::same(22)),
            )
            .show(ctx, |ui| {
                icons.heading(ui, "project-info", "Project details");
                ui.add_space(12.0);
                if let Some(recent) = self
                    .selected
                    .and_then(|index| self.recent.projects.get(index))
                {
                    match Project::load(&recent.path) {
                        Ok(project) => {
                            ui.label(RichText::new(&project.config.name).size(22.0).strong());
                            if !project.config.description.is_empty() {
                                ui.label(&project.config.description);
                            }
                            ui.separator();
                            ui.label(RichText::new("Project folder").strong());
                            ui.label(project.root().display().to_string());
                            ui.label(format!("Engine {}", project.config.engine_version));
                            if let Some(opened) = recent.opened {
                                ui.label(format!("Last opened {}", display_time(opened)));
                            }
                            if icons.button(ui, "project-open", "Open project").clicked() {
                                open = Some(project);
                            }
                        }
                        Err(failure) => {
                            ui.label(failure.to_string());
                        }
                    }
                    ui.add_space(10.0);
                    if icons
                        .button(ui, "project-remove", "Remove from list")
                        .clicked()
                    {
                        self.recent.projects.remove(self.selected.take().unwrap());
                        if let Err(error_value) = self.persist() {
                            *error = Some(error_value.to_string());
                        }
                    }
                } else {
                    ui.label("Select a project to view its details.");
                }
            });
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(BACKGROUND)
                    .inner_margin(egui::Margin::same(24)),
            )
            .show(ctx, |ui| {
                ui.heading(RichText::new("Projects").size(30.0));
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui
                        .add(egui::Button::image_and_text(
                            icons.image("project-new", 18.0),
                            RichText::new("New project").strong().color(ASH_YELLOW),
                        ))
                        .clicked()
                    {
                        self.creation = Some(Creation::default());
                    }
                    if icons.button(ui, "project-open", "Open file").clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("Tellurium project", &["toml", "tprj"])
                            .pick_file()
                        {
                            match Project::load(&path) {
                                Ok(project) => open = Some(project),
                                Err(failure) => *error = Some(failure.to_string()),
                            }
                        }
                    }
                    if icons.button(ui, "project-scan", "Scan folder").clicked() {
                        self.scan_root = rfd::FileDialog::new().pick_folder();
                    }
                });
                ui.add_space(14.0);
                ui.separator();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.heading("Built-in samples");
                    for sample in CATALOG {
                        let mut selected = false;
                        egui::Frame::new()
                            .fill(crate::theme::PANEL)
                            .corner_radius(5.0)
                            .inner_margin(egui::Margin::same(12))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.horizontal(|ui| {
                                    ui.add(icons.image(sample.icon, 22.0));
                                    ui.label(RichText::new(sample.name).strong().size(16.0));
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            selected = icons
                                                .button(ui, "project-open", "Open sample")
                                                .on_hover_text(format!("Open {}", sample.name))
                                                .clicked();
                                        },
                                    );
                                });
                                ui.label(sample.description);
                                ui.label(RichText::new(sample.controls).small().color(ASH_YELLOW));
                            });
                        if selected {
                            let result = if !self.data_directories.is_empty() {
                                self.open_sample(sample)
                            } else if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                                sample.open(&folder)
                            } else {
                                continue;
                            };
                            match result {
                                Ok(project) => {
                                    *error = None;
                                    open = Some(project);
                                }
                                Err(failure) => *error = Some(failure.to_string()),
                            }
                        }
                    }
                    ui.add_space(12.0);
                    ui.heading("Recent projects");
                    if self.recent.projects.is_empty() {
                        ui.label("Create a project or open one from disk.");
                    }
                    for (index, recent) in self.recent.projects.iter().enumerate() {
                        let name = Project::load(&recent.path)
                            .map(|project| project.config.name)
                            .unwrap_or_else(|_| {
                                recent
                                    .path
                                    .file_stem()
                                    .unwrap_or_default()
                                    .to_string_lossy()
                                    .into_owned()
                            });
                        let result = ui.add_sized(
                            [ui.available_width(), 54.0],
                            egui::Button::image_and_text(
                                icons.image("asset-folder", 22.0),
                                format!("{name}\n{}", recent.path.display()),
                            )
                            .selected(self.selected == Some(index)),
                        );
                        if result.clicked() {
                            self.selected = Some(index);
                            *error = None;
                        }
                        if result.double_clicked() {
                            match Project::load(&recent.path) {
                                Ok(project) => open = Some(project),
                                Err(failure) => *error = Some(failure.to_string()),
                            }
                        }
                    }
                });
            });
        if let Some(root) = self.scan_root.take() {
            match self.scan(&root) {
                Ok(0) => *error = Some("No new projects found.".into()),
                Ok(_) => *error = None,
                Err(failure) => *error = Some(failure.to_string()),
            }
        }
        if let Some(form) = &mut self.creation {
            let mut cancel = false;
            let mut project = None;
            egui::Modal::new("create_project".into()).show(ctx, |ui| {
                ui.set_width(440.0);
                icons.heading(ui, "project-new", "Create project");
                ui.label("Name");
                ui.add(egui::TextEdit::singleline(&mut form.name).desired_width(f32::INFINITY));
                ui.label("Description");
                ui.add(
                    egui::TextEdit::singleline(&mut form.description).desired_width(f32::INFINITY),
                );
                ui.horizontal(|ui| {
                    if ui.button("Choose parent folder").clicked() {
                        form.folder = rfd::FileDialog::new().pick_folder().or(form.folder.take());
                    }
                    if let Some(folder) = &form.folder {
                        ui.label(folder.display().to_string());
                    }
                });
                ui.label("Template");
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut form.template, Template::Empty, "Empty");
                    ui.selectable_value(&mut form.template, Template::TwoD, "2D");
                    ui.selectable_value(&mut form.template, Template::ThreeD, "3D");
                });
                if let Some(error) = error.as_ref() {
                    ui.label(error);
                }
                ui.separator();
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            form.folder.is_some() && !form.name.trim().is_empty(),
                            egui::Button::new(RichText::new("Create and open").color(ASH_YELLOW)),
                        )
                        .clicked()
                    {
                        match form.create() {
                            Ok(created) => project = Some(created),
                            Err(failure) => *error = Some(failure.to_string()),
                        }
                    }
                    cancel = ui.button("Cancel").clicked();
                });
            });
            if project.is_some() || cancel {
                self.creation = None;
            }
            if project.is_some() {
                open = project;
            }
        }
        open
    }

    fn scan(&mut self, root: &Path) -> io::Result<usize> {
        let mut pending = vec![root.to_path_buf()];
        let mut count = 0;
        while let Some(folder) = pending.pop() {
            for entry in fs::read_dir(&folder)?.filter_map(Result::ok) {
                let path = entry.path();
                let kind = entry.file_type()?;
                if kind.is_dir() && !entry.file_name().to_string_lossy().starts_with('.') {
                    pending.push(path);
                } else if kind.is_file() && is_project_file(&path) {
                    if let Ok(project) = Project::load(&path) {
                        if !self
                            .recent
                            .projects
                            .iter()
                            .any(|item| item.path == project.file)
                        {
                            self.recent.projects.push(Recent {
                                path: project.file,
                                opened: None,
                            });
                            count += 1;
                        }
                    }
                }
            }
        }
        self.persist()?;
        Ok(count)
    }
}

fn data_directories() -> Vec<PathBuf> {
    let mut directories = Vec::new();
    if let Some(local) = std::env::var_os("LOCALAPPDATA").filter(|value| !value.is_empty()) {
        directories.push(PathBuf::from(local).join("Tellurium"));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(bin) = exe.parent() {
            let workspace = bin
                .parent()
                .filter(|target| target.file_name().is_some_and(|name| name == "target"))
                .and_then(Path::parent)
                .filter(|engine| {
                    engine.join("editor/Cargo.toml").is_file()
                        && engine.join("runtime/Cargo.toml").is_file()
                });
            // Keep editable samples out of Cargo's target directory so they
            // survive a normal rebuild or Cargo clean.
            directories.push(match workspace {
                Some(engine) => engine.join(".tellurium"),
                None => bin.join("TelluriumData"),
            });
        }
    }
    directories
}

fn display_time(seconds: u64) -> String {
    let Ok(seconds) = i64::try_from(seconds) else {
        return "—".into();
    };
    chrono::Local
        .timestamp_opt(seconds, 0)
        .single()
        .map(|time| time.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| "—".into())
}

use chrono::TimeZone;

impl Creation {
    fn create(&self) -> io::Result<Project> {
        let folder = self
            .folder
            .as_ref()
            .ok_or_else(|| io::Error::other("Choose a parent folder"))?;
        let name = self.name.trim();
        let path = tellurium_serialization::relative_path(name)?;
        if path.components().count() != 1
            || name.contains(['<', '>', '"', '|', '?', '*'])
            || name.ends_with('.')
        {
            return Err(io::Error::other("Use a valid Windows folder name"));
        }
        let destination = folder.join(name);
        if destination.exists() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "A file or folder with this name already exists",
            ));
        }
        let staging = tempfile::tempdir_in(folder)?;
        let project = Project::new(
            staging.path().join("project.toml"),
            ProjectConfig {
                name: name.to_owned(),
                description: self.description.trim().to_owned(),
                ..ProjectConfig::default()
            },
        )?;
        fs::create_dir_all(project.path(&project.config.assets)?)?;
        let scene = template_scene(self.template, &project)?;
        scene.save(
            &project.path(&project.config.main_scene)?,
            project.config.scene_format,
        )?;
        project.save()?;
        fs::rename(staging.path(), &destination)?;
        Project::load(&destination)
    }
}

fn template_scene(template: Template, project: &Project) -> io::Result<PackedScene> {
    let mut scene = PackedScene::default();
    if template == Template::Empty {
        return Ok(scene);
    }
    let two_d = template == Template::TwoD;
    let root = scene.insert(
        Node::new(
            if two_d { "World2D" } else { "World3D" },
            if two_d {
                NodeKind::Node2D
            } else {
                NodeKind::Node3D
            },
        ),
        None,
    )?;
    let mut camera = Node::new("Camera", NodeKind::Camera);
    let eye = if two_d {
        Vec3::new(0.0, 0.0, -6.0)
    } else {
        Vec3::new(3.4, 2.5, -6.0)
    };
    let (_, rotation, position) = Mat4::look_at_lh(eye, Vec3::ZERO, Vec3::Y)
        .inverse()
        .to_scale_rotation_translation();
    camera.transform = Transform {
        position: position.to_array(),
        rotation: rotation.to_array(),
        ..Transform::default()
    };
    camera.audio_listener = true;
    scene.insert(camera, Some(root))?;
    let mut mesh = Node::new(
        if two_d { "Sprite" } else { "Cube" },
        if two_d {
            NodeKind::Sprite
        } else {
            NodeKind::MeshInstance
        },
    );
    mesh.mesh_renderer.as_mut().unwrap().tint = rgba(ASH_YELLOW);
    if two_d {
        let script = format!("{}/motion.luau", project.config.assets);
        atomic_write(&project.path(&script)?, b"function on_update(delta)\n    transform.set_position(math.sin(time.elapsed()) * 1.8, 0, 0)\nend\n")?;
        mesh.script = Some(tellurium_serialization::ScriptData {
            path: script,
            state: None,
        });
    } else {
        mesh.transform.position = [0.0, 1.5, 0.0];
        mesh.rigidbody = Some(RigidbodyData {
            kind: BodyType::Dynamic,
            mass: 1.0,
            linear_velocity: [0.0; 3],
        });
        mesh.collider = Some(box_collider());
        let mut floor = Node::new("Floor", NodeKind::MeshInstance);
        floor.transform.position = [0.0, -0.75, 0.0];
        floor.transform.scale = [4.5, 0.22, 3.2];
        floor.mesh_renderer.as_mut().unwrap().tint = rgba(ASH_ORANGE);
        floor.rigidbody = Some(RigidbodyData {
            kind: BodyType::Static,
            mass: 0.0,
            linear_velocity: [0.0; 3],
        });
        floor.collider = Some(box_collider());
        scene.insert(floor, Some(root))?;
    }
    scene.insert(mesh, Some(root))?;
    Ok(scene)
}

pub fn box_collider() -> ColliderData {
    ColliderData {
        shape: ColliderShape::Box,
        dimensions: [0.5; 3],
        trigger: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unavailable_app_data_uses_a_persistent_fallback_for_samples_and_history() -> io::Result<()> {
        let sandbox = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR"))?;
        let blocked = sandbox.path().join("app-data");
        fs::write(&blocked, b"not a directory")?;
        let fallback = sandbox.path().join("local-data");
        let dirs = vec![blocked, fallback.clone()];

        let mut manager = Manager::with_directories(dirs.clone());
        let sample = manager.open_sample(&CATALOG[0])?;
        assert!(sample.root().starts_with(fallback.canonicalize()?));
        manager.record(&sample)?;
        manager.record(&sample)?;

        let mut restarted = Manager::with_directories(dirs);
        assert_eq!(restarted.recent.projects.len(), 1);
        assert_eq!(restarted.recent.projects[0].path, sample.file);
        assert_eq!(restarted.open_sample(&CATALOG[0])?.file, sample.file);
        Ok(())
    }
}
