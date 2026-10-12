use std::f64::consts::{FRAC_PI_2, PI, TAU};

use blitz_dom::kurbo::{Affine, BezPath, PathEl, Point, Vec2};

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
            sweep = if sweep >= TAU {
                TAU
            } else {
                sweep.rem_euclid(TAU)
            };
        } else {
            sweep = if sweep <= -TAU {
                -TAU
            } else {
                -(-sweep).rem_euclid(TAU)
            };
        }
        let origin = start.rem_euclid(TAU);
        let matrix = transform
            * Affine::translate((x, y))
            * Affine::rotate(rotation)
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
            &[
                center.x,
                center.y,
                radius,
                radius,
                0.0,
                first,
                last,
                if cross > 0.0 { 1.0 } else { 0.0 },
            ],
            transform,
        );
    }

    fn round_rect(&mut self, n: &[f64], transform: Affine) {
        let (x, y, w, h) = (n[0], n[1], n[2], n[3]);
        let radii = [(n[4], n[5]), (n[6], n[7]), (n[8], n[9]), (n[10], n[11])];
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
        ]
        .into_iter()
        .enumerate()
        {
            let corner = (index + 1) % 4;
            let (rx, ry) = radii[corner];
            if rx == 0.0 || ry == 0.0 {
                let end = start + FRAC_PI_2;
                self.line_to(transform * Point::new(cx + rx * end.cos(), cy + ry * end.sin()));
            } else {
                self.ellipse(
                    &[cx, cy, rx, ry, 0.0, start, start + FRAC_PI_2, 0.0],
                    transform,
                );
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
                if self.current.is_none() {
                    self.move_to(point(0));
                }
                self.path.quad_to(point(0), point(2));
                self.current = Some(point(2));
            }
            "bezierCurveTo" => {
                if self.current.is_none() {
                    self.move_to(point(0));
                }
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
                if n[2] < 0.0 {
                    return Err("negative canvas arc radius".to_owned());
                }
                self.ellipse(&[n[0], n[1], n[2], n[2], 0.0, n[3], n[4], n[5]], transform);
            }
            "ellipse" => {
                if n[2] < 0.0 || n[3] < 0.0 {
                    return Err("negative canvas ellipse radius".to_owned());
                }
                self.ellipse(n, transform);
            }
            "arcTo" => {
                if n[4] < 0.0 {
                    return Err("negative canvas arcTo radius".to_owned());
                }
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

/// Build a standalone path from the immutable command records sent by JS.
pub(super) fn decode(commands: &[super::PathCommand]) -> Result<BezPath> {
    if commands.len() > super::PATH_LIMIT {
        return Err("canvas path limit reached in chuzz".to_owned());
    }
    let mut path = Path::default();
    for command in commands {
        numbers(&command.matrix, 6)?;
        if !path.command(&command.op, &command.n, Affine::new(command.matrix))? {
            return Err("invalid Path2D command".to_owned());
        }
        if path.path.elements().len() > super::PATH_LIMIT {
            return Err("canvas path limit reached in chuzz".to_owned());
        }
    }
    Ok(path.path)
}

fn svg_point(abs: bool, x: f64, y: f64, current: Point) -> Point {
    if abs {
        Point::new(x, y)
    } else {
        current + Vec2::new(x, y)
    }
}

fn svg_command(op: &str, n: Vec<f64>) -> super::PathCommand {
    super::PathCommand {
        op: op.to_owned(),
        n,
        matrix: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
    }
}

fn svg_arc(
    from: Point,
    to: Point,
    mut rx: f64,
    mut ry: f64,
    rotation: f64,
    large_arc: bool,
    sweep: bool,
) -> Option<Vec<f64>> {
    if from == to {
        return None;
    }
    rx = rx.abs();
    ry = ry.abs();
    if rx == 0.0 || ry == 0.0 {
        return None;
    }
    let phi = rotation.to_radians();
    let (sin_phi, cos_phi) = phi.sin_cos();
    let half = (from - to) * 0.5;
    let xp = cos_phi * half.x + sin_phi * half.y;
    let yp = -sin_phi * half.x + cos_phi * half.y;
    let scale = (xp * xp / (rx * rx) + yp * yp / (ry * ry)).sqrt().max(1.0);
    rx *= scale;
    ry *= scale;
    let rx2 = rx * rx;
    let ry2 = ry * ry;
    let xp2 = xp * xp;
    let yp2 = yp * yp;
    let denominator = rx2 * yp2 + ry2 * xp2;
    if denominator == 0.0 {
        return None;
    }
    let numerator = (rx2 * ry2 - rx2 * yp2 - ry2 * xp2).max(0.0);
    let sign = if large_arc == sweep { -1.0 } else { 1.0 };
    let coefficient = sign * (numerator / denominator).sqrt();
    let cxp = coefficient * rx * yp / ry;
    let cyp = coefficient * -ry * xp / rx;
    let center = Point::new(
        cos_phi * cxp - sin_phi * cyp + (from.x + to.x) * 0.5,
        sin_phi * cxp + cos_phi * cyp + (from.y + to.y) * 0.5,
    );
    let u = Vec2::new((xp - cxp) / rx, (yp - cyp) / ry);
    let v = Vec2::new((-xp - cxp) / rx, (-yp - cyp) / ry);
    let start = u.y.atan2(u.x);
    let mut end = start + u.cross(v).atan2(u.dot(v));
    if sweep && end < start {
        end += std::f64::consts::TAU;
    }
    if !sweep && end > start {
        end -= std::f64::consts::TAU;
    }
    Some(vec![
        center.x,
        center.y,
        rx,
        ry,
        phi,
        start,
        end,
        if sweep { 0.0 } else { 1.0 },
    ])
}

/// Parse SVG path data in Rust and normalize relative and smooth commands.
pub(super) fn svg_commands(value: &str) -> Result<Vec<super::PathCommand>> {
    use anyrender_svg::usvg::svgtypes::{PathParser, PathSegment};

    if value.len() > 1_048_576 {
        return Err("Path2D SVG input limit reached in chuzz".to_owned());
    }
    let mut output = Vec::new();
    let mut current = Point::new(0.0, 0.0);
    let mut start = Point::new(0.0, 0.0);
    let mut cubic_control = None;
    let mut quadratic_control = None;
    for item in PathParser::from(value) {
        let segment = item.map_err(|error| format!("invalid Path2D SVG data: {error}"))?;
        let command = match segment {
            PathSegment::MoveTo { abs, x, y } => {
                let point = svg_point(abs, x, y, current);
                start = point;
                current = point;
                cubic_control = None;
                quadratic_control = None;
                svg_command("moveTo", vec![point.x, point.y])
            }
            PathSegment::LineTo { abs, x, y } => {
                let point = svg_point(abs, x, y, current);
                current = point;
                cubic_control = None;
                quadratic_control = None;
                svg_command("lineTo", vec![point.x, point.y])
            }
            PathSegment::HorizontalLineTo { abs, x } => {
                let point = Point::new(if abs { x } else { current.x + x }, current.y);
                current = point;
                cubic_control = None;
                quadratic_control = None;
                svg_command("lineTo", vec![point.x, point.y])
            }
            PathSegment::VerticalLineTo { abs, y } => {
                let point = Point::new(current.x, if abs { y } else { current.y + y });
                current = point;
                cubic_control = None;
                quadratic_control = None;
                svg_command("lineTo", vec![point.x, point.y])
            }
            PathSegment::CurveTo {
                abs,
                x1,
                y1,
                x2,
                y2,
                x,
                y,
            } => {
                let first = svg_point(abs, x1, y1, current);
                let second = svg_point(abs, x2, y2, current);
                let end = svg_point(abs, x, y, current);
                current = end;
                cubic_control = Some(second);
                quadratic_control = None;
                svg_command(
                    "bezierCurveTo",
                    vec![first.x, first.y, second.x, second.y, end.x, end.y],
                )
            }
            PathSegment::SmoothCurveTo { abs, x2, y2, x, y } => {
                let first = cubic_control.map_or(current, |control: Point| {
                    Point::new(2.0 * current.x - control.x, 2.0 * current.y - control.y)
                });
                let second = svg_point(abs, x2, y2, current);
                let end = svg_point(abs, x, y, current);
                current = end;
                cubic_control = Some(second);
                quadratic_control = None;
                svg_command(
                    "bezierCurveTo",
                    vec![first.x, first.y, second.x, second.y, end.x, end.y],
                )
            }
            PathSegment::Quadratic { abs, x1, y1, x, y } => {
                let control = svg_point(abs, x1, y1, current);
                let end = svg_point(abs, x, y, current);
                current = end;
                quadratic_control = Some(control);
                cubic_control = None;
                svg_command("quadraticCurveTo", vec![control.x, control.y, end.x, end.y])
            }
            PathSegment::SmoothQuadratic { abs, x, y } => {
                let control = quadratic_control.map_or(current, |previous: Point| {
                    Point::new(2.0 * current.x - previous.x, 2.0 * current.y - previous.y)
                });
                let end = svg_point(abs, x, y, current);
                current = end;
                quadratic_control = Some(control);
                cubic_control = None;
                svg_command("quadraticCurveTo", vec![control.x, control.y, end.x, end.y])
            }
            PathSegment::EllipticalArc {
                abs,
                rx,
                ry,
                x_axis_rotation,
                large_arc,
                sweep,
                x,
                y,
            } => {
                let end = svg_point(abs, x, y, current);
                cubic_control = None;
                quadratic_control = None;
                let values = svg_arc(current, end, rx, ry, x_axis_rotation, large_arc, sweep);
                current = end;
                match values {
                    Some(values) => svg_command("ellipse", values),
                    None => svg_command("lineTo", vec![end.x, end.y]),
                }
            }
            PathSegment::ClosePath { .. } => {
                current = start;
                cubic_control = None;
                quadratic_control = None;
                svg_command("closePath", Vec::new())
            }
        };
        output.push(command);
        if output.len() > super::PATH_LIMIT {
            return Err("Path2D command limit reached in chuzz".to_owned());
        }
    }
    decode(&output)?;
    Ok(output)
}

fn midpoint(a: Point, b: Point) -> Point {
    Point::new((a.x + b.x) * 0.5, (a.y + b.y) * 0.5)
}

fn distance_to_line(point: Point, a: Point, b: Point) -> f64 {
    let edge = b - a;
    let length = edge.hypot();
    if length <= f64::EPSILON {
        return (point - a).hypot();
    }
    edge.cross(point - a).abs() / length
}

fn flatten_quad(a: Point, control: Point, b: Point, out: &mut Vec<Point>, depth: u8) {
    if depth >= 12 || distance_to_line(control, a, b) <= 0.1 {
        out.push(b);
        return;
    }
    let ac = midpoint(a, control);
    let cb = midpoint(control, b);
    let middle = midpoint(ac, cb);
    flatten_quad(a, ac, middle, out, depth + 1);
    flatten_quad(middle, cb, b, out, depth + 1);
}

fn flatten_cubic(a: Point, c1: Point, c2: Point, b: Point, out: &mut Vec<Point>, depth: u8) {
    if depth >= 12 || distance_to_line(c1, a, b).max(distance_to_line(c2, a, b)) <= 0.1 {
        out.push(b);
        return;
    }
    let a1 = midpoint(a, c1);
    let c12 = midpoint(c1, c2);
    let c2b = midpoint(c2, b);
    let a2 = midpoint(a1, c12);
    let b1 = midpoint(c12, c2b);
    let middle = midpoint(a2, b1);
    flatten_cubic(a, a1, a2, middle, out, depth + 1);
    flatten_cubic(middle, b1, c2b, b, out, depth + 1);
}

#[derive(Default)]
struct Contour {
    points: Vec<Point>,
    closed: bool,
}

fn contours(path: &BezPath) -> Vec<Contour> {
    let mut result = Vec::new();
    let mut active = Contour::default();
    let mut current = Point::new(0.0, 0.0);
    for element in path.elements() {
        match *element {
            PathEl::MoveTo(point) => {
                if !active.points.is_empty() {
                    result.push(active);
                }
                active = Contour {
                    points: vec![point],
                    closed: false,
                };
                current = point;
            }
            PathEl::LineTo(point) => {
                active.points.push(point);
                current = point;
            }
            PathEl::QuadTo(control, end) => {
                flatten_quad(current, control, end, &mut active.points, 0);
                current = end;
            }
            PathEl::CurveTo(first, second, end) => {
                flatten_cubic(current, first, second, end, &mut active.points, 0);
                current = end;
            }
            PathEl::ClosePath => {
                if let Some(first) = active.points.first().copied() {
                    if active.points.last() != Some(&first) {
                        active.points.push(first);
                    }
                    current = first;
                    active.closed = true;
                }
            }
        }
    }
    if !active.points.is_empty() {
        result.push(active);
    }
    result
}

fn on_segment(point: Point, a: Point, b: Point) -> bool {
    let edge = b - a;
    let relative = point - a;
    if edge.hypot2() <= f64::EPSILON {
        return relative.hypot2() <= 1.0e-14;
    }
    if edge.cross(relative).abs() > 1.0e-7 * edge.hypot().max(1.0) {
        return false;
    }
    let projection = relative.dot(edge);
    projection >= -1.0e-7 && projection <= edge.dot(edge) + 1.0e-7
}

pub(super) fn contains_fill(path: &BezPath, point: Point, fill: peniko::Fill) -> bool {
    let mut winding = 0i32;
    let mut crossings = 0usize;
    for contour in contours(path) {
        if contour.points.len() < 2 {
            continue;
        }
        for index in 0..contour.points.len() {
            let a = contour.points[index];
            let b = contour.points[(index + 1) % contour.points.len()];
            if on_segment(point, a, b) {
                return true;
            }
            if (a.y <= point.y && b.y > point.y) || (a.y > point.y && b.y <= point.y) {
                let side = (b - a).cross(point - a);
                if (b.y > a.y && side > 0.0) || (b.y < a.y && side < 0.0) {
                    winding += if b.y > a.y { 1 } else { -1 };
                    crossings += 1;
                }
            }
        }
    }
    if matches!(fill, peniko::Fill::EvenOdd) {
        crossings % 2 == 1
    } else {
        winding != 0
    }
}

fn triangle_contains(point: Point, a: Point, b: Point, c: Point) -> bool {
    let ab = (b - a).cross(point - a);
    let bc = (c - b).cross(point - b);
    let ca = (a - c).cross(point - c);
    (ab >= -1.0e-7 && bc >= -1.0e-7 && ca >= -1.0e-7)
        || (ab <= 1.0e-7 && bc <= 1.0e-7 && ca <= 1.0e-7)
}

fn line_contains(
    point: Point,
    a: Point,
    b: Point,
    radius: f64,
    cap: &str,
    first: bool,
    last: bool,
) -> bool {
    let edge = b - a;
    let length = edge.hypot();
    if length <= f64::EPSILON {
        return cap != "butt" && (point - a).hypot() <= radius;
    }
    let direction = edge / length;
    let along = (point - a).dot(direction);
    let across = direction.cross(point - a).abs();
    let lower = if first && cap == "square" {
        -radius
    } else {
        0.0
    };
    let upper = length + if last && cap == "square" { radius } else { 0.0 };
    if along >= lower && along <= upper && across <= radius {
        return true;
    }
    cap == "round"
        && ((first && (point - a).hypot() <= radius) || (last && (point - b).hypot() <= radius))
}

fn join_contains(
    point: Point,
    before: Point,
    at: Point,
    after: Point,
    radius: f64,
    join: &str,
    limit: f64,
) -> bool {
    let incoming = at - before;
    let outgoing = after - at;
    let (a_len, b_len) = (incoming.hypot(), outgoing.hypot());
    if a_len <= f64::EPSILON || b_len <= f64::EPSILON {
        return false;
    }
    let (a, b) = (incoming / a_len, outgoing / b_len);
    let turn = a.cross(b);
    if turn.abs() < 1.0e-10 {
        return false;
    }
    if join == "round" {
        return (point - at).hypot() <= radius;
    }
    let side = if turn > 0.0 { -1.0 } else { 1.0 };
    let p1 = at + Vec2::new(-a.y, a.x) * (radius * side);
    let p2 = at + Vec2::new(-b.y, b.x) * (radius * side);
    if join == "miter" {
        let tip = p1 + a * ((p2 - p1).cross(b) / turn);
        if (tip - at).hypot() <= radius * limit {
            return triangle_contains(point, p1, tip, p2) || triangle_contains(point, at, p1, p2);
        }
    }
    triangle_contains(point, at, p1, p2)
}

fn stroke_piece(points: &[Point], closed: bool, point: Point, state: &super::DrawState) -> bool {
    if points.is_empty() {
        return false;
    }
    let radius = state.width * 0.5;
    if points.len() == 1 {
        return state.cap != "butt" && (point - points[0]).hypot() <= radius;
    }
    let count = points.len() - 1;
    for index in 0..count {
        if line_contains(
            point,
            points[index],
            points[index + 1],
            radius,
            &state.cap,
            !closed && index == 0,
            !closed && index + 1 == count,
        ) {
            return true;
        }
    }
    for index in 1..count {
        if join_contains(
            point,
            points[index - 1],
            points[index],
            points[index + 1],
            radius,
            &state.join,
            state.miter,
        ) {
            return true;
        }
    }
    closed
        && points.len() >= 4
        && join_contains(
            point,
            points[points.len() - 2],
            points[0],
            points[1],
            radius,
            &state.join,
            state.miter,
        )
}

fn dashed_pieces(points: &[Point], pattern: &[f64], offset: f64) -> Vec<Vec<Point>> {
    if points.len() < 2 {
        return Vec::new();
    }
    let total: f64 = pattern.iter().sum();
    if total <= f64::EPSILON {
        return vec![points.to_vec()];
    }
    let mut index = 0usize;
    let mut phase = offset.rem_euclid(total);
    let mut guard = 0usize;
    while phase >= pattern[index] && guard < pattern.len() * 2 {
        if pattern[index] > 0.0 {
            phase -= pattern[index];
        }
        index = (index + 1) % pattern.len();
        guard += 1;
    }
    let mut remaining = (pattern[index] - phase).max(0.0);
    let mut active = index.is_multiple_of(2);
    let mut pieces = Vec::new();
    let mut piece = Vec::new();
    for segment in points.windows(2) {
        let a = segment[0];
        let edge = segment[1] - a;
        let length = edge.hypot();
        if length <= f64::EPSILON {
            continue;
        }
        let direction = edge / length;
        let mut traveled = 0.0;
        while traveled < length {
            if remaining <= f64::EPSILON {
                if piece.len() > 1 {
                    pieces.push(std::mem::take(&mut piece));
                }
                index = (index + 1) % pattern.len();
                active = index.is_multiple_of(2);
                remaining = pattern[index];
                if remaining <= f64::EPSILON {
                    continue;
                }
            }
            let step = remaining.min(length - traveled);
            let from = a + direction * traveled;
            let to = a + direction * (traveled + step);
            if active {
                if piece.is_empty() {
                    piece.push(from);
                }
                piece.push(to);
            }
            traveled += step;
            remaining -= step;
        }
    }
    if piece.len() > 1 {
        pieces.push(piece);
    }
    pieces
}

pub(super) fn contains_stroke(
    path: &BezPath,
    point: Point,
    state: &super::DrawState,
) -> Result<bool> {
    state.stroke()?;
    for contour in contours(path) {
        if state.dash.is_empty() || state.dash.iter().all(|length| *length == 0.0) {
            if stroke_piece(&contour.points, contour.closed, point, state) {
                return Ok(true);
            }
        } else {
            for piece in dashed_pieces(&contour.points, &state.dash, state.dash_offset) {
                if stroke_piece(&piece, false, point, state) {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}
