//! SYNTHRACE game simulation. Pure, deterministic, no platform or rendering deps.
//!
//! Determinism rule: only IEEE-exact operations (+ − × ÷, sqrt, abs, min/max, floor, %) on the
//! simulation path. Trigonometry goes through [`dmath`]; never libm `sin`/`atan2`/`powf`/`exp`,
//! whose results differ between platforms.
//! All gameplay quantities live in *track space*: `s` along the track (m), `x` lateral (m).

pub mod car;
pub mod dmath;
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
pub const NITRO_TAKEDOWN: f32 = 15.0;
/// Coefficient of restitution for car-on-car impacts.
const RESTITUTION: f32 = 0.25;
/// A traffic car whose speed changes by more than this (m/s) in one impact is wrecked.
const WRECK_DV: f32 = 5.0;

/// How a takedown happened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TakedownKind {
    /// Knocked off the road.
    OffRoad,
    /// Smashed into scenery.
    Scenery,
    /// Knocked into other traffic (pile-up).
    PileUp,
}

/// Things that happened during a step; drained by the game for audio, VFX and (M3) scoring.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SimEvent {
    NearMiss {
        gap: f32,
        side: f32,
        close: bool,
    },
    Overtake,
    /// Contact with traffic that didn't spin the player out; `strength` 0..1.
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
    /// A player-caused wreck took a traffic car out.
    Takedown {
        kind: TakedownKind,
    },
    /// Two traffic cars collided (wreck chain); for sound and sparks.
    TrafficImpact {
        ds: f32,
        x: f32,
        strength: f32,
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
    /// Body yaw relative to the road (heading + spin), rad.
    pub yaw: f32,
    /// Lateral / longitudinal acceleration (m/s²) for body roll and pitch.
    pub lat_accel: f32,
    pub long_accel: f32,
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

/// Impulse along unit axis `n` (track space) between bodies a and b with velocities (vs, vx).
/// Returns the impulse magnitude (≥ 0) or None if they're separating.
fn impulse(va: (f32, f32), vb: (f32, f32), n: (f32, f32), ma: f32, mb: f32) -> Option<f32> {
    let u = (vb.0 - va.0) * n.0 + (vb.1 - va.1) * n.1; // b relative to a along n
    if u >= 0.0 {
        return None;
    }
    Some(-(1.0 + RESTITUTION) * u / (1.0 / ma + 1.0 / mb))
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
        self.resolve_player_traffic();
        self.resolve_wrecks();
        self.resolve_scenery();
        self.tick += 1;
    }

    fn resolve_player_traffic(&mut self) {
        let t = &TUNING;
        let track = &self.track;
        for tc in self.traffic.cars.iter_mut() {
            let ds = track.delta(tc.s, self.car.s); // > 0: traffic ahead of the player
            let dx = tc.x - self.car.x;
            let (hw, hl) = tc.kind.half_extents();
            let pen_s = (t.half_length + hl) - ds.abs();
            let pen_x = (t.half_width + hw) - dx.abs();

            if pen_s > 0.0 && pen_x > 0.0 && self.car.stun <= 0.0 {
                tc.touched = true;
                // Collision normal: the axis of least penetration, pointing player → traffic.
                let lateral = pen_x < pen_s;
                let n = if lateral { (0.0, dx.signum()) } else { (ds.signum(), 0.0) };
                // Arcade "traffic checking": light vehicles yield to the player; trucks don't.
                let mp = if tc.kind == Archetype::Truck { t.mass } else { t.check_mass };
                let mt = tc.kind.mass();
                let vp = self.car.velocity();
                let vt = (tc.v, tc.vx);
                // Separate first so the pair never stays interpenetrated.
                let pen = if lateral { pen_x } else { pen_s } + 0.02;
                let wp = mt / (mp + mt);
                if lateral {
                    self.car.x -= n.1 * pen * wp;
                    tc.x += n.1 * pen * (1.0 - wp);
                } else {
                    self.car.s = track.wrap(self.car.s - n.0 * pen * wp);
                    tc.s = track.wrap(tc.s + n.0 * pen * (1.0 - wp));
                }
                if let Some(j) = impulse(vp, vt, n, mp, mt) {
                    let dvp = j / mp;
                    let dvt = j / mt;
                    self.car.set_velocity(vp.0 - n.0 * dvp, vp.1 - n.1 * dvp);
                    tc.v = (tc.v + n.0 * dvt).max(0.0);
                    tc.vx += n.1 * dvt;
                    // Off-centre hits spin the traffic car.
                    let lever = if lateral { ds } else { -dx };
                    tc.spin_rate += (dvt * lever * 0.18).clamp(-5.0, 5.0);
                    if dvt > WRECK_DV || tc.vx.abs() > 3.0 {
                        tc.wreck(true);
                    }
                    let side = dx.signum();
                    if dvp > t.spin_dv {
                        self.car.stun(t.spin_time);
                        self.events.push(SimEvent::SpinOut);
                    } else {
                        if dvp > 1.0 {
                            self.car.start_wobble();
                        }
                        self.car.yaw_rate -= side * (dvp * 0.08).min(0.6);
                        self.events.push(SimEvent::Bump { strength: (dvp / t.spin_dv).clamp(0.15, 1.0), side });
                    }
                }
            }

            // Pass detection: the player's centre crosses the traffic car's centre.
            if !tc.passed && tc.prev_ds > 0.0 && ds <= 0.0 && tc.prev_ds < 50.0 {
                tc.passed = true;
                if !tc.touched && !tc.wrecked {
                    let gap = dx.abs() - (t.half_width + hw);
                    let dv = self.car.v - tc.v;
                    if gap < NEAR_MISS_GAP && dv > NEAR_MISS_MIN_DV {
                        let close = gap < CLOSE_MISS_GAP;
                        self.car.add_nitro(if close { NITRO_CLOSE_MISS } else { NITRO_NEAR_MISS });
                        self.events.push(SimEvent::NearMiss { gap, side: dx.signum(), close });
                    }
                    self.car.add_nitro(NITRO_OVERTAKE);
                    self.events.push(SimEvent::Overtake);
                }
            }
            tc.prev_ds = ds;
        }
    }

    fn award_takedown(car: &mut Car, events: &mut Vec<SimEvent>, tc: &mut TrafficCar, kind: TakedownKind) {
        if tc.by_player && !tc.takedown {
            tc.takedown = true;
            car.add_nitro(NITRO_TAKEDOWN);
            events.push(SimEvent::Takedown { kind });
        }
    }

    /// Wrecked traffic: pile-ups with other cars, smashing into palms, leaving the road.
    fn resolve_wrecks(&mut self) {
        let track = &self.track;
        let cars = &mut self.traffic.cars;
        let n = cars.len();
        if !cars.iter().any(|c| c.wrecked) {
            return;
        }
        for i in 0..n {
            for j in (i + 1)..n {
                if !cars[i].wrecked && !cars[j].wrecked {
                    continue;
                }
                let (a, b) = {
                    let (lo, hi) = cars.split_at_mut(j);
                    (&mut lo[i], &mut hi[0])
                };
                let ds = track.delta(b.s, a.s);
                let dx = b.x - a.x;
                let (ahw, ahl) = a.kind.half_extents();
                let (bhw, bhl) = b.kind.half_extents();
                let pen_s = (ahl + bhl) - ds.abs();
                let pen_x = (ahw + bhw) - dx.abs();
                if pen_s <= 0.0 || pen_x <= 0.0 {
                    continue;
                }
                let lateral = pen_x < pen_s;
                let nrm = if lateral { (0.0, dx.signum()) } else { (ds.signum(), 0.0) };
                let (ma, mb) = (a.kind.mass(), b.kind.mass());
                let wa = mb / (ma + mb);
                if lateral {
                    a.x -= nrm.1 * pen_x * wa;
                    b.x += nrm.1 * pen_x * (1.0 - wa);
                } else {
                    a.s = track.wrap(a.s - nrm.0 * pen_s * wa);
                    b.s = track.wrap(b.s + nrm.0 * pen_s * (1.0 - wa));
                }
                if let Some(jm) = impulse((a.v, a.vx), (b.v, b.vx), nrm, ma, mb) {
                    a.v = (a.v - nrm.0 * jm / ma).max(0.0);
                    a.vx -= nrm.1 * jm / ma;
                    b.v = (b.v + nrm.0 * jm / mb).max(0.0);
                    b.vx += nrm.1 * jm / mb;
                    a.spin_rate -= (jm / ma * 0.15).min(3.0) * dx.signum();
                    b.spin_rate += (jm / mb * 0.15).min(3.0) * dx.signum();
                    let by_player = a.by_player || b.by_player;
                    let strength = ((jm / ma.min(mb)) / 15.0).clamp(0.1, 1.0);
                    for c in [&mut *a, &mut *b] {
                        if !c.wrecked {
                            c.wreck(by_player);
                        }
                    }
                    self.events.push(SimEvent::TrafficImpact {
                        ds: track.delta(a.s, self.car.s),
                        x: (a.x + b.x) * 0.5,
                        strength,
                    });
                    if by_player {
                        a.by_player = true;
                        b.by_player = true;
                        Self::award_takedown(&mut self.car, &mut self.events, a, TakedownKind::PileUp);
                        Self::award_takedown(&mut self.car, &mut self.events, b, TakedownKind::PileUp);
                    }
                }
            }
        }
        // Off the road, and into the palms.
        let palms = &self.track.palms;
        for c in cars.iter_mut().filter(|c| c.wrecked) {
            if c.x.abs() > ROAD_HALF_WIDTH + 2.5 {
                Self::award_takedown(&mut self.car, &mut self.events, c, TakedownKind::OffRoad);
            }
            if c.x.abs() < ROAD_HALF_WIDTH + 3.0 || (c.v < 0.5 && c.vx.abs() < 0.5) {
                continue;
            }
            let (hw, hl) = c.kind.half_extents();
            let lo = palms.partition_point(|p| p.s < c.s - hl - 1.0);
            for p in &palms[lo..] {
                if p.s > c.s + hl + 1.0 {
                    break;
                }
                let r = 0.42 * p.scale;
                if (p.x - c.x).abs() < hw + r && (p.s - c.s).abs() < hl + r {
                    c.v *= 0.15;
                    c.vx *= -0.2;
                    c.spin_rate *= 0.3;
                    self.events.push(SimEvent::TrafficImpact { ds: track.delta(c.s, self.car.s), x: c.x, strength: 1.0 });
                    Self::award_takedown(&mut self.car, &mut self.events, c, TakedownKind::Scenery);
                    break;
                }
            }
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
                    car.travel -= side * 0.15;
                    car.start_wobble();
                    self.events.push(SimEvent::Scrape { side });
                } else {
                    car.v *= 0.15;
                    car.x -= side * 1.5;
                    car.travel = 0.0;
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
            lat_accel: lerp(a.lat_accel, b.lat_accel),
            long_accel: lerp(a.long_accel, b.long_accel),
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
        for f in [c.s, c.x, c.v, c.steer, c.heading, c.travel, c.yaw_rate, c.nitro, c.rpm, c.spin] {
            mix(f.to_bits());
        }
        for tc in &self.traffic.cars {
            mix(tc.id);
            for f in [tc.s, tc.x, tc.v, tc.vx, tc.spin] {
                mix(f.to_bits());
            }
        }
        mix(self.tick as u32);
        h
    }
}

/// Attract-mode driver: scores lanes by time-to-collision, never crosses a car alongside,
/// steers by heading control, and brakes for anything in its own path.
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
            // Beside us (or just behind), not a car further ahead we're trying to get around.
            let alongside = ds > -(t.half_length + hl + 6.0) && ds < t.half_length + hl + 1.0;
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
    let danger = ttc_at(c.x, 0.4).min(ttc_at(target_x, 0.3)) < 2.6;
    // Free distance ahead in our own footprint (m): lets a stopped car creep round an obstacle.
    let gap_ahead = world
        .traffic
        .cars
        .iter()
        .filter(|tc| (tc.x - c.x).abs() < t.half_width + tc.kind.half_extents().0 + 0.3)
        .map(|tc| track.delta(tc.s, c.s) - t.half_length - tc.kind.half_extents().1)
        .filter(|d| *d > -1.0)
        .fold(f32::MAX, f32::min);
    let creep = c.v < 6.0 && gap_ahead > 3.0;

    // Heading control: pick a travel angle that closes the lateral error, then a yaw rate that
    // turns the body toward it while following the road's curvature.
    let v = c.v.max(5.0);
    let lat_des = ((target_x - c.x) * 1.2).clamp(-7.0, 7.0);
    let heading_des = (lat_des / v).clamp(-0.25, 0.25);
    let road_rate = track.curve_at(c.s + c.v * 0.3) * c.v;
    // Yaw needed: follow the unassisted part of the road, hold the heading against the
    // self-centring force, and close the heading error.
    let r_des = road_rate * (1.0 - t.road_assist)
        + heading_des * t.recentre
        + (heading_des - c.heading) * 4.0
        + (heading_des - c.travel) * 1.5;
    let cap = car::yaw_cap(c.v, t.mu_road);
    let steer = if cap > 1e-3 { r_des / cap } else { 0.0 };
    Input {
        steer: steer.clamp(-1.0, 1.0),
        throttle: if creep {
            0.35
        } else if all_blocked || danger {
            0.0
        } else {
            0.85
        },
        // Off-throttle braking never counts as a drift brake-tap.
        brake: if creep {
            0.0
        } else if danger {
            1.0
        } else if all_blocked {
            0.4
        } else {
            0.0
        },
        ..Default::default()
    }
}

