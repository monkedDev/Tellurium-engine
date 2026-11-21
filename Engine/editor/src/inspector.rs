use eframe::egui::{self, RichText};
use glam::{EulerRot, Quat};
use tellurium_serialization::{
    AudioSourceData, BodyType, CameraData, ColliderShape, EntityId, MeshRendererData, NodeKind,
    PackedScene, RigidbodyData, ScriptData,
};

use crate::projects::box_collider;
use crate::theme::{Icons, ASH_YELLOW};

pub fn kind_icon(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::Node2D => "node-2d",
        NodeKind::Node3D => "node-3d",
        NodeKind::Camera => "node-camera",
        NodeKind::Light => "node-light",
        NodeKind::MeshInstance => "node-mesh",
        NodeKind::Sprite => "node-sprite",
        NodeKind::AudioStreamPlayer => "node-audio",
    }
}

pub const KINDS: [NodeKind; 7] = [
    NodeKind::Node3D,
    NodeKind::Node2D,
    NodeKind::MeshInstance,
    NodeKind::Sprite,
    NodeKind::Camera,
    NodeKind::Light,
    NodeKind::AudioStreamPlayer,
];

pub fn kind_name(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::Node2D => "Node2D",
        NodeKind::Node3D => "Node3D",
        NodeKind::Camera => "Camera",
        NodeKind::Light => "Light",
        NodeKind::MeshInstance => "Mesh Instance",
        NodeKind::Sprite => "Sprite",
        NodeKind::AudioStreamPlayer => "Audio Player",
    }
}

