//! Procedural meshes (flat-shaded, low-poly) for SYNTHRACE. Pure and native-testable.
//!
//! Local space: x right, y up, z forward (toward the horizon). Units: metres.

use core::f32::consts::{PI, TAU};

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    /// Linear albedo.
    pub color: [f32; 3],
    /// Self-illumination multiplier of `color` (HDR; > 1 blooms).
    pub emissive: f32,
}

#[derive(Default, Clone, Debug)]
pub struct Mesh {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
}

type V3 = [f32; 3];

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn mul(a: V3, s: f32) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn normalize(a: V3) -> V3 {
    let l = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
    if l > 1e-9 { mul(a, 1.0 / l) } else { [0.0, 1.0, 0.0] }
}

#[derive(Clone, Copy)]
pub struct Material {
    pub color: V3,
    pub emissive: f32,
}

impl Mesh {
    /// Flat-shaded triangle (counter-clockwise seen from the front).
    pub fn tri(&mut self, a: V3, b: V3, c: V3, m: Material) {
        let n = normalize(cross(sub(b, a), sub(c, a)));
        let base = self.vertices.len() as u32;
        for p in [a, b, c] {
            self.vertices.push(Vertex { pos: p, normal: n, color: m.color, emissive: m.emissive });
        }
        self.indices.extend_from_slice(&[base, base + 1, base + 2]);
    }

