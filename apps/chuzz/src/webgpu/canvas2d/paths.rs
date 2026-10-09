use std::f64::consts::{FRAC_PI_2, PI, TAU};

use blitz_dom::kurbo::{Affine, BezPath, Point, Vec2};

use super::{Result, numbers};

#[derive(Default)]
pub(super) struct Path {
    pub path: BezPath,
    current: Option<Point>,
    start: Option<Point>,
}

pub(super) fn rectangle(n: &[f64]) -> BezPath {
    let mut path = BezPath::new();
    path.move_to((n[0], n[1]));
    path.line_to((n[0] + n[2], n[1]));
    path.line_to((n[0] + n[2], n[1] + n[3]));
    path.line_to((n[0], n[1] + n[3]));
    path.close_path();
    path
}

impl Path {
    fn move_to(&mut self, point: Point) {
        self.path.move_to(point);
        self.current = Some(point);
        self.start = Some(point);
    }

    fn line_to(&mut self, point: Point) {
        if self.current.is_none() {
            self.move_to(point);
        } else {
            self.path.line_to(point);
            self.current = Some(point);
        }
    }

    fn close(&mut self) {
        if self.current.is_some() {
            self.path.close_path();
            self.current = self.start;
        }
    }

    fn ellipse(&mut self, n: &[f64], transform: Affine) {
        let [x, y, rx, ry, rotation, start, end, ccw] =
            [n[0], n[1], n[2], n[3], n[4], n[5], n[6], n[7]];
        let mut sweep = end - start;
        if ccw == 0.0 {
            sweep = if sweep >= TAU { TAU } else { sweep.rem_euclid(TAU) };
        } else {
            sweep = if sweep <= -TAU { -TAU } else { -(-sweep).rem_euclid(TAU) };
        }
        let origin = start.rem_euclid(TAU);
        let matrix = transform * Affine::translate((x, y)) * Affine::rotate(rotation)
            * Affine::scale_non_uniform(rx, ry);
        let first = matrix * Point::new(origin.cos(), origin.sin());
        self.line_to(first);
        if sweep == 0.0 {
            return;
        }
        let segments = (sweep.abs() / FRAC_PI_2).ceil() as usize;
        let step = sweep / segments as f64;
        for index in 0..segments {
            let a = origin + step * index as f64;
            let b = a + step;
            let k = 4.0 / 3.0 * (step / 4.0).tan();
            let c1 = matrix * Point::new(a.cos() - k * a.sin(), a.sin() + k * a.cos());
            let c2 = matrix * Point::new(b.cos() + k * b.sin(), b.sin() - k * b.cos());
            let end = matrix * Point::new(b.cos(), b.sin());
            self.path.curve_to(c1, c2, end);
            self.current = Some(end);
        }
    }

    fn arc_to(&mut self, n: &[f64], transform: Affine) {
        let p1 = Point::new(n[0], n[1]);
        let p2 = Point::new(n[2], n[3]);
        let radius = n[4];
        let Some(current) = self.current else {
            self.move_to(transform * p1);
            return;
        };
        if transform.determinant().abs() < 1.0e-20 {
            self.line_to(transform * p1);
            return;
        }
        let p0 = transform.inverse() * current;
        let a = p0 - p1;
        let b = p2 - p1;
        let cross = a.cross(b);
        let lengths = a.hypot() * b.hypot();
        if radius == 0.0 || lengths == 0.0 || cross.abs() < 1.0e-12 * lengths {
            self.line_to(transform * p1);
            return;
        }
        let u = a / a.hypot();
        let v = b / b.hypot();
        let angle = u.dot(v).clamp(-1.0, 1.0).acos();
        let distance = radius / (angle * 0.5).tan();
        let t1 = p1 + u * distance;
        let t2 = p1 + v * distance;
        let normal = Vec2::new(-u.y, u.x) * if cross > 0.0 { 1.0 } else { -1.0 };
        let center = t1 + normal * radius;
        self.line_to(transform * t1);
        let first = (t1.y - center.y).atan2(t1.x - center.x);
        let last = (t2.y - center.y).atan2(t2.x - center.x);
        self.ellipse(
            &[center.x, center.y, radius, radius, 0.0, first, last,
                if cross > 0.0 { 1.0 } else { 0.0 }],
            transform,
        );
    }

