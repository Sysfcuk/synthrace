//! SYNTHRACE game simulation. Pure, deterministic, no platform or rendering deps.
//!
//! Determinism rule: only IEEE-exact operations (+ − × ÷, sqrt, abs, min/max) on the
//! simulation path; no `powf`/`sin`/`exp`, whose results differ between libm implementations.
//! All gameplay quantities live in *track space*: `s` along the track (m), `x` lateral (m).

pub mod car;
pub mod rng;
pub mod track;

pub use car::{Car, Input, TUNING};
pub use rng::Pcg32;
pub use track::{ROAD_HALF_WIDTH, SEGMENT_LEN, Track};

/// Fixed simulation rate.
pub const STEP_HZ: u32 = 120;
pub const DT: f32 = 1.0 / STEP_HZ as f32;

/// Visual snapshot of the car used for render interpolation.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CarView {
    pub s: f32,
    pub x: f32,
    pub v: f32,
    pub steer: f32,
    pub yaw: f32,
    pub offroad: bool,
}

pub struct World {
    pub track: Track,
    pub car: Car,
    pub prev: CarView,
    pub tick: u64,
}

impl World {
    pub fn new(seed: u64) -> Self {
        let track = Track::generate(seed);
        let car = Car::new(ROAD_HALF_WIDTH * 0.25);
        let prev = car.view();
        Self { track, car, prev, tick: 0 }
    }

    /// Advances the simulation by exactly one fixed step.
    pub fn step(&mut self, input: &Input) {
        self.prev = self.car.view();
        self.car.step(&self.track, input, DT);
        self.tick += 1;
    }

    /// Car state interpolated between the previous and current step (`alpha` in 0..1).
    pub fn interpolated(&self, alpha: f32) -> CarView {
        let a = self.prev;
        let b = self.car.view();
        let len = self.track.length();
        // Unwrap s across the track seam before lerping.
        let mut ds = b.s - a.s;
        if ds < -len * 0.5 {
            ds += len;
        }
        let lerp = |p: f32, q: f32| p + (q - p) * alpha;
        CarView {
            s: self.track.wrap(a.s + ds * alpha),
            x: lerp(a.x, b.x),
            v: lerp(a.v, b.v),
            steer: lerp(a.steer, b.steer),
            yaw: lerp(a.yaw, b.yaw),
            offroad: b.offroad,
        }
    }

    /// FNV-1a hash over the exact bit patterns of the simulation state.
    pub fn state_hash(&self) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let mut mix = |bits: u32| {
            for b in bits.to_le_bytes() {
                h ^= b as u64;
                h = h.wrapping_mul(0x0100_0000_01b3);
            }
        };
        let v = self.car.view();
        for f in [v.s, v.x, v.v, v.steer, v.yaw] {
            mix(f.to_bits());
        }
        mix(self.tick as u32);
        h
    }
}

/// Simple attract-mode driver: cancels centrifugal push and holds a lane.
pub fn autopilot(world: &World, lane_x: f32) -> Input {
    let c = &world.car;
    let t = &TUNING;
    let curve = world.track.curve_at(c.s + c.v * 0.4);
    let grip = car::grip(c.v);
    let push = curve * c.v * c.v * t.centrifugal;
    let want_rate = push + (lane_x - c.x) * 1.8 - c.lateral_vel * 0.3;
    let steer = if grip > 1e-3 { want_rate / (t.steer_rate * grip) } else { 0.0 };
    Input { steer: steer.clamp(-1.0, 1.0), throttle: 0.85, brake: 0.0 }
}

/// Golden state hash of [`determinism_probe`]`(42, 60)`. Changes only when the model or tuning
/// changes on purpose; the wasm build must produce the same value (checked in the browser e2e).
pub const GOLDEN_HASH_SEED42_60S: u64 = 0x06fe79dbc1b299b5;

/// Runs a scripted pseudo-player for `seconds` on track `seed` and returns the state hash.
pub fn determinism_probe(seed: u64, seconds: u32) -> u64 {
    let mut w = World::new(seed);
    for _ in 0..seconds * STEP_HZ {
        let i = scripted_input(w.tick);
        w.step(&i);
    }
    w.state_hash()
}

fn scripted_input(tick: u64) -> Input {
    // Deterministic pseudo-player: throttle mostly, weaving, occasional braking.
    let phase = (tick / 90) % 8;
    Input {
        steer: match phase {
            1 | 2 => -1.0,
            5 => 1.0,
            6 => 0.5,
            _ => 0.0,
        },
        throttle: if phase == 7 { 0.0 } else { 1.0 },
        brake: if phase == 7 && tick.is_multiple_of(3) { 1.0 } else { 0.0 },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(seed: u64, seconds: u32) -> World {
        let mut w = World::new(seed);
        for _ in 0..seconds * STEP_HZ {
            let i = scripted_input(w.tick);
            w.step(&i);
        }
        w
    }

    #[test]
    fn same_seed_and_inputs_give_identical_state() {
        let a = run(42, 60);
        let b = run(42, 60);
        assert_eq!(a.state_hash(), b.state_hash());
        assert_ne!(run(43, 60).state_hash(), a.state_hash(), "different track should diverge");
    }

    /// Golden value: changes only when tuning or the model changes on purpose.
    /// The same value must be produced by the wasm build (checked by the e2e suite).
    #[test]
    fn golden_hash_is_stable() {
        let h = determinism_probe(42, 60);
        assert_eq!(h, run(42, 60).state_hash());
        println!("golden hash = {h:#018x}");
        assert_eq!(h, GOLDEN_HASH_SEED42_60S, "simulation changed: update GOLDEN_HASH_SEED42_60S if intended");
    }

    #[test]
    fn autopilot_stays_on_the_road_for_five_minutes() {
        let mut w = World::new(7);
        let mut max_dev: f32 = 0.0;
        for _ in 0..300 * STEP_HZ {
            let i = autopilot(&w, 4.5);
            w.step(&i);
            max_dev = max_dev.max(w.car.x.abs());
        }
        assert!(max_dev < ROAD_HALF_WIDTH, "autopilot left the road: max |x| = {max_dev}");
        assert!(w.car.v > TUNING.v_max * 0.5);
    }

    #[test]
    fn interpolation_is_continuous_across_the_track_seam() {
        let mut w = World::new(1);
        w.car.s = w.track.length() - 0.2;
        w.car.v = 60.0;
        w.step(&Input::default());
        let mid = w.interpolated(0.5);
        assert!(mid.s < 1.0 || mid.s > w.track.length() - 1.0, "s = {}", mid.s);
    }
}
