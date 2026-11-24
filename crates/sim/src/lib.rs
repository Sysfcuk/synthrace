//! SYNTHRACE game simulation. Pure, deterministic, no platform or rendering deps.
//!
//! Determinism rule: only IEEE-exact operations (+ − × ÷, sqrt, abs, min/max, floor) on the
//! simulation path; no `powf`/`sin`/`exp`, whose results differ between libm implementations.
//! All gameplay quantities live in *track space*: `s` along the track (m), `x` lateral (m).

pub mod car;
pub mod rng;
pub mod track;
pub mod traffic;

pub use car::{Car, Input, TUNING};
pub use rng::Pcg32;
pub use track::{ROAD_HALF_WIDTH, SEGMENT_LEN, Track};
pub use traffic::{Archetype, LANES, Traffic, TrafficCar};

/// Fixed simulation rate.
pub const STEP_HZ: u32 = 120;
pub const DT: f32 = 1.0 / STEP_HZ as f32;

/// Near-miss thresholds (GAME_DESIGN §6.1): lateral gap as a fraction of a car width.
pub const NEAR_MISS_GAP: f32 = 0.35 * 1.9;
pub const CLOSE_MISS_GAP: f32 = 0.15 * 1.9;
pub const NEAR_MISS_MIN_DV: f32 = 30.0 / 3.6;
pub const NITRO_NEAR_MISS: f32 = 6.0;
pub const NITRO_CLOSE_MISS: f32 = 10.0;
pub const NITRO_OVERTAKE: f32 = 2.0;

/// Things that happened during a step; drained by the game for audio, VFX and (M3) scoring.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SimEvent {
    NearMiss {
        gap: f32,
        side: f32,
        close: bool,
    },
    Overtake,
    /// Low-speed contact with traffic (rear-end or sideswipe); `strength` 0..1.
    Bump {
        strength: f32,
        side: f32,
    },
    SpinOut,
    /// Square hit on scenery.
    Crash,
    /// Glancing contact with scenery.
    Scrape {
        side: f32,
    },
    DriftStart,
    DriftTier {
        tier: u8,
    },
    DriftBoost {
        tier: u8,
    },
    DriftEnd,
    NitroStart,
    NitroEnd,
    GearShift {
        gear: u8,
    },
}

/// Visual snapshot of the car used for render interpolation.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CarView {
    pub s: f32,
    pub x: f32,
    pub v: f32,
    pub steer: f32,
    pub yaw: f32,
    pub offroad: bool,
    pub drifting: bool,
    pub drift_tier: u8,
    pub nitro_active: bool,
    pub boosting: bool,
    pub stunned: bool,
}

pub struct World {
    pub track: Track,
    pub car: Car,
    pub traffic: Traffic,
    pub prev: CarView,
    pub tick: u64,
    /// Events produced since the game last drained them.
    pub events: Vec<SimEvent>,
}

impl World {
    pub fn new(seed: u64) -> Self {
        let track = Track::generate(seed);
        let car = Car::new(LANES[2]);
        let prev = car.view();
        let mut traffic = Traffic::new(seed);
        traffic.populate(&track, car.s, 0.0);
        Self { track, car, traffic, prev, tick: 0, events: Vec::with_capacity(32) }
    }

    /// Advances the simulation by exactly one fixed step.
    pub fn step(&mut self, input: &Input) {
        self.prev = self.car.view();
        self.car.step(&self.track, input, DT, &mut self.events);
        self.traffic.update(&self.track, self.car.s, self.car.v, DT);
        self.resolve_traffic();
        self.resolve_scenery();
        self.tick += 1;
    }