    /// Flat-shaded quad a-b-c-d (counter-clockwise).
    pub fn quad(&mut self, a: V3, b: V3, c: V3, d: V3, m: Material) {
        let n = normalize(cross(sub(c, a), sub(d, b)));
        let base = self.vertices.len() as u32;
        for p in [a, b, c, d] {
            self.vertices.push(Vertex { pos: p, normal: n, color: m.color, emissive: m.emissive });
        }
        self.indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    /// Axis-aligned box from min to max corner.
    pub fn cuboid(&mut self, min: V3, max: V3, m: Material) {
        let [x0, y0, z0] = min;
        let [x1, y1, z1] = max;
        self.quad([x0, y0, z0], [x0, y1, z0], [x1, y1, z0], [x1, y0, z0], m); // -z
        self.quad([x1, y0, z1], [x1, y1, z1], [x0, y1, z1], [x0, y0, z1], m); // +z
        self.quad([x0, y0, z1], [x0, y1, z1], [x0, y1, z0], [x0, y0, z0], m); // -x
        self.quad([x1, y0, z0], [x1, y1, z0], [x1, y1, z1], [x1, y0, z1], m); // +x
        self.quad([x0, y1, z0], [x0, y1, z1], [x1, y1, z1], [x1, y1, z0], m); // +y
        self.quad([x0, y0, z1], [x0, y0, z0], [x1, y0, z0], [x1, y0, z1], m); // -y
    }

    pub fn append(&mut self, other: &Mesh) {
        let base = self.vertices.len() as u32;
        self.vertices.extend_from_slice(&other.vertices);
        self.indices.extend(other.indices.iter().map(|i| i + base));
    }

    pub fn bounds(&self) -> (V3, V3) {
        let mut lo = [f32::MAX; 3];
        let mut hi = [f32::MIN; 3];
        for v in &self.vertices {
            for k in 0..3 {
                lo[k] = lo[k].min(v.pos[k]);
                hi[k] = hi[k].max(v.pos[k]);
            }
        }
        (lo, hi)
    }
}

/// Player car colours.
pub struct CarStyle {
    pub body: V3,
    pub glass: V3,
    pub tail_light: V3,
    pub neon: V3,
}

pub const CAR_DEFAULT: CarStyle =
    CarStyle { body: [0.045, 0.008, 0.075], glass: [0.015, 0.02, 0.05], tail_light: [1.0, 0.02, 0.12], neon: [0.05, 0.85, 1.0] };

pub const CAR_LENGTH: f32 = 4.45;

/// Low-poly wedge sports car, flat shaded, ~200 triangles. Origin at ground level, centred.
pub fn car(style: &CarStyle) -> Mesh {
    // z, half width, bottom, shoulder, roof, roof half width
    const ST: [[f32; 6]; 8] = [
        [0.00, 0.86, 0.30, 0.78, 0.80, 0.70],
        [0.15, 0.95, 0.22, 0.92, 0.94, 0.80],
        [0.90, 0.96, 0.20, 0.94, 0.98, 0.78],
        [1.55, 0.96, 0.20, 0.93, 1.22, 0.62],
        [2.45, 0.96, 0.20, 0.90, 1.26, 0.60],
        [3.05, 0.95, 0.20, 0.86, 0.86, 0.72],
        [3.95, 0.90, 0.22, 0.72, 0.74, 0.70],
        [4.45, 0.80, 0.30, 0.52, 0.54, 0.62],
    ];
    let zc = CAR_LENGTH * 0.5;
    let body = Material { color: style.body, emissive: 0.0 };
    let glass = Material { color: style.glass, emissive: 0.15 };
    let mut m = Mesh::default();
    shadow(&mut m, 1.08, zc);
    hull(&mut m, &ST, zc, body, glass, 3..=4, &[2, 4]);
    // HDR tail-light bar across the rear face, plus a slim centre light.
    let tl = Material { color: style.tail_light, emissive: 7.0 };
    m.cuboid([-0.78, 0.60, -zc - 0.03], [0.78, 0.70, -zc + 0.01], tl);
    // Neon shoulder strips along both sides.
    let neon = Material { color: style.neon, emissive: 3.0 };
    for side in [-1.0f32, 1.0] {
        let x = side * 0.965;
        let (xa, xb) = if side < 0.0 { (x - 0.012, x) } else { (x, x + 0.012) };
        m.cuboid([xa, 0.50, 0.2 - zc], [xb, 0.54, 4.1 - zc], neon);
    }
    // Wheels: octagonal prisms.
    let tyre = Material { color: [0.012, 0.012, 0.02], emissive: 0.0 };
    let rim = Material { color: style.neon, emissive: 1.2 };
    for (wx, wz) in [(-0.9, 0.8), (0.9, 0.8), (-0.9, 3.55), (0.9, 3.55)] {
        wheel(&mut m, [wx, 0.33, wz - zc], 0.33, 0.26, tyre, rim);
    }
    m
}

/// Contact shadow: a flat quad just above the road; negative emissive marks it as a shadow.
fn shadow(m: &mut Mesh, hw: f32, hl: f32) {
    let sh = Material { color: [0.0, 0.0, 0.0], emissive: -1.0 };
    let (w, l) = (hw + 0.12, hl + 0.2);
    m.quad([-w, 0.015, -l - 0.05], [-w, 0.015, l], [w, 0.015, l], [w, 0.015, -l - 0.05], sh);
}

/// Lofted 8-sided hull through `st` stations `[z, half width, bottom, shoulder, roof, roof half width]`.
/// Edges 2/4 within `side_glass` segments and edge 3 on `roof_glass` segments use the glass material.
fn hull(
    m: &mut Mesh,
    st: &[[f32; 6]],
    zc: f32,
    body: Material,
    glass: Material,
    side_glass: core::ops::RangeInclusive<usize>,
    roof_glass: &[usize],
) {
    let ring = |s: &[f32; 6]| -> [V3; 8] {
        let [z, hw, b, sh, roof, rhw] = *s;
        let z = z - zc;
        let low = b + (sh - b) * 0.45;
        [
            [-hw * 0.88, b, z],
            [-hw, low, z],
            [-hw, sh, z],
            [-rhw, roof, z],
            [rhw, roof, z],
            [hw, sh, z],
            [hw, low, z],
            [hw * 0.88, b, z],
        ]
    };
    for i in 0..st.len() - 1 {
        let a = ring(&st[i]);
        let b = ring(&st[i + 1]);
        for e in 0..8 {
            let f = (e + 1) % 8;
            let is_glass = ((e == 2 || e == 4) && side_glass.contains(&i)) || (e == 3 && roof_glass.contains(&i));
            m.quad(a[e], b[e], b[f], a[f], if is_glass { glass } else { body });
        }
    }
    let rear = ring(&st[0]);
    let front = ring(&st[st.len() - 1]);
    for k in 1..7 {
        m.tri(rear[0], rear[k + 1], rear[k], body);
        m.tri(front[0], front[k], front[k + 1], body);
    }
}

/// Emissive marker codes interpreted per instance by the mesh shader.
pub mod marker {
    /// Brake light: dim running light, bright when the instance's brake value is 1.
    pub const BRAKE: f32 = 50.0;
    /// Left / right indicator: blink when the instance's indicator signal points that way.
    pub const INDICATOR_LEFT: f32 = 51.0;
    pub const INDICATOR_RIGHT: f32 = 52.0;
}

fn rear_lights(m: &mut Mesh, hw: f32, z: f32, y: f32) {
    let brake = Material { color: [1.0, 0.03, 0.08], emissive: marker::BRAKE };
    let amber = [1.0, 0.42, 0.04];
    for side in [-1.0f32, 1.0] {
        let (x0, x1) = (side * (hw - 0.42), side * (hw - 0.08));
        m.cuboid([x0.min(x1), y, z - 0.03], [x0.max(x1), y + 0.13, z + 0.01], brake);
        let code = if side < 0.0 { marker::INDICATOR_LEFT } else { marker::INDICATOR_RIGHT };
        let ind = Material { color: amber, emissive: code };
        let (i0, i1) = (side * (hw - 0.08), side * (hw + 0.02));
        m.cuboid([i0.min(i1), y - 0.02, z - 0.03], [i0.max(i1), y + 0.15, z + 0.01], ind);
    }
}

/// Traffic vehicle archetypes (dimensions match `sim::Archetype::half_extents`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VehicleKind {
    Coupe,
    Van,
    Truck,
    Sports,
}

