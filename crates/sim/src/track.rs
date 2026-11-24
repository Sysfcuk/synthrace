//! Segment track (OutRun-style authoring model): each 5 m segment has a curvature and a height.
//! The track is a ring; it closes in height (curvature needs no closure because the world is
//! rendered relative to the camera).

use crate::rng::Pcg32;

pub const SEGMENT_LEN: f32 = 5.0;
/// Half-width of the tarmac (m). Lane centres at ±2.25 and ±6.75 for four 4.5 m lanes.
pub const ROAD_HALF_WIDTH: f32 = 9.0;
const TARGET_SEGMENTS: usize = 6000; // 30 km per lap
const MAX_CURVE: f32 = 0.0016; // 1/m; hard bend (radius ≈ 625 m)

#[derive(Clone, Copy, Debug, Default)]
pub struct Segment {
    /// Curvature (1/m), positive = bends right.
    pub curve: f32,
    /// Road height at the start of the segment (m).
    pub y: f32,
}

/// A roadside object placement in track space.
#[derive(Clone, Copy, Debug)]
pub struct Prop {
    pub s: f32,
    pub x: f32,
    pub scale: f32,
    pub yaw: f32,
    pub lean: f32,
}

pub struct Track {
    segs: Vec<Segment>,
    pub palms: Vec<Prop>,
}

fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl Track {
    pub fn generate(seed: u64) -> Self {
        let mut rng = Pcg32::new(seed, 0x5eed);
        let mut segs: Vec<Segment> = Vec::with_capacity(TARGET_SEGMENTS + 400);
        let mut y = 0.0f32;

        let section = |segs: &mut Vec<Segment>, enter: u32, hold: u32, leave: u32, curve: f32, dy: f32, y: &mut f32| {
            let total = enter + hold + leave;
            let y0 = *y;
            for n in 0..total {
                let c = if n < enter {
                    curve * smooth(n as f32 / enter.max(1) as f32)
                } else if n < enter + hold {
                    curve
                } else {
                    curve * (1.0 - smooth((n - enter - hold) as f32 / leave.max(1) as f32))
                };
                let yy = y0 + dy * smooth(n as f32 / total as f32);
                segs.push(Segment { curve: c, y: yy });
            }
            *y = y0 + dy;
        };

        section(&mut segs, 0, 60, 0, 0.0, 0.0, &mut y); // start straight
        while segs.len() < TARGET_SEGMENTS {
            // Keep height within a band so hills stay readable.
            let mut dy = rng.range(8.0, 34.0) * rng.sign();
            if y + dy > 70.0 || y + dy < -40.0 {
                dy = -dy;
            }
            match rng.int(0, 5) {
                0 => section(&mut segs, 0, rng.int(20, 60), 0, 0.0, 0.0, &mut y),
                1 | 2 => {
                    let c = rng.range(0.35, 1.0) * MAX_CURVE * rng.sign();
                    let hilly = if rng.f32() < 0.5 { dy } else { 0.0 };
                    section(&mut segs, rng.int(15, 35), rng.int(20, 70), rng.int(15, 35), c, hilly, &mut y);
                }
                3 => {
                    // S-bend.
                    let c = rng.range(0.5, 0.9) * MAX_CURVE * rng.sign();
                    section(&mut segs, rng.int(12, 20), rng.int(15, 30), rng.int(12, 20), c, dy * 0.5, &mut y);
                    section(&mut segs, rng.int(12, 20), rng.int(15, 30), rng.int(12, 20), -c, dy * 0.5, &mut y);
                }
                _ => {
                    // Rolling hill with a gentle sweep.
                    let c = rng.range(0.0, 0.4) * MAX_CURVE * rng.sign();
                    section(&mut segs, 30, rng.int(30, 90), 30, c, dy, &mut y);
                }
            }
        }
        // Close the ring in height.
        section(&mut segs, 40, 60, 40, 0.0, -y, &mut y);

        let mut track = Self { segs, palms: Vec::new() };
        track.palms = track.place_palms(&mut Pcg32::new(seed, 0xa1a1));
        // Sorted by distance so collision checks can binary-search a window.
        track.palms.sort_by(|a, b| a.s.total_cmp(&b.s));
        track
    }

    /// A flat, straight ring of `segments` segments (for tests and tooling).
    pub fn straight(segments: usize) -> Self {
        Self { segs: vec![Segment::default(); segments.max(2)], palms: Vec::new() }
    }

    fn place_palms(&self, rng: &mut Pcg32) -> Vec<Prop> {
        let mut out = Vec::new();
        let mut s = 30.0;
        let len = self.length();
        while s < len - 30.0 {
            // Clusters along both verges with occasional gaps.
            if rng.f32() < 0.85 {
                for side in [-1.0f32, 1.0] {
                    if rng.f32() < 0.8 {
                        out.push(Prop {
                            s: s + rng.range(-4.0, 4.0),
                            x: side * rng.range(ROAD_HALF_WIDTH + 4.5, ROAD_HALF_WIDTH + 9.0),
                            scale: rng.range(1.05, 1.5),
                            yaw: rng.range(0.0, core::f32::consts::TAU),
                            lean: side * rng.range(0.02, 0.12),
                        });
                    }
                }
            }
            s += rng.range(16.0, 30.0);
        }
        out
    }

    pub fn segments(&self) -> &[Segment] {
        &self.segs
    }

    pub fn length(&self) -> f32 {
        self.segs.len() as f32 * SEGMENT_LEN
    }

    /// Signed shortest distance from `b` to `a` along the ring, in (−len/2, len/2].
    pub fn delta(&self, a: f32, b: f32) -> f32 {
        let len = self.length();
        let mut d = (a - b) % len;
        if d > len * 0.5 {
            d -= len;
        } else if d <= -len * 0.5 {
            d += len;
        }
        d
    }

    pub fn wrap(&self, s: f32) -> f32 {
        let len = self.length();
        let r = s % len;
        if r < 0.0 { r + len } else { r }
    }

    fn index(&self, s: f32) -> (usize, f32) {
        let s = self.wrap(s);
        let f = s / SEGMENT_LEN;
        let i = (f as usize).min(self.segs.len() - 1);
        (i, f - i as f32)
    }

    pub fn curve_at(&self, s: f32) -> f32 {
        self.segs[self.index(s).0].curve
    }

    pub fn height_at(&self, s: f32) -> f32 {
        let (i, t) = self.index(s);
        let a = self.segs[i].y;
        let b = self.segs[(i + 1) % self.segs.len()].y;
        a + (b - a) * t
    }

    /// Accumulated lateral offset C(z) and relative height H(z) ahead of `s`, sampled every
    /// `step` metres (integrated per segment, like classic pseudo-3D `dx += curve`).
    pub fn profile(&self, s: f32, step: f32, out: &mut [[f32; 2]]) {
        let h0 = self.height_at(s);
        let (mut x, mut dx) = (0.0f32, 0.0f32);
        let mut z = 0.0f32;
        for (k, o) in out.iter_mut().enumerate() {
            let target = k as f32 * step;
            while z + SEGMENT_LEN <= target + 1e-3 {
                x += dx * SEGMENT_LEN;
                dx += self.curve_at(s + z) * SEGMENT_LEN;
                z += SEGMENT_LEN;
            }
            let rem = target - z;
            *o = [x + dx * rem, self.height_at(s + target) - h0];
        }
    }

    /// Lateral offset and heading slope of the road at distance `z` ahead of `s` (for placing
    /// objects so they sit on the bent road exactly where the renderer draws it).
    pub fn offset_at(&self, s: f32, z: f32) -> f32 {
        let mut out = [[0.0f32; 2]; 2];
        self.profile(s, z, &mut out);
        out[1][0]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_closes_in_height_and_is_long_enough() {
        let t = Track::generate(42);
        assert!(t.length() >= TARGET_SEGMENTS as f32 * SEGMENT_LEN);
        assert!((t.height_at(0.0) - t.height_at(t.length() - 0.01)).abs() < 0.5);
        for w in t.segments().windows(2) {
            assert!((w[1].y - w[0].y).abs() < 2.5, "height step too large");
            assert!(w[0].curve.abs() <= MAX_CURVE + 1e-6);
        }
    }

    #[test]
    fn profile_starts_at_zero_and_matches_offset_at() {
        let t = Track::generate(3);
        let mut p = [[0.0f32; 2]; 64];
        t.profile(1234.0, 20.0, &mut p);
        assert_eq!(p[0], [0.0, 0.0]);
        assert!((t.offset_at(1234.0, 100.0) - p[5][0]).abs() < 1e-3);
    }

    #[test]
    fn palms_are_off_the_tarmac() {
        let t = Track::generate(9);
        assert!(t.palms.len() > 500);
        assert!(t.palms.iter().all(|p| p.x.abs() > ROAD_HALF_WIDTH + 4.0));
    }
}