pub fn show(
    ui: &mut egui::Ui,
    icons: &Icons,
    doc: &mut PackedScene,
    id: EntityId,
    selected_asset: Option<&str>,
    dirty: &mut bool,
) -> std::io::Result<()> {
    let parents: Vec<_> = doc
        .nodes
        .iter()
        .filter(|node| node.id != id)
        .map(|node| (node.id, node.name.clone()))
        .collect();
    let Some(node) = doc.node_mut(id) else {
        return Ok(());
    };
    ui.horizontal(|ui| {
        ui.add(icons.image(kind_icon(node.kind), 21.0));
        ui.label(RichText::new(kind_name(node.kind)).size(17.0).strong());
    });

    ui.label("Name");
    *dirty |= ui
        .add(egui::TextEdit::singleline(&mut node.name).desired_width(f32::INFINITY))
        .changed();
    let mut parent = node.parent;
    let parent_name = parents
        .iter()
        .find(|(id, _)| Some(*id) == parent)
        .map_or("Scene root", |(_, name)| name);
    egui::ComboBox::from_label("Parent")
        .selected_text(parent_name)
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut parent, None, "Scene root");
            for (id, name) in &parents {
                ui.selectable_value(&mut parent, Some(*id), format!("{name}  ({})", id.0));
            }
        });

    ui.separator();
    icons.heading(ui, "component-transform", "Transform");
    *dirty |= vector(ui, "Position", &mut node.transform.position, 0.04);
    let (x, y, z) = Quat::from_array(node.transform.rotation).to_euler(EulerRot::XYZ);
    let mut degrees = [x.to_degrees(), y.to_degrees(), z.to_degrees()];
    if vector(ui, "Rotation (°)", &mut degrees, 0.35) {
        node.transform.rotation = Quat::from_euler(
            EulerRot::XYZ,
            degrees[0].to_radians(),
            degrees[1].to_radians(),
            degrees[2].to_radians(),
        )
        .normalize()
        .to_array();
        *dirty = true;
    }
    *dirty |= vector(ui, "Scale", &mut node.transform.scale, 0.02);

    if let Some(mesh) = &mut node.mesh_renderer {
        if header(ui, icons, "node-mesh", "Mesh Renderer") {
            node.mesh_renderer = None;
            *dirty = true;
        } else {
            *dirty |= ui.checkbox(&mut mesh.visible, "Visible").changed();
            *dirty |= asset(ui, "Mesh (.fbx)", &mut mesh.mesh, selected_asset, &["fbx"]);
            *dirty |= asset(
                ui,
                "Texture (.png)",
                &mut mesh.texture,
                selected_asset,
                &["png"],
            );
            ui.horizontal(|ui| {
                ui.label("Tint");
                *dirty |= ui
                    .color_edit_button_rgba_unmultiplied(&mut mesh.tint)
                    .changed();
            });
        }
    }

    if let Some(body) = &mut node.rigidbody {
        if header(ui, icons, "component-rigidbody", "Rigidbody") {
            node.rigidbody = None;
            *dirty = true;
        } else {
            let before = body.kind;
            egui::ComboBox::from_label("Type")
                .selected_text(format!("{:?}", body.kind))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut body.kind, BodyType::Static, "Static");
                    ui.selectable_value(&mut body.kind, BodyType::Dynamic, "Dynamic");
                    ui.selectable_value(&mut body.kind, BodyType::Kinematic, "Kinematic");
                });
            if before != body.kind {
                if body.kind == BodyType::Dynamic && body.mass == 0.0 {
                    body.mass = 1.0;
                }
                *dirty = true;
            }
            *dirty |= ui
                .add(
                    egui::DragValue::new(&mut body.mass)
                        .range(0.0..=100000.0)
                        .speed(0.05)
                        .prefix("Mass "),
                )
                .changed();
            *dirty |= vector(ui, "Linear velocity", &mut body.linear_velocity, 0.04);
        }
    }

    if let Some(collider) = &mut node.collider {
        if header(ui, icons, "component-collider", "Collider") {
            node.collider = None;
            *dirty = true;
        } else {
            let before = collider.shape;
            egui::ComboBox::from_label("Shape")
                .selected_text(format!("{:?}", collider.shape))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut collider.shape, ColliderShape::Box, "Box");
                    ui.selectable_value(&mut collider.shape, ColliderShape::Sphere, "Sphere");
                });
            *dirty |= before != collider.shape;
            if collider.shape == ColliderShape::Box {
                *dirty |= vector(ui, "Half extents", &mut collider.dimensions, 0.02);
            } else {
                *dirty |= ui
                    .add(
                        egui::DragValue::new(&mut collider.dimensions[0])
                            .range(0.001..=10000.0)
                            .speed(0.025)
                            .prefix("Radius "),
                    )
                    .changed();
            }
            *dirty |= ui.checkbox(&mut collider.trigger, "Trigger").changed();
        }
    }

    if let Some(script) = &mut node.script {
        if header(ui, icons, "asset-script", "Luau Script") {
            node.script = None;
            *dirty = true;
        } else {
            *dirty |= required_asset(
                ui,
                "Script (.luau)",
                &mut script.path,
                selected_asset,
                &["luau"],
            );
            if let Some(state) = &script.state {
                for (key, value) in &state.globals {
                    ui.label(format!("{key} = {value:?}"));
                }
            }
        }
    }

    if let Some(audio) = &mut node.audio_source {
        if header(ui, icons, "node-audio", "Audio Source") {
            node.audio_source = None;
            *dirty = true;
        } else {
            *dirty |= required_asset(
                ui,
                "Clip (.wav or .ogg)",
                &mut audio.clip,
                selected_asset,
                &["wav", "ogg"],
            );
            *dirty |= ui
                .add(egui::Slider::new(&mut audio.volume, 0.0..=1.0).text("Volume"))
                .changed();
            *dirty |= ui.checkbox(&mut audio.looping, "Loop").changed();
            *dirty |= ui.checkbox(&mut audio.spatial, "Spatial audio").changed();
        }
    }

    if node.audio_listener && header(ui, icons, "component-audio-listener", "Audio Listener") {
        node.audio_listener = false;
        *dirty = true;
    }

    if let Some(camera) = &mut node.camera {
        if header(ui, icons, "node-camera", "Camera") {
            node.camera = None;
            *dirty = true;
        } else {
            *dirty |= ui
                .checkbox(&mut camera.active, "Active in Game View")
                .changed();
            *dirty |= ui
                .add(
                    egui::DragValue::new(&mut camera.half_height)
                        .range(0.01..=10000.0)
                        .speed(0.04)
                        .prefix("Half height "),
                )
                .changed();
        }
    }

    if !node.custom.is_empty() {
        ui.separator();
        ui.label(RichText::new("Custom components").strong());
        for (name, value) in &mut node.custom {
            ui.label(name);
            *dirty |= ui
                .add(
                    egui::TextEdit::multiline(value)
                        .desired_rows(2)
                        .code_editor(),
                )
                .changed();
        }
    }

    ui.separator();
    ui.menu_button(RichText::new("Add component").color(ASH_YELLOW), |ui| {
        macro_rules! add {
            ($field:ident, $name:literal, $value:expr) => {
                if ui
                    .add_enabled(node.$field.is_none(), egui::Button::new($name))
                    .clicked()
                {
                    node.$field = Some($value);
                    *dirty = true;
                    ui.close_menu();
                }
            };
        }
        add!(mesh_renderer, "Mesh Renderer", MeshRendererData::default());
        add!(
            rigidbody,
            "Rigidbody",
            RigidbodyData {
                kind: BodyType::Dynamic,
                mass: 1.0,
                linear_velocity: [0.0; 3]
            }
        );
        add!(collider, "Collider", box_collider());
        add!(
            script,
            "Luau Script",
            ScriptData {
                path: matching(selected_asset, &["luau"]).unwrap_or("").to_owned(),
                state: None
            }
        );
        add!(
            audio_source,
            "Audio Source",
            AudioSourceData {
                clip: matching(selected_asset, &["wav", "ogg"])
                    .unwrap_or("")
                    .to_owned(),
                volume: 1.0,
                looping: false,
                spatial: true
            }
        );
        add!(
            camera,
            "Camera",
            CameraData {
                active: true,
                half_height: 2.5
            }
        );
        if ui
            .add_enabled(!node.audio_listener, egui::Button::new("Audio Listener"))
            .clicked()
        {
            node.audio_listener = true;
            *dirty = true;
            ui.close_menu();
        }
    });
    if parent != node.parent {
        doc.reparent(id, parent)?;
        *dirty = true;
    }
    Ok(())
}

