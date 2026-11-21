use std::collections::HashMap;
use std::f32::consts::FRAC_PI_2;

use eframe::egui::{self, Color32, Pos2, Rect, Stroke};
use glam::{Mat4, Quat, Vec3};
use tellurium_serialization::{compose, EntityId, NodeKind, PackedScene, Transform};

use crate::theme::{ASH_ORANGE, ASH_YELLOW, BACKGROUND, BORDER, TEXT};
use crate::GameHost;

pub struct SceneView {
    pub planar: bool,
    yaw: f32,
    pitch: f32,
    zoom: f32,
    target: Vec3,
}

impl Default for SceneView {
    fn default() -> Self {
        Self {
            planar: false,
            yaw: 0.55,
            pitch: 0.38,
            zoom: 3.3,
            target: Vec3::ZERO,
        }
    }
}

impl SceneView {
    pub fn reset(&mut self, scene: &PackedScene) {
        *self = Self {
            planar: scene.nodes.iter().any(|node| node.kind == NodeKind::Node2D),
            ..Self::default()
        };
    }

    pub fn focus(&mut self, scene: &PackedScene, selected: Option<EntityId>) {
        self.target = selected
            .and_then(|id| poses(scene).remove(&id))
            .map_or(Vec3::ZERO, |pose| Vec3::from_array(pose.position));
    }

    pub fn draw(
        &mut self,
        ui: &mut egui::Ui,
        scene: &PackedScene,
        selected: &mut Option<EntityId>,
        host: &impl GameHost,
    ) {
        let (rect, input) =
            ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0, BACKGROUND);
        if rect.width() < 2.0 || rect.height() < 2.0 {
            return;
        }

        let direction = if self.planar {
            Vec3::NEG_Z
        } else {
            Vec3::new(
                self.yaw.sin() * self.pitch.cos(),
                self.pitch.sin(),
                -self.yaw.cos() * self.pitch.cos(),
            )
        };
        let right = Vec3::Y.cross(-direction).normalize();
        let up = (-direction).cross(right);
        let scale = rect.height() / (2.0 * self.zoom);
        let delta = ui.input(|input| input.pointer.delta());
        if input.dragged_by(egui::PointerButton::Middle)
            || (self.planar && input.dragged_by(egui::PointerButton::Secondary))
        {
            self.target -= (right * delta.x - up * delta.y) / scale;
        } else if input.dragged_by(egui::PointerButton::Secondary) {
            self.yaw += delta.x * 0.013;
            self.pitch = (self.pitch + delta.y * 0.013).clamp(-FRAC_PI_2 + 0.04, FRAC_PI_2 - 0.04);
        }
        if input.hovered() {
            let scroll = ui.input(|input| input.smooth_scroll_delta.y);
            self.zoom = (self.zoom * (-scroll * 0.0025).exp()).clamp(0.15, 1000.0);
        }

        let view = Mat4::look_at_lh(self.target + direction * 20.0, self.target, Vec3::Y);
        let width = self.zoom * rect.width() / rect.height();
        let camera = Camera {
            matrix: Mat4::orthographic_lh(-width, width, -self.zoom, self.zoom, -1000.0, 1000.0)
                * view,
            rect,
        };
        self.grid(&painter, &camera);

        let transforms = poses(scene);
        let mut draw: Vec<_> = scene
            .nodes
            .iter()
            .filter_map(|node| {
                let transform = *transforms.get(&node.id)?;
                let center = Vec3::from_array(transform.position);
                let depth = camera.matrix.transform_point3(center).z;
                Some((depth, node, transform))
            })
            .collect();
        draw.sort_by(|a, b| b.0.total_cmp(&a.0));

