//! Small vector-drawn icons for the toolbar buttons.
//!
//! egui's bundled fonts don't cover the Unicode symbols (⛶ ▤ ⌂ …), which would
//! otherwise render as empty boxes, so these are painted with the painter.

use eframe::egui::{self, pos2, vec2, Color32, Rect, Response, Sense, Shape, Stroke};

#[derive(Clone, Copy)]
pub enum Icon {
    Fullscreen,
    Filmstrip,
    Folder,
    Info,
    Play,
    Pause,
    Settings,
    Tracks,
}

pub fn icon_button(ui: &mut egui::Ui, icon: Icon, tooltip: &str) -> Response {
    let size = vec2(28.0, 24.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let painter = ui.painter();

    let color = if response.is_pointer_button_down_on() {
        Color32::from_rgb(0xf0, 0xf3, 0xf6)
    } else if response.hovered() {
        Color32::from_rgb(0xe6, 0xea, 0xf0)
    } else {
        Color32::from_rgb(0xb4, 0xbb, 0xc4)
    };
    if response.hovered() {
        painter.rect_filled(rect, 5.0, Color32::from_rgb(0x22, 0x27, 0x2f));
    }

    let c = rect.center();
    let s = Stroke::new(1.5, color);
    match icon {
        Icon::Fullscreen => {
            let r = Rect::from_center_size(c, vec2(7.0, 7.0));
            painter.rect_stroke(r, 1.0, s, egui::StrokeKind::Inside);
            // Outward arrows at each corner.
            let d = 3.2;
            let tl = r.min;
            painter.line_segment([tl, tl + vec2(-d, 0.0)], s);
            painter.line_segment([tl, tl + vec2(0.0, -d)], s);
            let tr = pos2(r.max.x, r.min.y);
            painter.line_segment([tr, tr + vec2(d, 0.0)], s);
            painter.line_segment([tr, tr + vec2(0.0, -d)], s);
            let bl = pos2(r.min.x, r.max.y);
            painter.line_segment([bl, bl + vec2(-d, 0.0)], s);
            painter.line_segment([bl, bl + vec2(0.0, d)], s);
            let br = r.max;
            painter.line_segment([br, br + vec2(d, 0.0)], s);
            painter.line_segment([br, br + vec2(0.0, d)], s);
        }
        Icon::Filmstrip => {
            let w = 4.5;
            let gap = 2.0;
            let total = 3.0 * w + 2.0 * gap;
            let x0 = c.x - total / 2.0;
            for i in 0..3 {
                let frame =
                    Rect::from_min_size(pos2(x0 + i as f32 * (w + gap), c.y - 3.5), vec2(w, 7.0));
                painter.rect_stroke(
                    frame,
                    1.0,
                    Stroke::new(1.2, color),
                    egui::StrokeKind::Inside,
                );
            }
        }
        Icon::Folder => {
            let body = Rect::from_center_size(c + vec2(0.0, 1.0), vec2(16.0, 10.0));
            painter.rect_stroke(body, 2.0, s, egui::StrokeKind::Inside);
            let tab = Rect::from_min_max(
                pos2(body.min.x - 1.0, body.min.y - 4.0),
                pos2(body.min.x + 5.0, body.min.y),
            );
            painter.rect_stroke(tab, 1.0, Stroke::new(1.3, color), egui::StrokeKind::Inside);
        }
        Icon::Info => {
            painter.circle_stroke(c, 7.0, Stroke::new(1.5, color));
            painter.circle_filled(c + vec2(0.0, -3.0), 1.1, color);
            painter.line_segment([c + vec2(0.0, -0.5), c + vec2(0.0, 4.0)], s);
        }
        Icon::Play => {
            let pts = vec![
                c + vec2(-3.0, -4.5),
                c + vec2(-3.0, 4.5),
                c + vec2(4.5, 0.0),
            ];
            painter.add(Shape::convex_polygon(pts, color, Stroke::NONE));
        }
        Icon::Pause => {
            painter.rect_filled(
                Rect::from_center_size(c + vec2(-2.6, 0.0), vec2(2.2, 9.0)),
                1.0,
                color,
            );
            painter.rect_filled(
                Rect::from_center_size(c + vec2(2.6, 0.0), vec2(2.2, 9.0)),
                1.0,
                color,
            );
        }
        Icon::Settings => {
            let ys = [c.y - 3.5, c.y, c.y + 3.5];
            let knobs = [0.0, 3.0, -3.0];
            for (i, y) in ys.iter().enumerate() {
                painter.line_segment(
                    [pos2(c.x - 6.5, *y), pos2(c.x + 6.5, *y)],
                    Stroke::new(1.4, color),
                );
                painter.circle_filled(pos2(c.x + knobs[i], *y), 2.0, color);
            }
        }
        Icon::Tracks => {
            for dy in [-3.0f32, 3.0] {
                let y = c.y + dy;
                let arrow = vec![
                    pos2(c.x - 6.5, y - 2.5),
                    pos2(c.x - 6.5, y + 2.5),
                    pos2(c.x - 3.5, y),
                ];
                painter.add(Shape::convex_polygon(arrow, color, Stroke::NONE));
                painter.line_segment(
                    [pos2(c.x - 2.5, y), pos2(c.x + 6.5, y)],
                    Stroke::new(1.4, color),
                );
            }
        }
    }

    response.on_hover_text(tooltip)
}