fn header(ui: &mut egui::Ui, icons: &Icons, icon: &str, name: &str) -> bool {
    ui.separator();
    ui.horizontal(|ui| {
        icons.heading(ui, icon, name);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.small_button("×")
                .on_hover_text("Remove component")
                .clicked()
        })
        .inner
    })
    .inner
}

fn vector(ui: &mut egui::Ui, label: &str, values: &mut [f32; 3], speed: f64) -> bool {
    ui.label(label);
    ui.horizontal(|ui| {
        let mut changed = false;
        for (axis, value) in ["X ", "Y ", "Z "].into_iter().zip(values) {
            changed |= ui
                .add(egui::DragValue::new(value).speed(speed).prefix(axis))
                .changed();
        }
        changed
    })
    .inner
}

fn matching<'a>(asset: Option<&'a str>, extensions: &[&str]) -> Option<&'a str> {
    asset.filter(|path| {
        std::path::Path::new(path).extension().is_some_and(|ext| {
            extensions
                .iter()
                .any(|known| ext.eq_ignore_ascii_case(known))
        })
    })
}

fn required_asset(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut String,
    selected: Option<&str>,
    extensions: &[&str],
) -> bool {
    ui.label(label);
    let mut changed = ui
        .add(egui::TextEdit::singleline(value).desired_width(f32::INFINITY))
        .changed();
    if let Some(path) = matching(selected, extensions) {
        if ui.small_button("Use selected asset").clicked() {
            *value = path.to_owned();
            changed = true;
        }
    }
    changed
}

fn asset(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut Option<String>,
    selected: Option<&str>,
    extensions: &[&str],
) -> bool {
    let mut text = value.clone().unwrap_or_default();
    let changed = required_asset(ui, label, &mut text, selected, extensions);
    if changed {
        *value = (!text.is_empty()).then_some(text);
    }
    changed
}