    fn resolve_traffic(&mut self) {
        let t = &TUNING;
        let car = &mut self.car;
        let track = &self.track;
        for tc in self.traffic.cars.iter_mut() {
            let ds = track.delta(tc.s, car.s); // > 0: traffic ahead of the player
            let dx = tc.x - car.x;
            let (hw, hl) = tc.kind.half_extents();
            let pen_s = (t.half_length + hl) - ds.abs();
            let pen_x = (t.half_width + hw) - dx.abs();

            if pen_s > 0.0 && pen_x > 0.0 && car.stun <= 0.0 {
                tc.touched = true;
                let side = if dx > 0.0 { 1.0 } else { -1.0 };
                if pen_x < pen_s {
                    // Sideswipe: push apart laterally.
                    car.x -= side * (pen_x + 0.05);
                    car.v *= 0.9;
                    car.start_wobble();
                    self.events.push(SimEvent::Bump { strength: 0.4, side });
                } else if ds > 0.0 {
                    // Rear-ending a car ahead.
                    let dv = car.v - tc.v;
                    if dv < t.bump_max_dv {
                        car.v = (car.v * 0.7).min(tc.v - 1.0).max(0.0);
                        car.s = track.wrap(tc.s - (t.half_length + hl) - 0.05);
                        car.x -= side * 0.2;
                        car.start_wobble();
                        self.events.push(SimEvent::Bump { strength: (dv / t.bump_max_dv).clamp(0.2, 1.0), side });
                    } else {
                        car.v *= 0.35;
                        car.s = track.wrap(tc.s - (t.half_length + hl) - 0.05);
                        tc.v += 4.0;
                        car.stun(t.spin_time);
                        self.events.push(SimEvent::SpinOut);
                    }
                } else {
                    // Being rear-ended by faster traffic while slow.
                    tc.v = (car.v - 1.0).max(0.0);
                    tc.s = track.wrap(car.s - (t.half_length + hl) - 0.05);
                    car.v += 2.0;
                    car.start_wobble();
                    self.events.push(SimEvent::Bump { strength: 0.3, side });
                }
            }

            // Pass detection: the player's centre crosses the traffic car's centre.
            if !tc.passed && tc.prev_ds > 0.0 && ds <= 0.0 && tc.prev_ds < 50.0 {
                tc.passed = true;
                if !tc.touched {
                    let gap = dx.abs() - (t.half_width + hw);
                    let dv = car.v - tc.v;
                    if gap < NEAR_MISS_GAP && dv > NEAR_MISS_MIN_DV {
                        let close = gap < CLOSE_MISS_GAP;
                        car.add_nitro(if close { NITRO_CLOSE_MISS } else { NITRO_NEAR_MISS });
                        self.events.push(SimEvent::NearMiss { gap, side: dx.signum(), close });
                    }
                    car.add_nitro(NITRO_OVERTAKE);
                    self.events.push(SimEvent::Overtake);
                }
            }
            tc.prev_ds = ds;
        }
    }

    fn resolve_scenery(&mut self) {
        let t = &TUNING;
        let car = &mut self.car;
        if car.x.abs() < ROAD_HALF_WIDTH + 3.0 || car.stun > 0.0 {
            return;
        }
        let palms = &self.track.palms;
        let lo = palms.partition_point(|p| p.s < car.s - 4.0);
        for p in &palms[lo..] {
            if p.s > car.s + 4.0 {
                break;
            }
            let r = 0.42 * p.scale;
            let ds = p.s - car.s;
            let dx = p.x - car.x;
            let pen_s = (t.half_length + r) - ds.abs();
            let pen_x = (t.half_width + r) - dx.abs();
            if pen_s > 0.0 && pen_x > 0.0 {
                let side = dx.signum();
                if pen_x < pen_s * 0.6 {
                    car.x -= side * (pen_x + 0.05);
                    car.v *= 0.85;
                    car.start_wobble();
                    self.events.push(SimEvent::Scrape { side });
                } else {
                    car.v *= 0.15;
                    car.x -= side * 1.5;
                    car.stun(t.crash_time);
                    self.events.push(SimEvent::Crash);
                }
                return;
            }
        }
    }

    /// Car state interpolated between the previous and current step (`alpha` in 0..1).
    pub fn interpolated(&self, alpha: f32) -> CarView {
        let a = self.prev;
        let b = self.car.view();
        let ds = self.track.delta(b.s, a.s);
        let lerp = |p: f32, q: f32| p + (q - p) * alpha;
        CarView {
            s: self.track.wrap(a.s + ds * alpha),
            x: lerp(a.x, b.x),
            v: lerp(a.v, b.v),
            steer: lerp(a.steer, b.steer),
            yaw: lerp(a.yaw, b.yaw),
            ..b
        }
    }

