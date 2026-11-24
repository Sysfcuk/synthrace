//! Arcade handling model (GAME_DESIGN §4). Units: metres, seconds, m/s.

use crate::track::{ROAD_HALF_WIDTH, Track};
use crate::{CarView, SimEvent};

const G: f32 = 9.81;

/// All handling constants in one place; mirrored in GAME_DESIGN.md §4.
pub struct Tuning {
    /// Top speed (m/s) — 300 km/h.
    pub v_max: f32,
    /// Peak acceleration at standstill (m/s²).
    pub accel: f32,
    pub brake: f32,
    pub coast_drag: f32,
    /// Lateral speed (m/s) at full steer and full grip.
    pub steer_rate: f32,
    /// Steer input ramp-in / recentre times (s) for digital input.
    pub steer_in_time: f32,
    pub steer_out_time: f32,
    /// Lateral push = curve · v² · centrifugal (m/s).
    pub centrifugal: f32,
    /// Off-road speed cap as a fraction of v_max, and deceleration toward it.
    pub offroad_speed: f32,
    pub offroad_decel: f32,
    pub offroad_steer: f32,
    /// Hard verge limit (m); hitting it scrapes speed.
    pub x_limit: f32,
    pub scrape_decay: f32,
    // --- drift ---
    pub drift_min_speed: f32,
    pub drift_centrifugal: f32,
    pub drift_drag: f32,
    /// Charge seconds for tiers 1..3, and (boost gain, boost seconds, nitro bonus) per tier.
    pub drift_tiers: [f32; 3],
    pub drift_boosts: [(f32, f32, f32); 3],
    pub drift_nitro_per_s: f32,
    // --- nitro ---
    pub nitro_drain: f32,
    pub nitro_top_gain: f32,
    pub nitro_accel_gain: f32,
    pub nitro_start: f32,
    // --- collisions ---
    pub half_width: f32,
    pub half_length: f32,
    pub bump_max_dv: f32,
    pub spin_time: f32,
    pub crash_time: f32,
    pub wobble_time: f32,
    // --- engine ---
    pub idle_rpm: f32,
    pub redline_rpm: f32,
    /// Upper speed of each gear as a fraction of v_max.
    pub gear_tops: [f32; 6],
    pub shift_cut: f32,
}

pub const TUNING: Tuning = Tuning {
    v_max: 300.0 / 3.6,
    accel: 1.2 * G,
    brake: 1.5 * G,
    coast_drag: 0.15 * G,
    steer_rate: 16.0,
    steer_in_time: 0.12,
    steer_out_time: 0.08,
    centrifugal: 0.53,
    offroad_speed: 0.38,
    offroad_decel: 2.0 * G,
    offroad_steer: 0.5,
    x_limit: ROAD_HALF_WIDTH + 11.0,
    scrape_decay: 1.5,
    drift_min_speed: 120.0 / 3.6,
    drift_centrifugal: 0.45,
    drift_drag: 0.1 * G,
    drift_tiers: [0.8, 1.8, 3.0],
    drift_boosts: [(0.08, 0.6, 5.0), (0.12, 1.2, 8.0), (0.15, 1.8, 12.0)],
    drift_nitro_per_s: 3.0,
    nitro_drain: 25.0,
    nitro_top_gain: 0.2,
    nitro_accel_gain: 0.25,
    nitro_start: 34.0,
    half_width: 0.95,
    half_length: 2.2,
    bump_max_dv: 40.0 / 3.6,
    spin_time: 1.7,
    crash_time: 2.3,
    wobble_time: 0.5,
    idle_rpm: 1100.0,
    redline_rpm: 7600.0,
    gear_tops: [0.20, 0.35, 0.50, 0.66, 0.83, 1.25],
    shift_cut: 0.12,
};

