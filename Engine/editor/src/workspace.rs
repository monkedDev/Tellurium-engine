use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Duration;

use eframe::egui::{self, RichText};
use tellurium_serialization::{
    DocumentFormat, EntityId, GraphicsBackend, Node, PackedScene, Project,
};

use crate::inspector::{self, kind_icon, kind_name, KINDS};
use crate::scene_view::SceneView;
use crate::theme::{Icons, ASH_ORANGE, ASH_YELLOW, BACKGROUND, BORDER, PANEL, TEXT};
use crate::{EditorEvent, EditorResult, GameHost, Messages, ResourceKind};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Player {
    Stopped,
    Playing,
    Paused,
}

enum Navigation {
    Manager,
    New(PathBuf),
    Open(PathBuf),
    Close,
}

pub struct Workspace {
    pub project: Project,
    scene: PackedScene,
    scene_path: PathBuf,
    dirty: bool,
    selected: Option<EntityId>,
    collapsed: HashSet<EntityId>,
    asset: Option<String>,
    files: Vec<String>,
    filter: String,
    view: SceneView,
    game_view: bool,
    console: bool,
    player: Player,
    pending: Option<Navigation>,
    close_confirmed: bool,
    settings: Option<tellurium_serialization::ProjectConfig>,
    export: bool,
    export_job: Option<Receiver<io::Result<PathBuf>>>,
    status: String,
}

impl Workspace {
    pub fn open(project: Project, host: &mut impl GameHost) -> EditorResult<Self> {
        let scene_path = project.path(&project.config.main_scene)?;
        let scene = PackedScene::load(&scene_path)?;
        let mut view = SceneView::default();
        view.reset(&scene);
        let mut work = Self {
            project,
            scene_path,
            scene,
            view,
            dirty: false,
            selected: None,
            collapsed: HashSet::new(),
            asset: None,
            files: Vec::new(),
            filter: String::new(),
            game_view: false,
            console: false,
            player: Player::Stopped,
            pending: None,
            close_confirmed: false,
            settings: None,
            export: false,
            export_job: None,
            status: "Scene opened".into(),
        };
        work.refresh(host)?;
        work.remember_scene();
        host.open_project(&work.project);
        work.publish_scene_opened(host);
        Ok(work)
    }