    /// FNV-1a hash over the exact bit patterns of the simulation state (car + traffic).
    pub fn state_hash(&self) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let mut mix = |bits: u32| {
            for b in bits.to_le_bytes() {
                h ^= b as u64;
                h = h.wrapping_mul(0x0100_0000_01b3);
            }
        };
        let c = &self.car;
        for f in [c.s, c.x, c.v, c.steer, c.yaw, c.nitro, c.rpm, c.spin] {
            mix(f.to_bits());
        }
        for tc in &self.traffic.cars {
            mix(tc.id);
            for f in [tc.s, tc.x, tc.v] {
                mix(f.to_bits());
            }
        }
        mix(self.tick as u32);
        h
    }
}

/// Attract-mode driver: scores lanes by time-to-collision, never crosses a car alongside,
/// cancels centrifugal push, and brakes for anything in its own footprint.
pub fn autopilot(world: &World, preferred_x: f32) -> Input {
    let c = &world.car;
    let t = &TUNING;
    let track = &world.track;
    const HORIZON: f32 = 8.0; // seconds of look-ahead that count as "free"

    // Time until we'd reach the first car ahead in a lateral band around `x`.
    let ttc_at = |x: f32, band: f32| -> f32 {
        let mut best = HORIZON;
        for tc in &world.traffic.cars {
            let (hw, hl) = tc.kind.half_extents();
            let occupies_x = (tc.x - x).abs() < t.half_width + hw + band
                || (tc.change_t >= 0.0 && (LANES[tc.target as usize] - x).abs() < t.half_width + hw + band);
            if !occupies_x {
                continue;
            }
            let ds = track.delta(tc.s, c.s) - (t.half_length + hl);
            if ds < -(t.half_length + hl) * 2.0 - 2.0 {
                continue; // well behind us
            }
            let closing = (c.v - tc.v).max(0.5);
            let ttc = if ds <= 0.0 { 0.0 } else { ds / closing };
            best = best.min(ttc);
        }
        best
    };
    // Is a car alongside anywhere between our x and `x` (so moving there would sideswipe)?
    let path_clear = |x: f32| -> bool {
        let (lo, hi) = if x < c.x { (x, c.x) } else { (c.x, x) };
        world.traffic.cars.iter().all(|tc| {
            let (hw, hl) = tc.kind.half_extents();
            let ds = track.delta(tc.s, c.s);
            let alongside = ds.abs() < t.half_length + hl + 6.0;
            let in_path = tc.x + hw > lo - t.half_width && tc.x - hw < hi + t.half_width;
            !(alongside && in_path) || (tc.x - c.x).abs() < 0.5
        })
    };
    let mut target_x = c.x;
    let mut best_score = f32::MIN;
    for &lane in &LANES {
        if (lane - c.x).abs() > 0.5 && !path_clear(lane) {
            continue;
        }
        let stay_bonus = if (lane - c.x).abs() < 1.2 { 1.0 } else { 0.0 };
        let score = ttc_at(lane, 0.2) + stay_bonus - (lane - c.x).abs() * 0.08 - (lane - preferred_x).abs() * 0.03;
        if score > best_score {
            best_score = score;
            target_x = lane;
        }
    }
    let all_blocked = best_score < 1.5;
    if all_blocked {
        // Nothing good anywhere: hold the nearest lane rather than weave.
        target_x = LANES.iter().copied().min_by(|a, b| (a - c.x).abs().total_cmp(&(b - c.x).abs())).unwrap_or(c.x);
    }
    let danger = ttc_at(c.x, 0.3).min(ttc_at(target_x, 0.2)) < 1.6;
    let curve = track.curve_at(c.s + c.v * 0.4);
    let grip = car::grip(c.v);
    let push = curve * c.v * c.v * t.centrifugal;
    let want_rate = push + (target_x - c.x) * 1.8 - c.lateral_vel * 0.3;
    let steer = if grip > 1e-3 { want_rate / (t.steer_rate * grip) } else { 0.0 };
    Input {
        steer: steer.clamp(-1.0, 1.0),
        throttle: if all_blocked || danger { 0.0 } else { 0.85 },
        brake: if danger {
            1.0
        } else if all_blocked {
            0.6
        } else {
            0.0
        },
        ..Default::default()
    }
}