/// Player input for one step. `steer` −1 (left) … 1 (right); throttle/brake 0 … 1.
#[derive(Clone, Copy, Debug, Default)]
pub struct Input {
    pub steer: f32,
    pub throttle: f32,
    pub brake: f32,
    pub drift: bool,
    pub nitro: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Drift {
    pub active: bool,
    pub dir: f32,
    /// Accumulated charge (s); doubles while the angle is tight.
    pub charge: f32,
    pub tier: u8,
    centre_time: f32,
    by_button: bool,
}

#[derive(Clone, Debug)]
pub struct Car {
    pub s: f32,
    pub x: f32,
    pub v: f32,
    /// Shaped steering (−1 … 1).
    pub steer: f32,
    /// Visual yaw (rad).
    pub yaw: f32,
    pub lateral_vel: f32,
    pub offroad: bool,
    pub drift: Drift,
    /// Nitro meter 0..100.
    pub nitro: f32,
    pub nitro_active: bool,
    pub boost_time: f32,
    pub boost_gain: f32,
    /// Seconds of lost control remaining (spin-out / crash).
    pub stun: f32,
    pub spin: f32,
    pub wobble: f32,
    pub gear: u8,
    pub rpm: f32,
    shift_timer: f32,
    brake_tap: f32,
    /// Tyre slip 0..1 (drives squeal audio and smoke).
    pub slip: f32,
}

/// Steering authority: none at standstill, reduced at top speed for stability.
pub fn grip(v: f32) -> f32 {
    let u = (v / TUNING.v_max).clamp(0.0, 1.0);
    (v / 20.0).min(1.0) * (0.45 + 0.55 * (1.0 - u))
}

fn approach(cur: f32, target: f32, max_delta: f32) -> f32 {
    if cur < target { (cur + max_delta).min(target) } else { (cur - max_delta).max(target) }
}

impl Car {
    pub fn new(x: f32) -> Self {
        Self {
            s: 0.0,
            x,
            v: 0.0,
            steer: 0.0,
            yaw: 0.0,
            lateral_vel: 0.0,
            offroad: false,
            drift: Drift::default(),
            nitro: TUNING.nitro_start,
            nitro_active: false,
            boost_time: 0.0,
            boost_gain: 0.0,
            stun: 0.0,
            spin: 0.0,
            wobble: 0.0,
            gear: 1,
            rpm: TUNING.idle_rpm,
            shift_timer: 0.0,
            brake_tap: 0.0,
            slip: 0.0,
        }
    }

    pub fn view(&self) -> CarView {
        CarView {
            s: self.s,
            x: self.x,
            v: self.v,
            steer: self.steer,
            yaw: self.yaw + self.spin,
            offroad: self.offroad,
            drifting: self.drift.active,
            drift_tier: self.drift.tier,
            nitro_active: self.nitro_active,
            boosting: self.boost_time > 0.0,
            stunned: self.stun > 0.0,
        }
    }

    /// Current top speed including nitro and drift boost.
    pub fn top_speed(&self) -> f32 {
        let t = &TUNING;
        let mut top = t.v_max;
        if self.nitro_active {
            top *= 1.0 + t.nitro_top_gain;
        }
        if self.boost_time > 0.0 {
            top *= 1.0 + self.boost_gain;
        }
        top
    }

    pub fn add_nitro(&mut self, amount: f32) {
        let k = if self.nitro_active { 1.5 } else { 1.0 };
        self.nitro = (self.nitro + amount * k).min(100.0);
    }

    pub fn stun(&mut self, seconds: f32) {
        self.stun = self.stun.max(seconds);
        self.end_drift(false, &mut Vec::new());
        self.nitro_active = false;
    }

    pub fn start_wobble(&mut self) {
        self.wobble = TUNING.wobble_time;
        self.end_drift(false, &mut Vec::new());
    }

    fn end_drift(&mut self, award: bool, events: &mut Vec<SimEvent>) {
        if !self.drift.active {
            return;
        }
        let tier = self.drift.tier;
        self.drift = Drift::default();
        if award && tier > 0 {
            let (gain, secs, nitro) = TUNING.drift_boosts[tier as usize - 1];
            self.boost_gain = gain;
            self.boost_time = secs;
            self.add_nitro(nitro);
            events.push(SimEvent::DriftBoost { tier });
        } else {
            events.push(SimEvent::DriftEnd);
        }
    }