    pub fn show(
        &mut self,
        ctx: &egui::Context,
        frame: &eframe::Frame,
        icons: &Icons,
        messages: &Messages,
        host: &mut impl GameHost,
    ) -> bool {
        let selected = self.selected;
        for event in host.reload() {
            host.editor_event(event.clone());
            if let EditorEvent::SceneChanged(path) = &event {
                let result = self.reload_scene(path, host);
                self.report(result);
            }
        }
        if let Some(job) = &self.export_job {
            let result = match job.try_recv() {
                Ok(result) => Some(result),
                Err(TryRecvError::Disconnected) => {
                    Some(Err(io::Error::other("Export worker stopped")))
                }
                Err(TryRecvError::Empty) => {
                    ctx.request_repaint_after(Duration::from_millis(100));
                    None
                }
            };
            if let Some(result) = result {
                self.export_job = None;
                match result {
                    Ok(exe) => {
                        self.status = format!("Exported {}", comparable_path(&exe).display());
                        log::info!("{}", self.status);
                    }
                    Err(error) => self.report::<(), _>(Err(error)),
                }
            }
        }
        if !self.close_confirmed
            && self.dirty
            && ctx.input(|input| input.viewport().close_requested())
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.pending = Some(Navigation::Close);
        }
        if self.player == Player::Stopped
            && ctx.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::S))
        {
            let result = self.save();
            self.report(result);
        }
        let mut action = None;
        let mut step = false;
        egui::TopBottomPanel::top("toolbar")
            .frame(panel(egui::Margin::symmetric(12, 9)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| self.toolbar(ui, icons, host, &mut action, &mut step));
            });
        egui::TopBottomPanel::bottom("status")
            .frame(panel(egui::Margin::symmetric(12, 5)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("●")
                            .color(if self.player == Player::Playing {
                                ASH_YELLOW
                            } else {
                                ASH_ORANGE
                            })
                            .small(),
                    );
                    ui.label(RichText::new(&self.status).small());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!(
                                "{:?}  ·  {} nodes",
                                self.project.config.graphics_backend,
                                self.scene.nodes.len()
                            ))
                            .small(),
                        );
                    });
                });
            });
        egui::TopBottomPanel::bottom("assets_console")
            .default_height(150.0)
            .min_height(91.0)
            .resizable(true)
            .frame(panel(egui::Margin::symmetric(12, 8)))
            .show(ctx, |ui| {
                self.bottom(ui, icons, messages, host, &mut action)
            });
        egui::SidePanel::left("hierarchy")
            .resizable(true)
            .default_width(225.0)
            .min_width(170.0)
            .max_width(360.0)
            .frame(panel(egui::Margin::same(11)))
            .show(ctx, |ui| self.hierarchy(ui, icons));
        egui::SidePanel::right("inspector")
            .resizable(true)
            .default_width(304.0)
            .min_width(240.0)
            .max_width(490.0)
            .frame(panel(egui::Margin::same(11)))
            .show(ctx, |ui| {
                icons.heading(ui, "panel-inspector", "Inspector");
                ui.separator();
                if let Some(id) = self.selected {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        ui.add_enabled_ui(self.player == Player::Stopped, |ui| {
                            ui.push_id(id.0, |ui| {
                                let result = inspector::show(
                                    ui,
                                    icons,
                                    &mut self.scene,
                                    id,
                                    self.asset.as_deref(),
                                    &mut self.dirty,
                                );
                                self.report(result);
                            });
                        });
                        let globals = host.script_globals(id);
                        if !globals.is_empty() {
                            ui.separator();
                            ui.collapsing("Live script state", |ui| {
                                for (name, value) in globals {
                                    ui.label(format!("{name} = {value}"));
                                }
                            });
                        }
                    });
                } else {
                    ui.label("Select a node to edit its components.");
                }
            });
        let mut game = None;
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BACKGROUND))
            .show(ctx, |ui| {
                panel(egui::Margin::symmetric(10, 6)).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if tab(ui, icons, "panel-scene", "Scene View", !self.game_view) {
                            self.game_view = false;
                        }
                        if tab(ui, icons, "panel-game", "Game View", self.game_view) {
                            self.game_view = true;
                        }
                        ui.separator();
                        if self.game_view {
                            ui.label(match self.player {
                                Player::Stopped => "Stopped",
                                Player::Playing => "Playing",
                                Player::Paused => "Paused",
                            });
                        } else {
                            ui.toggle_value(&mut self.view.planar, "2D");
                            if ui.small_button("Focus").clicked() {
                                self.view.focus(&self.scene, self.selected);
                            }
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let name = self
                                .scene_path
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy();
                            ui.label(
                                RichText::new(format!(
                                    "{name}{}",
                                    if self.dirty { " *" } else { "" }
                                ))
                                .small(),
                            );
                        });
                    });
                });
                if !self.game_view {
                    self.view.draw(ui, &self.scene, &mut self.selected, host);
                } else if self.player == Player::Stopped {
                    ui.centered_and_justified(|ui| {
                        ui.label("Press Play to run this scene.");
                    });
                } else {
                    let (_, response) =
                        ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
                    if response.clicked() {
                        response.request_focus();
                    }
                    game = Some(response);
                }
            });

        if let Some(action) = action {
            if self.dirty {
                self.pending = Some(action);
            } else if self.navigate(action, ctx, host) {
                return true;
            }
        }
        if self.pending.is_some() {
            let mut decision = None;
            egui::Modal::new("unsaved_scene".into()).show(ctx, |ui| {
                ui.heading("Save changes?");
                ui.label("The current scene has unsaved edits.");
                ui.horizontal(|ui| {
                    if ui.button("Save").clicked() {
                        decision = Some(1);
                    }
                    if ui.button("Discard").clicked() {
                        decision = Some(2);
                    }
                    if ui.button("Cancel").clicked() {
                        decision = Some(0);
                    }
                });
            });
            if let Some(decision) = decision {
                let saved = if decision == 1 {
                    let result = self.save();
                    let saved = result.is_ok();
                    self.report(result);
                    saved
                } else {
                    true
                };
                if saved {
                    let action = self.pending.take().unwrap();
                    if decision != 0 && self.navigate(action, ctx, host) {
                        return true;
                    }
                }
            }
        }
        self.settings_dialog(ctx, icons, host);
        let visible = if self.pending.is_none()
            && self.settings.is_none()
            && !ctx.memory(|memory| memory.any_popup_open())
        {
            game.as_ref()
        } else {
            None
        };
        if self.selected != selected {
            host.editor_event(EditorEvent::EntitySelected(self.selected.map(|id| id.0)));
        }
        if let Err(error) = host.frame(frame, visible, self.player != Player::Playing, step) {
            self.stop(host);
            self.console = true;
            self.report::<(), _>(Err(error));
        }
        ctx.request_repaint_after(if self.player == Player::Stopped {
            Duration::from_millis(300)
        } else {
            Duration::from_millis(15)
        });
        false
    }

    fn toolbar(
        &mut self,
        ui: &mut egui::Ui,
        icons: &Icons,
        host: &mut impl GameHost,
        action: &mut Option<Navigation>,
        step: &mut bool,
    ) {
        ui.add(icons.image("tellurium", 24.0));
        ui.label(RichText::new(&self.project.config.name).strong().size(15.0));
        ui.separator();
        ui.menu_button("Project", |ui| {
            if ui.button("Project manager").clicked() {
                *action = Some(Navigation::Manager);
                ui.close_menu();
            }
            if ui.button("Project settings").clicked() {
                self.settings = Some(self.project.config.clone());
                self.export = false;
                ui.close_menu();
            }
        });
        ui.add_enabled_ui(self.player == Player::Stopped, |ui| {
            if icons
                .button(ui, "scene-save", "Save")
                .on_hover_text("Save scene (Ctrl+S)")
                .clicked()
            {
                let result = self.save();
                self.report(result);
            }
            ui.menu_button("Scene", |ui| {
                if icons.button(ui, "scene-new", "New scene").clicked() {
                    *action = self.scene_picker(true).map(Navigation::New);
                    ui.close_menu();
                }
                if icons.button(ui, "project-open", "Open scene").clicked() {
                    *action = self.scene_picker(false).map(Navigation::Open);
                    ui.close_menu();
                }
                if ui.button("Save as").clicked() {
                    if let Some(path) = self.scene_picker(true) {
                        let result = self.save_as(path);
                        self.report(result);
                    }
                    ui.close_menu();
                }
                if ui.button("Set as startup scene").clicked() {
                    let result = self.set_startup_scene();
                    self.report(result);
                    ui.close_menu();
                }
                if icons
                    .button(ui, "scene-instance", "Instance scene")
                    .clicked()
                {
                    self.add_instance();
                    ui.close_menu();
                }
            });
        });
        ui.separator();
        let play = if self.player == Player::Paused {
            "Resume"
        } else {
            "Play"
        };
        ui.add_enabled_ui(self.player != Player::Playing, |ui| {
            if ui
                .add(egui::Button::image_and_text(
                    icons.image("play", 17.0),
                    RichText::new(play).color(ASH_YELLOW).strong(),
                ))
                .clicked()
            {
                let result = if self.player == Player::Paused {
                    Ok(())
                } else {
                    host.play(&self.project, self.scene.clone())
                };
                if result.is_ok() {
                    self.player = Player::Playing;
                    self.game_view = true;
                    self.status = "Playing scene".into();
                }
                self.report(result);
            }
        });
        ui.add_enabled_ui(self.player == Player::Playing, |ui| {
            if icons.button(ui, "pause", "Pause").clicked() {
                self.player = Player::Paused;
                self.status = "Paused".into();
            }
        });
        ui.add_enabled_ui(self.player == Player::Paused, |ui| {
            if icons.button(ui, "step", "Step").clicked() {
                *step = true;
                self.game_view = true;
            }
        });
        ui.add_enabled_ui(self.player != Player::Stopped, |ui| {
            if icons.button(ui, "stop", "Stop").clicked() {
                self.stop(host);
            }
        });
        ui.menu_button("Snapshot", |ui| {
            if ui
                .add_enabled(
                    self.player != Player::Stopped,
                    egui::Button::new("Save world"),
                )
                .clicked()
            {
                if let Some(path) = snapshot_picker(&self.project).save_file() {
                    let result = host.save(path, self.project.config.scene_format);
                    if result.is_ok() {
                        self.status = "World snapshot saved".into();
                    }
                    self.report(result);
                }
                ui.close_menu();
            }
            if ui.button("Load world").clicked() {
                if let Some(path) = snapshot_picker(&self.project).pick_file() {
                    let result = host.load(&self.project, path);
                    if result.is_ok() {
                        self.player = Player::Paused;
                        self.game_view = true;
                        self.status = "World snapshot loaded".into();
                    }
                    self.report(result);
                }
                ui.close_menu();
            }
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add_enabled_ui(self.export_job.is_none(), |ui| {
                    icons.button(ui, "export", "Export")
                })
                .inner
                .clicked()
            {
                self.settings = Some(self.project.config.clone());
                self.export = true;
            }
        });
    }

    fn bottom(
        &mut self,
        ui: &mut egui::Ui,
        icons: &Icons,
        messages: &Messages,
        host: &mut impl GameHost,
        action: &mut Option<Navigation>,
    ) {
        ui.horizontal(|ui| {
            if tab(ui, icons, "panel-project", "Project", !self.console) {
                self.console = false;
            }
            if tab(ui, icons, "panel-console", "Console", self.console) {
                self.console = true;
            }
            ui.separator();
            if self.console {
                if ui.small_button("Clear").clicked() {
                    messages.clear();
                }
            } else {
                ui.add(
                    egui::TextEdit::singleline(&mut self.filter)
                        .hint_text("Filter files")
                        .desired_width(155.0),
                );
                if ui.small_button("Refresh").clicked() {
                    let result = self.refresh(host);
                    self.report(result);
                }
                if ui.small_button("Import files").clicked() {
                    let result = self.import_files(host);
                    self.report(result);
                }
            }
        });
        ui.separator();
        if self.console {
            if let Some(frame) = host.metrics() {
                ui.horizontal_wrapped(|ui| {
                    ui.label(format!("{:.0} FPS", frame.fps));
                    ui.label(format!("Frame {:.2} ms", frame.frame_ms));
                    ui.label(format!("Entities {}", frame.entities));
                    ui.label(format!("Luau {:.0} KiB", frame.luau_bytes as f64 / 1024.0));
                    ui.label(format!("Draw calls {}", frame.draw_calls));
                    ui.label(format!("Physics {:.2} ms", frame.physics_ms));
                    ui.label(format!("ECS {:.2} ms", frame.ecs_ms));
                });
                ui.separator();
            }
            egui::ScrollArea::vertical()
                .stick_to_bottom(true)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for (level, message) in messages.recent() {
                        ui.horizontal(|ui| {
                            match level {
                                log::Level::Error => {
                                    ui.add(icons.image("console-error", 16.0));
                                }
                                log::Level::Warn => {
                                    ui.add(icons.image("console-warning", 16.0));
                                }
                                _ => {
                                    ui.label(RichText::new("●").small().color(ASH_ORANGE));
                                }
                            }
                            ui.label(RichText::new(message).monospace().size(12.0));
                        });
                    }
                });
        } else {
            let filter = self.filter.to_lowercase();
            egui::ScrollArea::vertical().show(ui, |ui| {
                for file in self
                    .files
                    .iter()
                    .filter(|file| file.to_lowercase().contains(&filter))
                {
                    let response = ui.add(
                        egui::Button::image_and_text(icons.image(asset_icon(file), 15.0), file)
                            .selected(self.asset.as_ref() == Some(file)),
                    );
                    if response.clicked() {
                        self.asset = Some(file.clone());
                    }
                    if response.double_clicked()
                        && self.player == Player::Stopped
                        && extension(Path::new(file)) == "telsc"
                    {
                        match self.project.path(file) {
                            Ok(path) => *action = Some(Navigation::Open(path)),
                            Err(error) => log::error!("{error}"),
                        }
                    }
                }
            });
        }
    }

    fn hierarchy(&mut self, ui: &mut egui::Ui, icons: &Icons) {
        icons.heading(ui, "panel-hierarchy", "Hierarchy");
        ui.add_enabled_ui(self.player == Player::Stopped, |ui| {
            ui.horizontal(|ui| {
                ui.menu_button(RichText::new("+ Node").color(ASH_YELLOW), |ui| {
                    ui.label(if self.selected.is_some() {
                        "Add child"
                    } else {
                        "Add to scene root"
                    });
                    for kind in KINDS {
                        if icons.button(ui, kind_icon(kind), kind_name(kind)).clicked() {
                            let result = self
                                .scene
                                .insert(Node::new(kind_name(kind), kind), self.selected);
                            if let Ok(id) = &result {
                                if let Some(parent) = self.selected {
                                    self.collapsed.remove(&parent);
                                }
                                self.selected = Some(*id);
                                self.dirty = true;
                            }
                            self.report(result);
                            ui.close_menu();
                        }
                    }
                });
                if ui.small_button("Root").clicked() {
                    self.selected = None;
                }
                if self.selected.is_some()
                    && ui
                        .small_button("Remove")
                        .on_hover_text("Remove selected node and its children")
                        .clicked()
                {
                    self.scene.remove_subtree(self.selected.take().unwrap());
                    self.dirty = true;
                }
            });
        });
        ui.separator();
        if self.scene.nodes.is_empty() {
            ui.label("Add a node to start the scene.");
        }
        let mut children: HashMap<_, Vec<_>> = HashMap::new();
        for node in &self.scene.nodes {
            children.entry(node.parent).or_default().push(node);
        }
        let mut pending: Vec<_> = children
            .get(&None)
            .into_iter()
            .flatten()
            .rev()
            .map(|node| (*node, 0))
            .collect();
        egui::ScrollArea::vertical().show(ui, |ui| {
            while let Some((node, depth)) = pending.pop() {
                let nested = children.get(&Some(node.id));
                ui.push_id(node.id.0, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 3.0;
                        ui.add_space((depth as f32 * 13.0).min(104.0));
                        if nested.is_some() {
                            let icon = if self.collapsed.contains(&node.id) {
                                "chevron-right"
                            } else {
                                "chevron-down"
                            };
                            if ui
                                .add(egui::Button::image(icons.image(icon, 13.0)).frame(false))
                                .clicked()
                                && !self.collapsed.remove(&node.id)
                            {
                                self.collapsed.insert(node.id);
                            }
                        } else {
                            ui.add_space(18.0);
                        }
                        let response = ui.add(
                            egui::Button::image_and_text(
                                icons.image(kind_icon(node.kind), 15.0),
                                &node.name,
                            )
                            .selected(self.selected == Some(node.id))
                            .wrap_mode(egui::TextWrapMode::Truncate),
                        );
                        if response.clicked() {
                            self.selected = Some(node.id);
                        }
                        if response.double_clicked() {
                            self.view.focus(&self.scene, self.selected);
                        }
                    });
                });
                if !self.collapsed.contains(&node.id) {
                    pending.extend(
                        nested
                            .into_iter()
                            .flatten()
                            .rev()
                            .map(|node| (*node, depth + 1)),
                    );
                }
            }
        });
    }

    fn settings_dialog(&mut self, ctx: &egui::Context, icons: &Icons, host: &mut impl GameHost) {
        let Some(mut config) = self.settings.take() else {
            return;
        };
        let mut close = false;
        let mut apply = false;
        let modal = egui::Modal::new("project_settings".into()).show(ctx, |ui| {
            ui.set_width(435.0);
            if self.export {
                icons.heading(ui, "export", "Windows export");
                config.export.release = true;
                ui.label("Release · DirectX 12");
                ui.horizontal(|ui| {
                    if ui.button("Output folder").clicked() {
                        if let Some(path) = rfd::FileDialog::new().pick_folder() {
                            config.export.target = path.to_string_lossy().into_owned();
                        }
                    }
                    ui.label(&config.export.target);
                });
                ui.horizontal(|ui| {
                    if ui.button("Application icon").clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("Windows icon", &["ico"])
                            .pick_file()
                        {
                            config.export.icon = path.to_string_lossy().into_owned();
                        }
                    }
                    ui.label(&config.export.icon);
                });
                ui.checkbox(&mut config.export.console, "Debug console");
            } else {
                icons.heading(ui, "settings", "Project settings");
                ui.label("Project name");
                ui.text_edit_singleline(&mut config.name);
                ui.label("Description");
                ui.text_edit_multiline(&mut config.description);
                ui.horizontal(|ui| {
                    ui.add(icons.image("graphics-backend", 17.0));
                    ui.label("Graphics backend");
                    ui.selectable_value(
                        &mut config.graphics_backend,
                        GraphicsBackend::Dx12,
                        "DirectX 12",
                    );
                    ui.add_enabled(false, egui::Button::new("Vulkan"))
                        .on_disabled_hover_text("This runtime provides DirectX 12");
                });
                ui.horizontal(|ui| {
                    ui.label("Scene and save format");
                    ui.selectable_value(&mut config.scene_format, DocumentFormat::Text, "Text");
                    ui.selectable_value(&mut config.scene_format, DocumentFormat::Binary, "Binary");
                });
            }
            ui.separator();
            ui.horizontal(|ui| {
                apply = if self.export {
                    let label = if self.dirty {
                        "Save and export"
                    } else {
                        "Export"
                    };
                    ui.add_enabled(!config.export.target.is_empty(), egui::Button::new(label))
                        .clicked()
                } else {
                    ui.button("Save settings").clicked()
                };
                close = ui.button("Cancel").clicked();
            });
        });
        if apply {
            let mut project = self.project.clone();
            project.config = config.clone();
            let result: EditorResult<()> = if self.export {
                (|| {
                    if self.dirty {
                        self.save()?;
                    }
                    project.save()?;
                    let output = PathBuf::from(&project.config.export.target);
                    self.export_job = Some(host.export(project.clone(), output)?);
                    self.status = "Exporting Windows build…".into();
                    ctx.request_repaint_after(Duration::from_millis(100));
                    Ok(())
                })()
            } else {
                project.save().map_err(Into::into)
            };
            match result {
                Ok(()) => {
                    self.project = project;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!(
                        "{} — Tellurium",
                        self.project.config.name
                    )));
                    if !self.export {
                        self.status = "Project settings saved".into();
                    }
                    close = true;
                }
                Err(error) => self.report::<(), _>(Err(error)),
            }
        }
        if !close && !modal.should_close() {
            self.settings = Some(config);
        }
    }

    fn stop(&mut self, host: &mut impl GameHost) {
        host.stop();
        if self.player != Player::Stopped {
            self.player = Player::Stopped;
            self.game_view = false;
            self.status = "Play session stopped".into();
        }
    }

    fn report<T, E: std::fmt::Display>(&mut self, result: Result<T, E>) {
        if let Err(error) = result {
            self.status = error.to_string();
            log::error!("{error}");
        }
    }

    fn save(&mut self) -> EditorResult<()> {
        self.scene
            .save(&self.scene_path, self.project.config.scene_format)?;
        self.dirty = false;
        self.status = "Scene saved".into();
        Ok(())
    }

    fn save_as(&mut self, path: PathBuf) -> EditorResult<()> {
        let path = self.checked_scene_path(&path.with_extension("telsc"))?;
        self.scene.save(&path, self.project.config.scene_format)?;
        self.scene_path = path;
        self.dirty = false;
        self.status = "Scene saved".into();
        self.remember_scene();
        self.list_files()?;
        Ok(())
    }

    fn relative(&self, path: &Path) -> io::Result<String> {
        let relative = path
            .strip_prefix(self.project.root())
            .map_err(|_| io::Error::other("Choose a path inside the project"))?;
        let text = relative
            .to_str()
            .ok_or_else(|| io::Error::other("The project path is not UTF-8"))?
            .replace('\\', "/");
        Ok(text)
    }

    fn checked_scene_path(&self, path: &Path) -> io::Result<PathBuf> {
        // Native file dialogs return paths without the Windows verbatim prefix.
        let path = comparable_path(path);
        let root = comparable_path(self.project.root());
        let relative = path
            .strip_prefix(&root)
            .map_err(|_| io::Error::other("Choose a path inside the project"))?;
        let relative = relative
            .to_str()
            .ok_or_else(|| io::Error::other("Invalid scene path"))?;
        let path = self.project.path(relative)?;
        if !path.starts_with(self.project.path(&self.project.config.scenes)?)
            || extension(&path) != "telsc"
        {
            return Err(io::Error::other(
                "Choose a .telsc file in the project scenes folder",
            ));
        }
        Ok(path)
    }

    fn remember_scene(&mut self) {
        if let Ok(path) = self.relative(&self.scene_path) {
            if self.project.config.recent_scenes.first() == Some(&path) {
                return;
            }
            self.project
                .config
                .recent_scenes
                .retain(|existing| existing != &path);
            self.project.config.recent_scenes.insert(0, path);
            self.project.config.recent_scenes.truncate(12);
            if let Err(error) = self.project.save() {
                log::warn!("Could not save recent scenes: {error}");
            }
        }
    }

    fn set_startup_scene(&mut self) -> EditorResult<()> {
        let mut project = self.project.clone();
        project.config.main_scene = self.relative(&self.scene_path)?;
        project.save()?;
        self.project = project;
        self.status = "Startup scene updated".into();
        Ok(())
    }

    fn scene_picker(&self, save: bool) -> Option<PathBuf> {
        let folder = self.project.path(&self.project.config.scenes).ok()?;
        let picker = rfd::FileDialog::new()
            .set_directory(comparable_path(&folder))
            .add_filter("Tellurium scene", &["telsc"]);
        if save {
            picker.set_file_name("new_scene.telsc").save_file()
        } else {
            picker.pick_file()
        }
    }

    fn add_instance(&mut self) {
        let result: EditorResult<()> = (|| {
            let Some(path) = self.scene_picker(false) else {
                return Ok(());
            };
            let path = self.checked_scene_path(&path)?;
            let scene = PackedScene::load(&path)?;
            let name = path.file_stem().unwrap_or_default().to_string_lossy();
            self.selected = Some(self.scene.instance(&name, &scene, self.selected)?);
            self.dirty = true;
            Ok(())
        })();
        self.report(result);
    }

    fn navigate(
        &mut self,
        action: Navigation,
        ctx: &egui::Context,
        host: &mut impl GameHost,
    ) -> bool {
        let result: EditorResult<bool> = (|| match action {
            Navigation::Manager => Ok(true),
            Navigation::Close => {
                self.close_confirmed = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                Ok(false)
            }
            Navigation::New(path) => {
                let path = self.checked_scene_path(&path.with_extension("telsc"))?;
                if path.exists() {
                    return Err(io::Error::other("A scene with this name already exists").into());
                }
                let scene = PackedScene::default();
                scene.save(&path, self.project.config.scene_format)?;
                self.set_scene(path, scene, host);
                self.list_files()?;
                Ok(false)
            }
            Navigation::Open(path) => {
                let path = self.checked_scene_path(&path)?;
                let scene = PackedScene::load(&path)?;
                self.set_scene(path, scene, host);
                Ok(false)
            }
        })();
        match result {
            Ok(exit) => exit,
            Err(error) => {
                self.report::<(), _>(Err(error));
                false
            }
        }
    }

    fn set_scene(&mut self, path: PathBuf, scene: PackedScene, host: &mut impl GameHost) {
        self.stop(host);
        self.scene_path = path;
        self.scene = scene;
        self.view.reset(&self.scene);
        self.selected = None;
        self.collapsed.clear();
        self.dirty = false;
        self.status = "Scene opened".into();
        self.remember_scene();
        self.publish_scene_opened(host);
    }

    fn publish_scene_opened(&self, host: &mut impl GameHost) {
        if let Ok(path) = self.relative(&self.scene_path) {
            host.editor_event(EditorEvent::SceneOpened(path));
        }
    }

    fn reload_scene(&mut self, path: &str, host: &mut impl GameHost) -> EditorResult<()> {
        self.list_files()?;
        if !self.relative(&self.scene_path)?.eq_ignore_ascii_case(path) {
            return Ok(());
        }
        if self.dirty {
            self.status = "Scene changed on disk. Unsaved edits kept.".into();
            return Ok(());
        }
        let scene = PackedScene::load(&self.project.path(path)?)?;
        if scene.to_bytes(DocumentFormat::Binary)? == self.scene.to_bytes(DocumentFormat::Binary)? {
            return Ok(());
        }
        if self.player != Player::Stopped {
            host.play(&self.project, scene.clone())?;
        }
        self.selected = self.selected.filter(|id| scene.node(*id).is_some());
        self.scene = scene;
        self.status = format!("Reloaded {path}");
        host.editor_event(EditorEvent::ResourceReloaded(
            ResourceKind::Scene,
            path.to_owned(),
        ));
        Ok(())
    }

    fn list_files(&mut self) -> io::Result<()> {
        let mut paths = vec![
            self.project.path(&self.project.config.assets)?,
            self.project.path(&self.project.config.scenes)?,
        ];
        let mut seen = HashSet::new();
        let mut files = Vec::new();
        while let Some(folder) = paths.pop() {
            if !folder.is_dir() || !seen.insert(folder.clone()) {
                continue;
            }
            for entry in fs::read_dir(&folder)? {
                let entry = entry?;
                if entry.file_name().to_string_lossy().starts_with('.') {
                    continue;
                }
                let path = entry.path();
                let kind = entry.file_type()?;
                if kind.is_dir() {
                    paths.push(path);
                } else if kind.is_file() && extension(&path) != "telmeta" {
                    files.push(self.relative(&path)?);
                }
            }
        }
        files.sort_unstable();
        files.dedup();
        self.files = files;
        Ok(())
    }

    fn refresh(&mut self, host: &mut impl GameHost) -> EditorResult<()> {
        self.list_files()?;
        host.import(&self.project, self.files.clone());
        Ok(())
    }

    fn import_files(&mut self, host: &mut impl GameHost) -> EditorResult<()> {
        let Some(files) = rfd::FileDialog::new()
            .add_filter("Tellurium assets", &["png", "fbx", "wav", "ogg", "luau"])
            .pick_files()
        else {
            return Ok(());
        };
        let assets = self.project.path(&self.project.config.assets)?;
        fs::create_dir_all(&assets)?;
        let mut copied = Vec::new();
        let result: EditorResult<()> = (|| {
            for file in files {
                let name = file
                    .file_name()
                    .ok_or_else(|| io::Error::other("Asset has no filename"))?;
                let target = assets.join(name);
                if target.is_file() && file.canonicalize()? == target.canonicalize()? {
                    continue;
                }
                let mut input = fs::File::open(file)?;
                let mut output = tempfile::NamedTempFile::new_in(&assets)?;
                io::copy(&mut input, &mut output)?;
                output.persist_noclobber(&target)?;
                copied.push(target);
            }
            Ok(())
        })();
        if result.is_err() {
            for path in copied {
                let _ = fs::remove_file(path);
            }
            return result;
        }
        self.refresh(host)
    }
}