        let pointer = input.interact_pointer_pos();
        let mut hit = None;
        let mut triangle_budget = 4000;
        for (_, node, pose) in draw {
            let point = camera.project(Vec3::from_array(pose.position));
            if !point.x.is_finite() || !point.y.is_finite() {
                continue;
            }
            let active = *selected == Some(node.id);
            let outline = if active { ASH_YELLOW } else { ASH_ORANGE };
            if let Some(mesh) = node.mesh_renderer.as_ref().filter(|mesh| mesh.visible) {
                let color = Color32::from_rgb(
                    (mesh.tint[0] * 255.0) as u8,
                    (mesh.tint[1] * 255.0) as u8,
                    (mesh.tint[2] * 255.0) as u8,
                );
                let preview = mesh
                    .mesh
                    .as_deref()
                    .and_then(|path| host.preview_mesh(path));
                let bounds = if let Some(preview) = preview.filter(|_| triangle_budget > 0) {
                    draw_mesh(
                        &painter,
                        &camera,
                        pose,
                        preview,
                        outline,
                        &mut triangle_budget,
                    )
                } else {
                    draw_primitive(
                        &painter,
                        &camera,
                        pose,
                        node.kind == NodeKind::Sprite,
                        color,
                        outline,
                        active,
                    )
                };
                if pointer.is_some_and(|pointer| bounds.expand(3.0).contains(pointer)) {
                    hit = Some(node.id);
                }
            } else {
                let size = if active { 5.0 } else { 3.0 };
                painter.circle_stroke(point, size, Stroke::new(1.0_f32, outline));
                if active
                    || matches!(
                        node.kind,
                        NodeKind::Camera | NodeKind::Light | NodeKind::AudioStreamPlayer
                    )
                {
                    painter.text(
                        point + egui::vec2(9.0, -9.0),
                        egui::Align2::LEFT_CENTER,
                        &node.name,
                        egui::FontId::monospace(11.0),
                        if active { ASH_YELLOW } else { TEXT },
                    );
                }
                if pointer.is_some_and(|pointer| pointer.distance(point) < 10.0) {
                    hit = Some(node.id);
                }
            }
        }
        if input.clicked_by(egui::PointerButton::Primary) {
            *selected = hit;
        }
        painter.text(
            rect.left_bottom() + egui::vec2(12.0, -12.0),
            egui::Align2::LEFT_BOTTOM,
            if self.planar {
                "Drag MMB to pan  ·  Scroll to zoom"
            } else {
                "Drag RMB to orbit  ·  MMB to pan  ·  Scroll to zoom"
            },
            egui::FontId::proportional(11.0),
            TEXT,
        );
    }

    fn grid(&self, painter: &egui::Painter, camera: &Camera) {
        let step = 10.0_f32.powf((self.zoom / 5.0).log10().floor());
        let radius = 55;
        let coordinates = if self.planar {
            self.target.y
        } else {
            self.target.z
        };
        let x = (self.target.x / step).round() * step;
        let z = (coordinates / step).round() * step;
        let point = |a, b| {
            if self.planar {
                Vec3::new(a, b, 0.0)
            } else {
                Vec3::new(a, 0.0, b)
            }
        };
        let distance = radius as f32 * step;
        for offset in -radius..=radius {
            let offset = offset as f32 * step;
            let a = x + offset;
            let b = z + offset;
            for (location, before, after) in [
                (a, point(a, z - distance), point(a, z + distance)),
                (b, point(x - distance, b), point(x + distance, b)),
            ] {
                let color = if location.abs() < step * 0.05 {
                    ASH_ORANGE.gamma_multiply(0.55)
                } else {
                    BORDER.gamma_multiply(0.65)
                };
                painter.line_segment(
                    [camera.project(before), camera.project(after)],
                    Stroke::new(1.0_f32, color),
                );
            }
        }
    }
}

struct Camera {
    matrix: Mat4,
    rect: Rect,
}

impl Camera {
    fn project(&self, position: Vec3) -> Pos2 {
        let clip = self.matrix.transform_point3(position);
        self.rect.center()
            + egui::vec2(
                clip.x * self.rect.width() * 0.5,
                -clip.y * self.rect.height() * 0.5,
            )
    }
}