    pub fn step(&mut self, track: &Track, input: &Input, dt: f32, events: &mut Vec<SimEvent>) {
        let t = &TUNING;
        let controllable = self.stun <= 0.0;
        let target = if controllable { input.steer.clamp(-1.0, 1.0) } else { 0.0 };
        let throttle = if controllable { input.throttle.clamp(0.0, 1.0) } else { 0.0 };
        let brake = if controllable { input.brake.clamp(0.0, 1.0) } else { 0.0 };

        // Steering shaping: ramp toward input, recentre faster.
        let recentring = target.abs() < self.steer.abs() || target * self.steer < 0.0;
        let time = if recentring { t.steer_out_time } else { t.steer_in_time };
        self.steer = approach(self.steer, target, dt / time);

        // --- drift state ---
        let tapped_brake = if brake > 0.5 {
            self.brake_tap += dt;
            false
        } else {
            let tap = self.brake_tap > 0.0 && self.brake_tap < 0.2;
            self.brake_tap = 0.0;
            tap
        };
        if !self.drift.active
            && controllable
            && self.v > t.drift_min_speed
            && self.steer.abs() > 0.3
            && (input.drift || tapped_brake)
            && self.wobble <= 0.0
        {
            self.drift = Drift { active: true, dir: self.steer.signum(), by_button: input.drift, ..Default::default() };
            events.push(SimEvent::DriftStart);
        }
        if self.drift.active {
            let tight = self.steer * self.drift.dir > 0.66;
            self.drift.charge += if tight { 2.0 * dt } else { dt };
            let new_tier = t.drift_tiers.iter().filter(|&&c| self.drift.charge >= c).count() as u8;
            if new_tier > self.drift.tier {
                self.drift.tier = new_tier;
                events.push(SimEvent::DriftTier { tier: new_tier });
            }
            self.add_nitro(t.drift_nitro_per_s * dt);
            self.drift.centre_time = if self.steer.abs() < 0.15 { self.drift.centre_time + dt } else { 0.0 };
            let released = self.drift.by_button && !input.drift;
            if released || self.drift.centre_time > 0.15 || self.v < t.drift_min_speed * 0.75 {
                self.end_drift(true, events);
            }
        }

        // --- nitro ---
        let want_nitro = input.nitro && controllable && self.nitro > 0.0;
        if want_nitro && !self.nitro_active {
            events.push(SimEvent::NitroStart);
        } else if !want_nitro && self.nitro_active {
            events.push(SimEvent::NitroEnd);
        }
        self.nitro_active = want_nitro;
        if self.nitro_active {
            self.nitro = (self.nitro - t.nitro_drain * dt).max(0.0);
        }
        if self.boost_time > 0.0 {
            self.boost_time -= dt;
        }

        // --- longitudinal ---
        let top = self.top_speed();
        let u = (self.v / top).clamp(0.0, 1.0);
        let mut accel = t.accel * (1.0 - u) * (1.0 + 0.5 * u);
        if self.nitro_active {
            accel *= 1.0 + t.nitro_accel_gain;
        }
        if self.boost_time > 0.0 {
            accel *= 1.0 + 2.0 * self.boost_gain;
        }
        if self.shift_timer > 0.0 {
            self.shift_timer -= dt;
            accel = 0.0; // torque cut during the shift
        }
        self.offroad = self.x.abs() > ROAD_HALF_WIDTH + 0.6;
        let cap = t.offroad_speed * t.v_max;
        if self.offroad && self.v > cap {
            // Off-road above the cap: no drive, ~2 g of drag down to the cap.
            self.v = (self.v - t.offroad_decel * dt).max(cap);
        } else {
            self.v += throttle * accel * dt;
            if self.offroad {
                self.v = self.v.min(cap);
            }
        }
        self.v -= brake * t.brake * dt;
        if throttle < 0.05 {
            self.v -= t.coast_drag * dt;
        }
        if self.drift.active {
            self.v -= t.drift_drag * dt;
        }
        if !controllable {
            self.v -= 1.3 * G * dt; // skidding to a halt
        }
        if self.v > top {
            // Above the (possibly just-reduced) top speed: bleed off gently.
            self.v = (self.v - 0.4 * G * dt).max(top);
        }
        self.v = self.v.max(0.0);

        // --- lateral: steering vs centrifugal push from the curve ---
        let mut pen = if self.offroad { t.offroad_steer } else { 1.0 };
        if self.wobble > 0.0 {
            pen *= 0.5;
        }
        let cf = if self.drift.active { t.centrifugal * t.drift_centrifugal } else { t.centrifugal };
        let curve = track.curve_at(self.s);
        let steer_part = if controllable { self.steer * t.steer_rate * grip(self.v) * pen } else { 0.0 };
        let lat = steer_part - curve * self.v * self.v * cf;
        self.lateral_vel = lat;
        self.x += lat * dt;
        if self.x.abs() > t.x_limit {
            self.x = t.x_limit * self.x.signum();
            self.v -= self.v * t.scrape_decay * dt;
        }

        // --- visual yaw, spin and wobble ---
        let yaw_target = if self.drift.active {
            self.drift.dir * (0.35 + 0.25 * (self.steer * self.drift.dir).max(0.0))
        } else {
            // Floor the divisor so low-speed steering doesn't spin the body round.
            (lat / self.v.max(18.0)).clamp(-0.3, 0.3)
        };
        let yaw_rate = if self.drift.active { 5.0 } else { 8.0 };
        self.yaw += (yaw_target - self.yaw) * (dt * yaw_rate).min(1.0);
        if self.stun > 0.0 {
            self.stun -= dt;
            self.spin += dt * (4.0 + self.v * 0.15);
        } else {
            // Settle any leftover spin to the nearest full turn.
            let turns = (self.spin / core::f32::consts::TAU + 0.5).floor();
            self.spin += (turns * core::f32::consts::TAU - self.spin) * (dt * 6.0).min(1.0);
        }
        if self.wobble > 0.0 {
            self.wobble -= dt;
        }

        // --- slip (for audio / smoke) ---
        self.slip = if self.drift.active {
            0.75 + 0.25 * self.steer.abs()
        } else if self.stun > 0.0 && self.v > 3.0 {
            1.0
        } else {
            ((lat.abs() - 7.0) / 8.0).clamp(0.0, 1.0) * (self.v / 30.0).min(1.0)
        };

        // --- gears and rpm ---
        let frac = self.v / t.v_max;
        let g = self.gear as usize;
        if g < 6 && frac > t.gear_tops[g - 1] * 0.97 {
            self.gear += 1;
            self.shift_timer = t.shift_cut;
            events.push(SimEvent::GearShift { gear: self.gear });
        } else if g > 1 && frac < t.gear_tops[g - 2] * 0.85 {
            self.gear -= 1;
        }
        let lo = if self.gear > 1 { t.gear_tops[self.gear as usize - 2] * 0.85 } else { 0.0 };
        let hi = t.gear_tops[self.gear as usize - 1];
        let r = ((frac - lo) / (hi - lo)).clamp(0.0, 1.05);
        self.rpm = t.idle_rpm + r * (t.redline_rpm - t.idle_rpm);

        self.s = track.wrap(self.s + self.v * dt);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DT, STEP_HZ, World};