    fn round_rect(&mut self, n: &[f64], transform: Affine) {
        let (x, y, w, h) = (n[0], n[1], n[2], n[3]);
        let radii = [
            (n[4], n[5]), (n[6], n[7]), (n[8], n[9]), (n[10], n[11]),
        ];
        let ratios = [
            w / (radii[0].0 + radii[1].0),
            w / (radii[3].0 + radii[2].0),
            h / (radii[0].1 + radii[3].1),
            h / (radii[1].1 + radii[2].1),
        ];
        let factor = ratios.into_iter().fold(1.0, f64::min).max(0.0);
        let radii = radii.map(|(rx, ry)| (rx * factor, ry * factor));
        self.move_to(transform * Point::new(x + radii[0].0, y));
        self.line_to(transform * Point::new(x + w - radii[1].0, y));
        for (index, (cx, cy, start)) in [
            (x + w - radii[1].0, y + radii[1].1, -FRAC_PI_2),
            (x + w - radii[2].0, y + h - radii[2].1, 0.0),
            (x + radii[3].0, y + h - radii[3].1, FRAC_PI_2),
            (x + radii[0].0, y + radii[0].1, PI),
        ].into_iter().enumerate() {
            let corner = (index + 1) % 4;
            let (rx, ry) = radii[corner];
            if rx == 0.0 || ry == 0.0 {
                let end = start + FRAC_PI_2;
                self.line_to(transform * Point::new(cx + rx * end.cos(), cy + ry * end.sin()));
            } else {
                self.ellipse(&[cx, cy, rx, ry, 0.0, start, start + FRAC_PI_2, 0.0], transform);
            }
            let next = match index {
                0 => Point::new(x + w, y + h - radii[2].1),
                1 => Point::new(x + radii[3].0, y + h),
                2 => Point::new(x, y + radii[0].1),
                _ => Point::new(x + radii[0].0, y),
            };
            self.line_to(transform * next);
        }
        self.close();
    }

    pub fn command(&mut self, op: &str, n: &[f64], transform: Affine) -> Result<bool> {
        let count = match op {
            "beginPath" | "closePath" => 0,
            "moveTo" | "lineTo" => 2,
            "rect" | "quadraticCurveTo" => 4,
            "arcTo" => 5,
            "bezierCurveTo" | "arc" => 6,
            "ellipse" => 8,
            "roundRect" => 12,
            _ => return Ok(false),
        };
        numbers(n, count)?;
        let point = |offset| transform * Point::new(n[offset], n[offset + 1]);
        match op {
            "beginPath" => *self = Self::default(),
            "closePath" => self.close(),
            "moveTo" => self.move_to(point(0)),
            "lineTo" => self.line_to(point(0)),
            "quadraticCurveTo" => {
                if self.current.is_none() { self.move_to(point(0)); }
                self.path.quad_to(point(0), point(2));
                self.current = Some(point(2));
            }
            "bezierCurveTo" => {
                if self.current.is_none() { self.move_to(point(0)); }
                self.path.curve_to(point(0), point(2), point(4));
                self.current = Some(point(4));
            }
            "rect" => {
                self.move_to(point(0));
                self.line_to(transform * Point::new(n[0] + n[2], n[1]));
                self.line_to(transform * Point::new(n[0] + n[2], n[1] + n[3]));
                self.line_to(transform * Point::new(n[0], n[1] + n[3]));
                self.close();
            }
            "arc" => {
                if n[2] < 0.0 { return Err("negative canvas arc radius".to_owned()); }
                self.ellipse(&[n[0], n[1], n[2], n[2], 0.0, n[3], n[4], n[5]], transform);
            }
            "ellipse" => {
                if n[2] < 0.0 || n[3] < 0.0 {
                    return Err("negative canvas ellipse radius".to_owned());
                }
                self.ellipse(n, transform);
            }
            "arcTo" => {
                if n[4] < 0.0 { return Err("negative canvas arcTo radius".to_owned()); }
                self.arc_to(n, transform);
            }
            "roundRect" => {
                if n[2] < 0.0 || n[3] < 0.0 || n[4..].iter().any(|radius| *radius < 0.0) {
                    return Err("invalid canvas roundRect radius".to_owned());
                }
                self.round_rect(n, transform);
            }
            _ => unreachable!(),
        }
        Ok(true)
    }
}