const COUPE_STATIONS: [[f32; 6]; 8] = [
    [0.00, 0.84, 0.32, 0.80, 0.82, 0.70],
    [0.20, 0.92, 0.24, 0.92, 0.95, 0.80],
    [0.85, 0.92, 0.22, 0.95, 1.00, 0.80],
    [1.35, 0.92, 0.22, 0.95, 1.42, 0.72],
    [2.70, 0.92, 0.22, 0.93, 1.45, 0.72],
    [3.25, 0.92, 0.22, 0.90, 0.95, 0.80],
    [4.05, 0.88, 0.24, 0.80, 0.82, 0.78],
    [4.40, 0.80, 0.32, 0.62, 0.62, 0.70],
];

const SPORTS_STATIONS: [[f32; 6]; 8] = [
    [0.00, 0.86, 0.30, 0.76, 0.78, 0.70],
    [0.15, 0.95, 0.22, 0.88, 0.90, 0.80],
    [1.00, 0.95, 0.20, 0.90, 0.94, 0.78],
    [1.70, 0.95, 0.20, 0.88, 1.12, 0.60],
    [2.40, 0.95, 0.20, 0.86, 1.15, 0.58],
    [3.00, 0.93, 0.20, 0.80, 0.80, 0.72],
    [4.00, 0.88, 0.22, 0.66, 0.68, 0.70],
    [4.40, 0.78, 0.28, 0.48, 0.50, 0.60],
];

const VAN_STATIONS: [[f32; 6]; 6] = [
    [0.00, 0.98, 0.30, 1.90, 2.02, 0.92],
    [0.10, 1.02, 0.26, 1.95, 2.06, 0.96],
    [3.60, 1.02, 0.26, 1.95, 2.06, 0.96],
    [4.20, 1.00, 0.26, 1.40, 1.90, 0.92],
    [4.75, 0.98, 0.28, 1.00, 1.05, 0.90],
    [5.00, 0.92, 0.32, 0.85, 0.86, 0.86],
];