    fn straight_track() -> Track {
        Track::straight(4000)
    }

    fn drive(car: &mut Car, track: &Track, input: Input, seconds: f32) -> Vec<SimEvent> {
        let mut ev = Vec::new();
        for _ in 0..(seconds / DT).round() as usize {
            car.step(track, &input, DT, &mut ev);
        }
        ev
    }

    #[test]
    fn zero_to_200_kmh_in_five_to_seven_seconds() {
        let track = straight_track();
        let mut car = Car::new(0.0);
        let full = Input { throttle: 1.0, ..Default::default() };
        let mut t = 0.0;
        let mut ev = Vec::new();
        while car.v < 200.0 / 3.6 {
            car.step(&track, &full, DT, &mut ev);
            t += DT;
            assert!(t < 20.0);
        }
        assert!((5.0..7.5).contains(&t), "0-200 km/h took {t:.2}s");
    }

    #[test]
    fn speed_never_exceeds_top_speed_without_boost() {
        let mut w = World::new(5);
        w.traffic.cars.clear();
        w.traffic.row_gap = 1e9;
        for _ in 0..60 * STEP_HZ {
            let steer = crate::autopilot(&w, 2.25).steer;
            w.step(&Input { steer, throttle: 1.0, ..Default::default() });
            assert!(w.car.v <= TUNING.v_max + 1e-3);
        }
        assert!(w.car.v > TUNING.v_max * 0.97);
    }

    #[test]
    fn no_steering_at_standstill() {
        let track = straight_track();
        let mut car = Car::new(1.0);
        drive(&mut car, &track, Input { steer: 1.0, ..Default::default() }, 1.0);
        assert_eq!(car.x, 1.0);
    }

    #[test]
    fn steering_ramps_in_over_about_120ms() {
        let track = straight_track();
        let mut car = Car::new(0.0);
        car.v = 30.0;
        let right = Input { steer: 1.0, throttle: 1.0, ..Default::default() };
        drive(&mut car, &track, right, 0.06);
        assert!((0.4..0.6).contains(&car.steer), "steer after 60ms = {}", car.steer);
        drive(&mut car, &track, right, 0.08);
        assert_eq!(car.steer, 1.0);
    }

