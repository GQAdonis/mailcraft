// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The runtime chain's icons (B11, 21.6), drawn in code on the 20-unit grid in one stroke, as the
//! rest of MailCraft's set: original artwork, no asset files. None of them is, resembles or is
//! traced from any vendor's mark: a runtime is told by a generic thing (a chip, a server, a
//! terminal), and a vendor's name appears only as text beside it.
//!
//! - `aiDevice`: a chip square with pins, a filled die in the accent.
//! - `aiProvider`: a small server stack with a link line.
//! - `aiPlan`: a cloud with a small key-hole circle.
//! - `aiTerminal`: a terminal frame with a prompt chevron and an underscore.
//! - `aiHub`: a centre node with three satellites.

use egui::{Color32, Painter, Pos2, Rect, Stroke, pos2, vec2};

/// The names this module draws.
pub(crate) const NAMES: [&str; 5] = ["aiDevice", "aiProvider", "aiPlan", "aiTerminal", "aiHub"];

/// The grid fitted to the screen's pixels: a whole-pixel origin and a stroke a whole number of
/// pixels wide, never less than one, so that a 20 pt icon at 100 % is sharp.
struct Grid<'a> {
    p: &'a Painter,
    ppp: f32,
    o: Pos2,
    k: f32,
    w: f32,
}

impl<'a> Grid<'a> {
    fn new(p: &'a Painter, r: Rect) -> Option<Grid<'a>> {
        let ppp = p.pixels_per_point();
        let size = r.width().min(r.height());
        if !(size > 0.0 && size.is_finite() && ppp > 0.0 && ppp.is_finite()) {
            return None;
        }
        let k = size * ppp / 20.0;
        let corner = r.center() - vec2(size, size) / 2.0;
        Some(Grid { p, ppp, o: pos2((corner.x * ppp).round(), (corner.y * ppp).round()), k, w: k.round().max(1.0) })
    }

    /// A grid point, snapped so that a straight stroke of this width is sharp, in points.
    fn at(&self, x: f32, y: f32) -> Pos2 {
        let snap = |v: f32| if self.w as i32 % 2 == 1 { (v - 0.5).round() + 0.5 } else { v.round() };
        pos2(snap(self.o.x + x * self.k) / self.ppp, snap(self.o.y + y * self.k) / self.ppp)
    }

    fn stroke(&self, c: Color32) -> Stroke {
        Stroke::new(self.w / self.ppp, c)
    }

    fn line(&self, pts: &[(f32, f32)], c: Color32) {
        let v: Vec<Pos2> = pts.iter().map(|&(x, y)| self.at(x, y)).collect();
        self.p.add(egui::Shape::line(v, self.stroke(c)));
    }

    fn rect(&self, x0: f32, y0: f32, x1: f32, y1: f32, round: f32, c: Color32) {
        let r = Rect::from_two_pos(self.at(x0, y0), self.at(x1, y1));
        self.p.rect_stroke(r, round * self.k / self.ppp, self.stroke(c), egui::StrokeKind::Middle);
    }

    fn fill(&self, x0: f32, y0: f32, x1: f32, y1: f32, c: Color32) {
        let r = Rect::from_two_pos(self.at(x0, y0), self.at(x1, y1));
        self.p.rect_filled(r, 0.5 * self.k / self.ppp, c);
    }

    fn circle(&self, x: f32, y: f32, radius: f32, c: Color32) {
        let centre = pos2((self.o.x + x * self.k) / self.ppp, (self.o.y + y * self.k) / self.ppp);
        self.p.circle_stroke(centre, radius * self.k / self.ppp, self.stroke(c));
    }

    fn dot(&self, x: f32, y: f32, radius: f32, c: Color32) {
        let centre = pos2((self.o.x + x * self.k) / self.ppp, (self.o.y + y * self.k) / self.ppp);
        self.p.circle_filled(centre, radius * self.k / self.ppp, c);
    }

    /// An arc round (`x`, `y`) from `from` to `to` degrees (0 is to the right, clockwise).
    fn arc(&self, x: f32, y: f32, radius: f32, from: f32, to: f32, pts: &mut Vec<Pos2>) {
        let steps = 12;
        for i in 0..=steps {
            let a = (from + (to - from) * i as f32 / steps as f32).to_radians();
            pts.push(pos2((self.o.x + (x + radius * a.cos()) * self.k) / self.ppp, (self.o.y + (y + radius * a.sin()) * self.k) / self.ppp));
        }
    }
}

/// Paint `name` into `r` in ink `c`, with `accent` on the one part that marks it. `false` for a
/// name this module doesn't draw.
pub(crate) fn paint(p: &Painter, r: Rect, name: &str, c: Color32, accent: Color32) -> bool {
    if !NAMES.contains(&name) {
        return false;
    }
    let Some(g) = Grid::new(p, r) else { return true };
    match name {
        "aiDevice" => {
            g.rect(5.5, 5.5, 14.5, 14.5, 1.0, c);
            for at in [8.5, 11.5] {
                g.line(&[(at, 2.5), (at, 5.5)], c);
                g.line(&[(at, 14.5), (at, 17.5)], c);
                g.line(&[(2.5, at), (5.5, at)], c);
                g.line(&[(14.5, at), (17.5, at)], c);
            }
            g.fill(8.0, 8.0, 12.0, 12.0, accent);
        }
        "aiProvider" => {
            g.rect(3.5, 3.5, 16.5, 8.5, 1.0, c);
            g.rect(3.5, 10.5, 16.5, 15.5, 1.0, c);
            g.dot(6.5, 6.0, 0.9, accent);
            g.dot(6.5, 13.0, 0.9, accent);
            g.line(&[(10.0, 15.5), (10.0, 18.0)], c);
            g.line(&[(5.5, 18.0), (14.5, 18.0)], c);
        }
        "aiPlan" => {
            let mut pts = Vec::new();
            g.arc(5.5, 12.5, 3.0, 90.0, 270.0, &mut pts);
            g.arc(10.0, 9.5, 4.5, 180.0, 360.0, &mut pts);
            g.arc(14.5, 12.5, 3.0, 270.0, 450.0, &mut pts);
            if let Some(first) = pts.first().copied() {
                pts.push(first);
            }
            p.add(egui::Shape::line(pts, g.stroke(c)));
            g.circle(10.0, 11.5, 1.6, accent);
            g.line(&[(10.0, 13.1), (10.0, 14.5)], accent);
        }
        "aiTerminal" => {
            g.rect(2.5, 3.5, 17.5, 16.5, 1.5, c);
            g.line(&[(5.5, 7.5), (8.5, 10.0), (5.5, 12.5)], accent);
            g.line(&[(10.5, 12.5), (14.5, 12.5)], c);
        }
        "aiHub" => {
            g.circle(10.0, 10.5, 2.5, accent);
            for (x, y) in [(10.0, 3.5), (4.0, 15.5), (16.0, 15.5)] {
                g.circle(x, y, 1.75, c);
            }
            g.line(&[(10.0, 5.25), (10.0, 8.0)], c);
            g.line(&[(5.4, 14.3), (8.0, 12.0)], c);
            g.line(&[(14.6, 14.3), (12.0, 12.0)], c);
        }
        _ => {}
    }
    true
}