fn poses(scene: &PackedScene) -> HashMap<EntityId, Transform> {
    // The editor can show the last entered transform even while other inspector fields are invalid.
    let mut transforms = HashMap::with_capacity(scene.nodes.len());
    let mut children: HashMap<Option<EntityId>, Vec<_>> = HashMap::new();
    for node in &scene.nodes {
        children.entry(node.parent).or_default().push(node);
    }
    let mut pending = children.remove(&None).unwrap_or_default();
    while let Some(node) = pending.pop() {
        let pose = node
            .parent
            .and_then(|id| transforms.get(&id).copied())
            .map_or(node.transform, |parent| compose(parent, node.transform));
        transforms.insert(node.id, pose);
        if let Some(nodes) = children.remove(&Some(node.id)) {
            pending.extend(nodes);
        }
    }
    transforms
}

fn draw_primitive(
    painter: &egui::Painter,
    camera: &Camera,
    pose: Transform,
    quad: bool,
    color: Color32,
    outline: Color32,
    active: bool,
) -> Rect {
    let z = if quad { 0.0 } else { 0.5 };
    let points = [
        Vec3::new(-0.5, -0.5, -z),
        Vec3::new(0.5, -0.5, -z),
        Vec3::new(0.5, 0.5, -z),
        Vec3::new(-0.5, 0.5, -z),
        Vec3::new(-0.5, -0.5, z),
        Vec3::new(0.5, -0.5, z),
        Vec3::new(0.5, 0.5, z),
        Vec3::new(-0.5, 0.5, z),
    ]
    .map(|point| {
        Vec3::from_array(pose.position)
            + Quat::from_array(pose.rotation) * (Vec3::from_array(pose.scale) * point)
    });
    let mut faces: Vec<_> = [
        ([0, 1, 2, 3], 0.78),
        ([5, 4, 7, 6], 0.55),
        ([4, 0, 3, 7], 0.63),
        ([1, 5, 6, 2], 0.70),
        ([3, 2, 6, 7], 0.93),
        ([4, 5, 1, 0], 0.45),
    ]
    .into_iter()
    .take(if quad { 1 } else { 6 })
    .map(|(indices, shade)| {
        let depth = indices
            .iter()
            .map(|&index| camera.matrix.transform_point3(points[index]).z)
            .sum::<f32>();
        (depth, indices, shade)
    })
    .collect();
    faces.sort_by(|first, second| second.0.total_cmp(&first.0));
    for (_, indices, shade) in faces {
        painter.add(egui::Shape::convex_polygon(
            indices
                .into_iter()
                .map(|index| camera.project(points[index]))
                .collect(),
            color.gamma_multiply(0.58 + shade * 0.36),
            Stroke::new(
                if active { 1.3_f32 } else { 0.6 },
                outline.gamma_multiply(if active { 1.0 } else { 0.6 }),
            ),
        ));
    }
    Rect::from_points(&points.map(|point| camera.project(point)))
}

fn draw_mesh(
    painter: &egui::Painter,
    camera: &Camera,
    pose: Transform,
    mesh: &tellurium_assets::Mesh,
    outline: Color32,
    budget: &mut usize,
) -> Rect {
    let count = (mesh.indices.len() / 3).min(*budget).min(2000);
    if count == 0 {
        let center = camera.project(Vec3::from_array(pose.position));
        return Rect::from_center_size(center, egui::vec2(8.0, 8.0));
    }
    *budget -= count;
    let model = Mat4::from_scale_rotation_translation(
        Vec3::from_array(pose.scale),
        Quat::from_array(pose.rotation),
        Vec3::from_array(pose.position),
    );
    let mut bounds = Rect::NOTHING;
    for triangle in mesh
        .indices
        .chunks_exact(3)
        .step_by((mesh.indices.len() / 3).div_ceil(count))
        .take(count)
    {
        let project = |index: u32| {
            camera.project(
                model.transform_point3(Vec3::from_array(mesh.vertices[index as usize].position)),
            )
        };
        let points = [
            project(triangle[0]),
            project(triangle[1]),
            project(triangle[2]),
        ];
        bounds = bounds.union(Rect::from_points(&points));
        painter.add(egui::Shape::closed_line(
            points.to_vec(),
            Stroke::new(0.8_f32, outline.gamma_multiply(0.68)),
        ));
    }
    bounds
}