    #[test]
    fn hard_curve_at_top_speed_needs_most_of_the_steering() {
        let push = 0.0016 * TUNING.v_max * TUNING.v_max * TUNING.centrifugal;
        let authority = TUNING.steer_rate * grip(TUNING.v_max);
        let needed = push / authority;
        assert!((0.65..0.85).contains(&needed), "hard curve needs {:.0}% steer", needed * 100.0);
    }

    #[test]
    fn offroad_slows_toward_the_cap_and_braking_stops_the_car() {
        let track = straight_track();
        let mut car = Car::new(ROAD_HALF_WIDTH + 3.0);
        car.v = TUNING.v_max;
        drive(&mut car, &track, Input { throttle: 1.0, ..Default::default() }, 3.0);
        assert!(car.offroad);
        assert!((car.v - TUNING.offroad_speed * TUNING.v_max).abs() < 1.0, "v = {}", car.v);
        car.x = 0.0;
        let mut t = 0.0;
        let mut ev = Vec::new();
        while car.v > 0.0 {
            car.step(&track, &Input { brake: 1.0, ..Default::default() }, DT, &mut ev);
            t += DT;
        }
        assert!(t < 3.0);
    }

    #[test]
    fn drift_tiers_charge_at_08_18_30_seconds_and_release_boosts() {
        let track = straight_track();
        let mut car = Car::new(0.0);
        car.v = 50.0;
        // Moderate (not tight) steer so charge runs at 1x.
        let hold = Input { steer: 0.5, throttle: 1.0, drift: true, ..Default::default() };
        let mut ev = Vec::new();
        let mut tier_times = Vec::new();
        let mut t = 0.0;
        while t < 3.3 {
            car.step(&track, &hold, DT, &mut ev);
            t += DT;
            for e in ev.drain(..) {
                if let SimEvent::DriftTier { .. } = e {
                    tier_times.push(t);
                }
            }
            car.x = 0.0;
        }
        assert_eq!(tier_times.len(), 3, "{tier_times:?}");
        for (got, want) in tier_times.iter().zip([0.8, 1.8, 3.0]) {
            // Charge starts after the 120 ms steer ramp passes 0.3.
            assert!((got - want).abs() < 0.12, "tier at {got:.3}s, want ~{want}s");
        }
        let nitro_before = car.nitro;
        car.step(&track, &Input { steer: 0.5, throttle: 1.0, ..Default::default() }, DT, &mut ev);
        assert!(ev.iter().any(|e| matches!(e, SimEvent::DriftBoost { tier: 3 })));
        assert!(car.boost_time > 1.7 && car.nitro > nitro_before);
    }

    #[test]
    fn tight_drift_charges_twice_as_fast() {
        let track = straight_track();
        let mut car = Car::new(0.0);
        car.v = 50.0;
        let ev = drive(&mut car, &track, Input { steer: 1.0, throttle: 1.0, drift: true, ..Default::default() }, 1.0);
        assert!(ev.iter().any(|e| matches!(e, SimEvent::DriftTier { tier: 2 })), "{ev:?}");
    }

    #[test]
    fn nitro_drains_at_25_per_second_and_lifts_top_speed() {
        let track = straight_track();
        let mut car = Car::new(0.0);
        car.v = TUNING.v_max;
        car.nitro = 100.0;
        drive(&mut car, &track, Input { throttle: 1.0, nitro: true, ..Default::default() }, 2.0);
        assert!((car.nitro - 50.0).abs() < 0.5, "nitro {}", car.nitro);
        assert!(car.v > TUNING.v_max * 1.05);
        drive(&mut car, &track, Input { throttle: 1.0, nitro: true, ..Default::default() }, 2.5);
        assert_eq!(car.nitro, 0.0);
        assert!(!car.nitro_active);
    }

    #[test]
    fn gears_shift_up_through_six_and_rpm_stays_in_range() {
        let track = straight_track();
        let mut car = Car::new(0.0);
        let mut ev = Vec::new();
        for _ in 0..(30.0 / DT) as usize {
            car.step(&track, &Input { throttle: 1.0, ..Default::default() }, DT, &mut ev);
            assert!(car.rpm >= TUNING.idle_rpm && car.rpm <= TUNING.redline_rpm * 1.06);
        }
        let shifts: Vec<u8> =
            ev.iter().filter_map(|e| if let SimEvent::GearShift { gear } = e { Some(*gear) } else { None }).collect();
        assert_eq!(shifts, vec![2, 3, 4, 5, 6]);
    }
}
