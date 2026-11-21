use std::collections::HashMap;
use std::sync::Arc;

use eframe::egui::{
    self, Color32, ColorImage, Context, IconData, Image, RichText, TextureHandle, Vec2,
};

pub const BACKGROUND: Color32 = Color32::from_rgb(0x15, 0x13, 0x10);
pub const PANEL: Color32 = Color32::from_rgb(0x21, 0x1D, 0x19);
pub const BORDER: Color32 = Color32::from_rgb(0x3B, 0x33, 0x2A);
pub const ASH_YELLOW: Color32 = Color32::from_rgb(0xD5, 0xBB, 0x7E);
pub const ASH_ORANGE: Color32 = Color32::from_rgb(0xC7, 0x93, 0x70);
pub const TEXT: Color32 = Color32::from_rgb(0xEB, 0xE0, 0xD1);

pub fn rgba(color: Color32) -> [f32; 4] {
    color.to_array().map(|channel| f32::from(channel) / 255.0)
}

macro_rules! icon_files {
    ($($name:literal),* $(,)?) => {
        &[$(($name, include_bytes!(concat!("../../../assets/icons/", $name, ".svg")).as_slice())),*]
    };
}

const ICONS: &[(&str, &[u8])] = icon_files!(
    "asset-audio",
    "asset-folder",
    "asset-material",
    "asset-script",
    "asset-texture",
    "build-profile",
    "chevron-down",
    "chevron-right",
    "component-audio-listener",
    "component-collider",
    "component-rigidbody",
    "component-transform",
    "console-error",
    "console-warning",
    "export",
    "graphics-backend",
    "node-2d",
    "node-3d",
    "node-add",
    "node-audio",
    "node-camera",
    "node-light",
    "node-mesh",
    "node-sprite",
    "panel-console",
    "panel-game",
    "panel-hierarchy",
    "panel-inspector",
    "panel-project",
    "panel-scene",
    "pause",
    "play",
    "project-info",
    "project-new",
    "project-open",
    "project-remove",
    "project-scan",
    "scene-instance",
    "scene-new",
    "scene-save",
    "settings",
    "step",
    "stop",
);

const MARK: &[u8] = include_bytes!("../../../assets/icons/tellurium-editor.png");

pub fn apply(ctx: &Context) {
    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = egui::vec2(8.0, 7.0);
    style.spacing.button_padding = egui::vec2(8.0, 5.0);
    style.spacing.interact_size.y = 24.0;
    style.spacing.window_margin = egui::Margin::same(15);
    style.visuals = egui::Visuals::dark();
    let visuals = &mut style.visuals;
    visuals.override_text_color = Some(TEXT);
    visuals.panel_fill = PANEL;
    visuals.window_fill = PANEL;
    visuals.window_stroke = egui::Stroke::new(1.0_f32, BORDER);
    visuals.extreme_bg_color = BACKGROUND;
    visuals.faint_bg_color = BACKGROUND;
    visuals.code_bg_color = BACKGROUND;
    visuals.selection.bg_fill = BORDER;
    visuals.selection.stroke = egui::Stroke::new(1.0_f32, ASH_YELLOW);
    visuals.text_cursor.stroke = egui::Stroke::new(1.5_f32, ASH_YELLOW);
    visuals.hyperlink_color = ASH_ORANGE;
    visuals.warn_fg_color = ASH_YELLOW;
    visuals.error_fg_color = ASH_ORANGE;
    visuals.widgets.noninteractive.bg_fill = PANEL;
    visuals.widgets.noninteractive.weak_bg_fill = PANEL;
    visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0_f32, BORDER);
    for widget in [
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        widget.bg_fill = BORDER;
        widget.weak_bg_fill = BORDER;
        widget.bg_stroke = egui::Stroke::NONE;
        widget.fg_stroke = egui::Stroke::new(1.0_f32, TEXT);
        widget.corner_radius = egui::CornerRadius::same(4);
    }
    visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0_f32, ASH_ORANGE);
    visuals.widgets.active.bg_stroke = egui::Stroke::new(1.0_f32, ASH_YELLOW);
    ctx.set_style(style);
}

pub fn window_icon() -> Arc<IconData> {
    let pixels = image::load_from_memory(MARK)
        .expect("bundled editor mark is a PNG")
        .into_rgba8();
    Arc::new(IconData {
        width: pixels.width(),
        height: pixels.height(),
        rgba: pixels.into_raw(),
    })
}

pub struct Icons(HashMap<&'static str, TextureHandle>);

impl Icons {
    pub fn new(ctx: &Context) -> Self {
        let options = resvg::usvg::Options::default();
        let mut icons = HashMap::new();
        for &(name, svg) in ICONS {
            let tree = resvg::usvg::Tree::from_data(svg, &options)
                .expect("bundled editor icon is valid SVG");
            let mut pixmap = resvg::tiny_skia::Pixmap::new(48, 48).unwrap();
            resvg::render(
                &tree,
                resvg::tiny_skia::Transform::from_scale(2.0, 2.0),
                &mut pixmap.as_mut(),
            );
            let pixels = ColorImage::from_rgba_premultiplied([48, 48], pixmap.data());
            icons.insert(
                name,
                ctx.load_texture(name, pixels, egui::TextureOptions::LINEAR),
            );
        }
        let mark = window_icon();
        icons.insert(
            "tellurium",
            ctx.load_texture(
                "tellurium",
                ColorImage::from_rgba_unmultiplied(
                    [mark.width as usize, mark.height as usize],
                    &mark.rgba,
                ),
                egui::TextureOptions::LINEAR,
            ),
        );
        Self(icons)
    }

    pub fn image(&self, name: &str, size: f32) -> Image<'_> {
        Image::new(&self.0[name]).fit_to_exact_size(Vec2::splat(size))
    }

    pub fn button(&self, ui: &mut egui::Ui, name: &str, label: &str) -> egui::Response {
        ui.add(egui::Button::image_and_text(self.image(name, 17.0), label))
    }

    pub fn heading(&self, ui: &mut egui::Ui, icon: &str, label: &str) {
        ui.horizontal(|ui| {
            ui.add(self.image(icon, 17.0));
            ui.label(RichText::new(label).strong().size(15.0));
        });
    }
}