fn comparable_path(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(path) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{path}"));
    }
    if let Some(path) = text.strip_prefix(r"\\?\") {
        return PathBuf::from(path);
    }
    path.to_path_buf()
}

fn panel(margin: egui::Margin) -> egui::Frame {
    egui::Frame::new()
        .fill(PANEL)
        .stroke(egui::Stroke::new(1.0_f32, BORDER))
        .inner_margin(margin)
}

fn tab(ui: &mut egui::Ui, icons: &Icons, icon: &str, label: &str, selected: bool) -> bool {
    ui.add(
        egui::Button::image_and_text(
            icons.image(icon, 16.0),
            RichText::new(label)
                .strong()
                .color(if selected { ASH_YELLOW } else { TEXT }),
        )
        .selected(selected),
    )
    .clicked()
}

fn extension(path: &Path) -> String {
    path.extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

fn asset_icon(file: &str) -> &'static str {
    match extension(Path::new(file)).as_str() {
        "telsc" => "panel-scene",
        "luau" => "asset-script",
        "png" => "asset-texture",
        "fbx" => "node-mesh",
        "wav" | "ogg" => "asset-audio",
        _ => "asset-material",
    }
}

fn snapshot_picker(project: &Project) -> rfd::FileDialog {
    rfd::FileDialog::new()
        .set_directory(comparable_path(project.root()))
        .add_filter("Tellurium world snapshot", &["telsave"])
        .set_file_name("world.telsave")
}