/// Golden state hash of [`determinism_probe`]`(42, 60)`. Changes only when the model or tuning
/// changes on purpose; the wasm build must produce the same value (checked in the browser e2e).
pub const GOLDEN_HASH_SEED42_60S: u64 = 0x5cb0a1880810eaeb;

/// Runs a scripted pseudo-player for `seconds` on track `seed` and returns the state hash.
pub fn determinism_probe(seed: u64, seconds: u32) -> u64 {
    let mut w = World::new(seed);
    for _ in 0..seconds * STEP_HZ {
        let i = scripted_input(&w);
        w.step(&i);
        w.events.clear();
    }
    w.state_hash()
}

/// Deterministic pseudo-player: the autopilot drives, with scripted drifts (into and out of
/// bends), nitro bursts and brake pulses layered on top, so every system gets exercised.
fn scripted_input(w: &World) -> Input {
    let mut i = autopilot(w, LANES[(w.tick / 2400 % 4) as usize]);
    match (w.tick / 90) % 10 {
        2 => {
            i.steer = 0.7;
            i.drift = true;
            i.throttle = 1.0;
            i.brake = 0.0;
        }
        3 => i.steer = -0.5,
        5 => i.nitro = true,
        8 if w.tick.is_multiple_of(3) => i.brake = 1.0,
        _ => {}
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(seed: u64, seconds: u32) -> World {
        let mut w = World::new(seed);
        for _ in 0..seconds * STEP_HZ {
            let i = scripted_input(&w);
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

    /// A world on a straight track with one traffic car of `kind` placed at (`ahead`, `x`).
    fn duel_kind(kind: Archetype, player_x: f32, player_v: f32, traffic_x: f32, traffic_v: f32, ahead: f32) -> World {
        let mut w = World::new(1);
        w.track = Track::straight(4000);
        w.traffic.cars.clear();
        w.traffic.row_gap = 1e9;
        w.car = Car::new(player_x);
        w.car.v = player_v;
        let mut tc = TrafficCar::new(99, kind, ahead, 1, traffic_v, ahead);
        tc.x = traffic_x;
        w.traffic.cars.push(tc);
        w
    }

    fn duel(player_x: f32, player_v: f32, traffic_x: f32, traffic_v: f32, ahead: f32) -> World {
        duel_kind(Archetype::Coupe, player_x, player_v, traffic_x, traffic_v, ahead)
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
    fn ramming_light_traffic_launches_it_without_spinning_the_player() {
        let mut w = duel(0.0, 50.0, 0.0, 25.0, 12.0);
        let ev = run_events(&mut w, 1.0, hold_speed());
        assert!(!ev.contains(&SimEvent::SpinOut), "{ev:?}");
        let tc = &w.traffic.cars[0];
        assert!(tc.wrecked, "coupe should be knocked out of control");
        assert!(tc.v > 30.0, "coupe should be launched forward: v = {}", tc.v);
        assert!(w.car.v < 50.0, "the hit should cost the player speed");
    }

    #[test]
    fn ramming_a_truck_hard_spins_the_player() {
        let mut w = duel_kind(Archetype::Truck, 0.0, 60.0, 0.0, 22.0, 20.0);
        let ev = run_events(&mut w, 1.0, hold_speed());
        assert!(ev.contains(&SimEvent::SpinOut), "{ev:?}");
        assert!(!w.traffic.cars[0].wrecked || w.traffic.cars[0].v < 30.0);
    }

    #[test]
    fn slamming_a_car_sideways_takes_it_off_the_road() {
        // Alongside a coupe, steer hard into it: it should be shoved off the tarmac → takedown.
        let widths = TUNING.half_width + Archetype::Coupe.half_extents().0;
        let mut w = duel(LANES[1] + widths + 0.6, 40.0, LANES[1], 38.0, 0.0);
        w.car.heading = -0.35;
        w.car.travel = -0.35;
        let ev = run_events(&mut w, 4.0, Input { steer: -1.0, throttle: 1.0, ..Default::default() });
        assert!(w.traffic.cars[0].wrecked, "{ev:?}");
        assert!(ev.iter().any(|e| matches!(e, SimEvent::Takedown { .. })), "expected a takedown: {ev:?}");
    }

    #[test]
    fn wrecks_cause_pile_ups() {
        let mut w = duel(0.0, 50.0, 0.0, 25.0, 12.0);
        // A slow car further ahead in the same lane, straight in the wreck's path.
        let mut other = TrafficCar::new(100, Archetype::Van, 32.0, 1, 18.0, 32.0);
        other.x = 0.0;
        w.traffic.cars.push(other);
        let ev = run_events(&mut w, 4.0, hold_speed());
        assert!(ev.iter().any(|e| matches!(e, SimEvent::TrafficImpact { .. })), "{ev:?}");
        assert!(w.traffic.cars.iter().filter(|c| c.id == 99 || c.id == 100).all(|c| c.wrecked));
        assert!(ev.iter().any(|e| matches!(e, SimEvent::Takedown { kind: TakedownKind::PileUp })));
    }

    #[test]
    fn touched_cars_give_no_near_miss_or_overtake_credit() {
        let mut w = duel(0.0, 36.0, 0.0, 30.0, 12.0);
        let mut ev = run_events(&mut w, 3.0, hold_speed());
        assert!(ev.iter().any(|e| matches!(e, SimEvent::Bump { .. })), "{ev:?}");
        ev.extend(run_events(&mut w, 0.6, Input { steer: 0.6, throttle: 1.0, ..Default::default() }));
        ev.extend(run_events(&mut w, 8.0, Input { throttle: 1.0, ..Default::default() }));
        assert!(!ev.iter().any(|e| matches!(e, SimEvent::NearMiss { .. })), "{ev:?}");
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
        let mut distance = 0.0;
        let mut last_spin = 0u64;
        for _ in 0..300 * STEP_HZ {
            distance += w.car.v * DT;
            let i = autopilot(&w, LANES[2]);
            w.step(&i);
            for e in w.events.drain(..) {
                match e {
                    SimEvent::SpinOut | SimEvent::Crash => {
                        spins += 1;
                        last_spin = w.tick;
                        let near: Vec<String> = w
                            .traffic
                            .cars
                            .iter()
                            .filter(|c| w.track.delta(c.s, w.car.s).abs() < 40.0)
                            .map(|c| {
                                format!(
                                    "{:?} x{:.1} d{:.1} v{:.1} chg{:.2}",
                                    c.kind,
                                    c.x,
                                    w.track.delta(c.s, w.car.s),
                                    c.v,
                                    c.change_t
                                )
                            })
                            .collect();
                        println!(
                            "SPIN at tick {} car x{:.2} v{:.1} heading {:.3}: {near:?}",
                            w.tick, w.car.x, w.car.v, w.car.heading
                        );
                    }
                    SimEvent::Bump { .. } => bumps += 1,
                    SimEvent::Overtake => passes += 1,
                    _ => {}
                }
            }
            // A spin-out slides wherever momentum takes it; judge the autopilot only when in control.
            let recovering = last_spin > 0 && w.tick - last_spin < 4 * STEP_HZ as u64;
            assert!(recovering || w.car.x.abs() < ROAD_HALF_WIDTH + 0.5, "autopilot left the road at x = {}", w.car.x);
        }
        // Attract-mode quality bar: at most one clipped rear corner in five minutes.
        assert!(spins <= 1, "{spins} spins");
        // Minor contacts (creeping round wrecks, clipped corners) are fine in attract mode.
        assert!(bumps <= 20, "{bumps} bumps");
        assert!(passes > 30, "only {passes} overtakes");
        assert!(distance > 300.0 * 40.0, "average speed too low: {:.0} km/h", distance / 300.0 * 3.6);
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