pub fn vehicle(kind: VehicleKind) -> Mesh {
    let mut m = Mesh::default();
    let tyre = Material { color: [0.012, 0.012, 0.02], emissive: 0.0 };
    let glass = Material { color: [0.015, 0.02, 0.05], emissive: 0.12 };
    match kind {
        VehicleKind::Coupe | VehicleKind::Sports => {
            let (st, body, neon) = if kind == VehicleKind::Coupe {
                (&COUPE_STATIONS, [0.025, 0.07, 0.09], [0.9, 0.25, 1.0])
            } else {
                (&SPORTS_STATIONS, [0.11, 0.01, 0.045], [1.0, 0.75, 0.1])
            };
            let zc = 2.2;
            let hw = st.iter().map(|s| s[1]).fold(0.0, f32::max);
            shadow(&mut m, hw, zc);
            hull(&mut m, st, zc, Material { color: body, emissive: 0.0 }, glass, 3..=3, &[2, 4]);
            rear_lights(&mut m, hw * 0.9, -zc, 0.58);
            let n = Material { color: neon, emissive: 2.2 };
            for side in [-1.0f32, 1.0] {
                let x = side * (hw + 0.004);
                m.cuboid([x.min(x + side * 0.012), 0.46, 0.3 - zc], [x.max(x + side * 0.012), 0.49, 4.0 - zc], n);
            }
            for (wx, wz) in [(-hw + 0.06, 0.8), (hw - 0.06, 0.8), (-hw + 0.06, 3.5), (hw - 0.06, 3.5)] {
                wheel(&mut m, [wx, 0.32, wz - zc], 0.32, 0.24, tyre, tyre);
            }
        }
        VehicleKind::Van => {
            let zc = 2.5;
            shadow(&mut m, 1.02, zc);
            hull(&mut m, &VAN_STATIONS, zc, Material { color: [0.07, 0.055, 0.11], emissive: 0.0 }, glass, 3..=3, &[3]);
            rear_lights(&mut m, 0.98, -zc, 0.75);
            let n = Material { color: [0.1, 0.9, 0.8], emissive: 1.8 };
            m.cuboid([-0.75, 1.82, -zc - 0.03], [0.75, 1.86, -zc + 0.01], n);
            for (wx, wz) in [(-0.97, 0.9), (0.97, 0.9), (-0.97, 4.0), (0.97, 4.0)] {
                wheel(&mut m, [wx, 0.36, wz - zc], 0.36, 0.26, tyre, tyre);
            }
        }
        VehicleKind::Truck => {
            let zc = 4.6;
            shadow(&mut m, 1.25, zc);
            let trailer = Material { color: [0.035, 0.03, 0.06], emissive: 0.0 };
            let cab = Material { color: [0.09, 0.02, 0.06], emissive: 0.0 };
            m.cuboid([-1.25, 1.0, -zc], [1.25, 3.6, 7.0 - zc], trailer);
            m.cuboid([-1.15, 0.6, 7.25 - zc], [1.15, 2.9, 9.2 - zc], cab);
            m.cuboid([-1.0, 1.9, 9.2 - zc], [1.0, 2.7, 9.21 - zc], glass);
            let n = Material { color: [1.0, 0.12, 0.6], emissive: 2.4 };
            for side in [-1.0f32, 1.0] {
                let x = side * 1.252;
                m.cuboid([x.min(x + side * 0.01), 3.45, -zc + 0.1], [x.max(x + side * 0.01), 3.52, 6.9 - zc], n);
            }
            m.cuboid([-1.2, 3.45, -zc - 0.02], [1.2, 3.52, -zc - 0.005], n);
            m.cuboid([-1.25, 0.55, -zc - 0.1], [1.25, 0.75, -zc + 0.2], Material { color: [0.05, 0.05, 0.06], emissive: 0.0 });
            rear_lights(&mut m, 1.2, -zc - 0.1, 0.8);
            for (wx, wz) in [(-1.12, 0.8), (1.12, 0.8), (-1.12, 2.1), (1.12, 2.1), (-1.08, 7.9), (1.08, 7.9)] {
                wheel(&mut m, [wx, 0.5, wz - zc], 0.5, 0.3, tyre, tyre);
            }
        }
    }
    m
}