/// Golden state hash of [`determinism_probe`]`(42, 60)`. Changes only when the model or tuning
/// changes on purpose; the wasm build must produce the same value (checked in the browser e2e).
pub const GOLDEN_HASH_SEED42_60S: u64 = 0xee1c339893585c25;

/// Runs a scripted pseudo-player for `seconds` on track `seed` and returns the state hash.
pub fn determinism_probe(seed: u64, seconds: u32) -> u64 {
    let mut w = World::new(seed);
    for _ in 0..seconds * STEP_HZ {
        let i = scripted_input(w.tick);
        w.step(&i);
        w.events.clear();
    }
    w.state_hash()
}

fn scripted_input(tick: u64) -> Input {
    // Deterministic pseudo-player: throttle mostly, weaving, drifting, nitro bursts, braking.
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
        drift: phase == 2,
        nitro: phase == 4,
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
            w.events.clear();
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

    #[test]
    fn golden_hash_is_stable() {
        let h = determinism_probe(42, 60);
        assert_eq!(h, run(42, 60).state_hash());
        println!("golden hash = {h:#018x}");
        assert_eq!(h, GOLDEN_HASH_SEED42_60S, "simulation changed: update GOLDEN_HASH_SEED42_60S if intended");
    }

    /// A world on a straight track with one traffic car placed at (`ahead`, `x`).
    fn duel(player_x: f32, player_v: f32, traffic_x: f32, traffic_v: f32, ahead: f32) -> World {
        let mut w = World::new(1);
        w.track = Track::straight(4000);
        w.traffic.cars.clear();
        w.traffic.row_gap = 1e9;
        w.car = Car::new(player_x);
        w.car.v = player_v;
        w.traffic.cars.push(TrafficCar {
            id: 99,
            kind: Archetype::Coupe,
            s: ahead,
            x: traffic_x,
            v: traffic_v,
            cruise: traffic_v,
            lane: 1,
            target: 1,
            change_t: -1.0,
            brake: 0.0,
            prev_ds: ahead,
            passed: false,
            touched: false,
        });
        w
    }

    fn run_events(w: &mut World, seconds: f32, input: Input) -> Vec<SimEvent> {
        let mut out = Vec::new();
        for _ in 0..(seconds / DT) as usize {
            w.step(&input);
            out.append(&mut w.events);
        }
        out
    }

    fn hold_speed() -> Input {
        Input { throttle: 0.0, ..Default::default() }
    }

    #[test]
    fn near_miss_thresholds() {
        let widths = TUNING.half_width + Archetype::Coupe.half_extents().0;
        // (gap, expected: None / Some(close))
        for (gap, expect) in [(0.2, Some(true)), (0.5, Some(false)), (0.9, None)] {
            let mut w = duel(0.0, 60.0, widths + gap, 30.0, 40.0);
            let ev = run_events(&mut w, 2.0, hold_speed());
            let nm: Vec<_> =
                ev.iter().filter_map(|e| if let SimEvent::NearMiss { close, .. } = e { Some(*close) } else { None }).collect();
            match expect {
                Some(c) => assert_eq!(nm, vec![c], "gap {gap}: {ev:?}"),
                None => assert!(nm.is_empty(), "gap {gap}: {ev:?}"),
            }
            assert!(ev.contains(&SimEvent::Overtake));
        }
    }

    #[test]
    fn near_miss_needs_speed_difference() {
        let widths = TUNING.half_width + Archetype::Coupe.half_extents().0;
        let mut w = duel(0.0, 36.0, widths + 0.2, 30.0, 10.0);
        let ev = run_events(&mut w, 4.0, hold_speed());
        assert!(!ev.iter().any(|e| matches!(e, SimEvent::NearMiss { .. })), "{ev:?}");
    }

    #[test]
    fn slow_rear_end_bumps_fast_rear_end_spins() {
        let mut w = duel(0.0, 36.0, 0.0, 30.0, 12.0);
        let ev = run_events(&mut w, 2.0, hold_speed());
        assert!(ev.iter().any(|e| matches!(e, SimEvent::Bump { .. })), "{ev:?}");
        assert!(!ev.contains(&SimEvent::SpinOut));

        let mut w = duel(0.0, 70.0, 0.0, 30.0, 30.0);
        let ev = run_events(&mut w, 1.0, hold_speed());
        assert!(ev.contains(&SimEvent::SpinOut), "{ev:?}");
        assert!(w.car.stun > 0.0 && w.car.v < 30.0);
    }

    #[test]
    fn touched_cars_give_no_near_miss_or_overtake_credit() {
        let mut w = duel(0.0, 36.0, 0.0, 30.0, 12.0);
        let mut ev = run_events(&mut w, 3.0, hold_speed());
        assert!(ev.iter().any(|e| matches!(e, SimEvent::Bump { .. })), "{ev:?}");
        ev.extend(run_events(&mut w, 0.6, Input { steer: 0.6, throttle: 1.0, ..Default::default() }));
        ev.extend(run_events(&mut w, 8.0, Input { throttle: 1.0, ..Default::default() }));
        assert!(w.traffic.cars[0].passed, "player should have got past");
        assert!(!ev.iter().any(|e| matches!(e, SimEvent::NearMiss { .. } | SimEvent::Overtake)), "{ev:?}");
    }

    #[test]
    fn hitting_a_palm_square_on_crashes() {
        let mut w = World::new(9);
        let p = w.track.palms[10];
        w.traffic.cars.clear();
        w.traffic.row_gap = 1e9;
        w.car.s = p.s - 8.0;
        w.car.x = p.x;
        w.car.v = 40.0;
        let ev = run_events(&mut w, 0.5, hold_speed());
        assert!(ev.contains(&SimEvent::Crash), "{ev:?}");
    }

    #[test]
    fn autopilot_drives_five_minutes_in_traffic_without_spinning() {
        let mut w = World::new(7);
        let mut spins = 0;
        let mut bumps = 0;
        let mut passes = 0;
        for _ in 0..300 * STEP_HZ {
            let i = autopilot(&w, LANES[2]);
            w.step(&i);
            for e in w.events.drain(..) {
                match e {
                    SimEvent::SpinOut | SimEvent::Crash => {
                        spins += 1;
                        let near: Vec<String> = w
                            .traffic
                            .cars
                            .iter()
                            .filter(|c| w.track.delta(c.s, w.car.s).abs() < 60.0)
                            .map(|c| {
                                format!(
                                    "x{:.1} t{:.2} tgt{} d{:.1} v{:.1}",
                                    c.x,
                                    c.change_t,
                                    c.target,
                                    w.track.delta(c.s, w.car.s),
                                    c.v
                                )
                            })
                            .collect();
                        println!("SPIN at tick {} car x{:.2} v{:.1}: {:?}", w.tick, w.car.x, w.car.v, near);
                    }
                    SimEvent::Bump { .. } => bumps += 1,
                    SimEvent::Overtake => passes += 1,
                    _ => {}
                }
            }
            assert!(w.car.x.abs() < ROAD_HALF_WIDTH, "autopilot left the road");
        }
        assert_eq!(spins, 0);
        assert!(bumps <= 3, "{bumps} bumps");
        assert!(passes > 50, "only {passes} overtakes");
    }

    #[test]
    fn interpolation_is_continuous_across_the_track_seam() {
        let mut w = World::new(1);
        w.traffic.cars.clear();
        w.car.s = w.track.length() - 0.2;
        w.car.v = 60.0;
        w.step(&Input::default());
        let mid = w.interpolated(0.5);
        assert!(mid.s < 1.0 || mid.s > w.track.length() - 1.0, "s = {}", mid.s);
    }
}