fn wheel(m: &mut Mesh, c: V3, r: f32, w: f32, tyre: Material, rim: Material) {
    let n = 8;
    let outer = if c[0] < 0.0 { -1.0 } else { 1.0 };
    let pt = |k: usize, x: f32| -> V3 {
        let a = k as f32 / n as f32 * TAU;
        [x, c[1] + r * a.cos(), c[2] + r * a.sin()]
    };
    let (xi, xo) = (c[0] - outer * w * 0.5, c[0] + outer * w * 0.5);
    for k in 0..n {
        let k2 = (k + 1) % n;
        if outer > 0.0 {
            m.quad(pt(k, xi), pt(k2, xi), pt(k2, xo), pt(k, xo), tyre);
        } else {
            m.quad(pt(k, xo), pt(k2, xo), pt(k2, xi), pt(k, xi), tyre);
        }
        // Outer face with a thin glowing rim ring.
        let hub = [xo, c[1], c[2]];
        if outer > 0.0 {
            m.tri(hub, pt(k, xo), pt(k2, xo), if k % 2 == 0 { rim } else { tyre });
        } else {
            m.tri(hub, pt(k2, xo), pt(k, xo), if k % 2 == 0 { rim } else { tyre });
        }
    }
}

/// Palm tree: curved hexagonal trunk with ring bumps and nine drooping V-folded fronds.
/// Height ≈ 10 m; the trunk leans toward +x (instances rotate it).
pub fn palm() -> Mesh {
    let mut m = Mesh::default();
    let h = 10.0f32;
    let rings = 12;
    let sides = 6;
    let trunk_a = Material { color: [0.055, 0.018, 0.05], emissive: 0.0 };
    let trunk_b = Material { color: [0.085, 0.03, 0.07], emissive: 0.0 };
    let centre = |t: f32| -> V3 { [1.1 * t * t, h * t, 0.0] };
    let ring = |i: usize| -> Vec<V3> {
        let t = i as f32 / rings as f32;
        let c = centre(t);
        let bump = if i.is_multiple_of(2) { 0.03 } else { -0.02 };
        let r = 0.34 - 0.13 * t + bump;
        (0..sides)
            .map(|k| {
                let a = k as f32 / sides as f32 * TAU;
                [c[0] + r * a.cos(), c[1], c[2] + r * a.sin()]
            })
            .collect()
    };
    for i in 0..rings {
        let a = ring(i);
        let b = ring(i + 1);
        let mat = if i % 2 == 0 { trunk_a } else { trunk_b };
        for k in 0..sides {
            let k2 = (k + 1) % sides;
            m.quad(a[k], a[k2], b[k2], b[k], mat);
        }
    }
    // Fronds.
    let top = centre(1.0);
    let frond = Material { color: [0.02, 0.055, 0.055], emissive: 0.0 };
    let frond_dark = Material { color: [0.012, 0.03, 0.035], emissive: 0.0 };
    let leaves = 9;
    let segs = 7;
    for i in 0..leaves {
        let a = i as f32 / leaves as f32 * TAU + 0.31 * (i % 3) as f32;
        let d = [a.cos(), 0.0, a.sin()];
        let side = normalize(cross(d, [0.0, 1.0, 0.0]));
        let len = 4.0 + 0.6 * ((i * 7 % 5) as f32 / 4.0);
        let droop = 2.6 + 0.8 * ((i * 3 % 4) as f32 / 3.0);
        let point = |t: f32| -> (V3, V3, V3) {
            let c = add(add(top, mul(d, len * t)), [0.0, 1.0 * t - droop * t * t, 0.0]);
            let w = 0.8 * (PI * t.max(0.06)).sin() * (1.0 - 0.3 * t);
            let edge_drop = [0.0, -0.18 * w, 0.0];
            (add(add(c, mul(side, -w * 0.5)), edge_drop), c, add(add(c, mul(side, w * 0.5)), edge_drop))
        };
        for s in 0..segs {
            let (l0, c0, r0) = point(s as f32 / segs as f32);
            let (l1, c1, r1) = point((s + 1) as f32 / segs as f32);
            m.quad(l0, c0, c1, l1, frond);
            m.quad(c0, r0, r1, c1, frond_dark);
        }
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(m: &Mesh) {
        assert!(!m.indices.is_empty());
        assert_eq!(m.indices.len() % 3, 0);
        assert!(m.indices.iter().all(|&i| (i as usize) < m.vertices.len()));
        for v in &m.vertices {
            assert!(v.pos.iter().chain(v.normal.iter()).all(|f| f.is_finite()));
            let n = v.normal;
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            assert!((len - 1.0).abs() < 1e-3);
        }
    }

    #[test]
    fn car_is_valid_and_sized_like_a_sports_car() {
        let c = car(&CAR_DEFAULT);
        check(&c);
        // Body bounds, excluding the contact-shadow quad (negative emissive).
        let body = Mesh { vertices: c.vertices.iter().copied().filter(|v| v.emissive >= 0.0).collect(), indices: vec![] };
        let (lo, hi) = body.bounds();
        assert!((hi[2] - lo[2] - CAR_LENGTH).abs() < 0.1);
        assert!((1.8..2.1).contains(&(hi[0] - lo[0])));
        assert!((1.1..1.4).contains(&hi[1]));
        assert!(lo[1] >= -0.01, "car must sit on the ground");
        assert!(c.indices.len() / 3 < 1000);
        assert!(c.vertices.iter().any(|v| v.emissive > 5.0), "needs HDR tail lights");
    }

    #[test]
    fn car_hull_faces_outward() {
        // Flat normals of the side panels should point away from the centre line.
        let c = car(&CAR_DEFAULT);
        let mut checked = 0;
        for v in c.vertices.iter().filter(|v| v.emissive == 0.0 && v.pos[1] > 0.4 && v.pos[1] < 0.8) {
            if v.pos[0].abs() > 0.9 && v.normal[0].abs() > 0.7 {
                assert!(v.normal[0] * v.pos[0] > 0.0, "inward normal at {:?}", v.pos);
                checked += 1;
            }
        }
        assert!(checked > 10);
    }

    #[test]
    fn traffic_vehicles_match_sim_collision_boxes() {
        // Must agree with sim::Archetype::half_extents (half width, half length).
        for (kind, hw, hl) in [
            (VehicleKind::Coupe, 0.92, 2.2),
            (VehicleKind::Van, 1.02, 2.5),
            (VehicleKind::Truck, 1.25, 4.6),
            (VehicleKind::Sports, 0.95, 2.2),
        ] {
            let m = vehicle(kind);
            check(&m);
            let body = Mesh {
                vertices: m.vertices.iter().copied().filter(|v| v.emissive >= 0.0 && v.emissive < 40.0).collect(),
                indices: vec![],
            };
            let (lo, hi) = body.bounds();
            assert!(((hi[0] - lo[0]) * 0.5 - hw).abs() < 0.15, "{kind:?} width {}", hi[0] - lo[0]);
            assert!(((hi[2] - lo[2]) * 0.5 - hl).abs() < 0.2, "{kind:?} length {}", hi[2] - lo[2]);
            assert!(m.vertices.iter().any(|v| v.emissive == marker::BRAKE));
            assert!(m.vertices.iter().any(|v| v.emissive == marker::INDICATOR_LEFT));
        }
    }

    #[test]
    fn palm_is_valid_and_tall() {
        let p = palm();
        check(&p);
        let (lo, hi) = p.bounds();
        assert!(lo[1] >= -0.01 && (8.5..12.0).contains(&hi[1]));
        assert!(hi[0] - lo[0] > 6.0, "fronds should spread");
    }
}
