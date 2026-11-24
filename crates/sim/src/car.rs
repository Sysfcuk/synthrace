//! Arcade handling model (GAME_DESIGN §4, docs/research/HANDLING.md). Units: metres, seconds.
//!
//! Velocity-alignment ("bicycle-lite") model in track space:
//! - `heading` ψ: where the body points, relative to the road tangent;
//! - `travel` θ: direction of the velocity, relative to the road tangent;
//! - steering demands a world yaw rate (capped by geometry at low speed, by grip at high speed),
//!   the body yaws with inertia, and tyre grip pulls the velocity toward the heading.
//!   The road's own curvature turns underneath the car, which is what pushes you wide in bends.

use crate::dmath::{cos, sin};
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
    /// Steer input ramp-in / recentre times (s) for digital input.
    pub steer_in_time: f32,
    pub steer_out_time: f32,
    // --- chassis / tyres ---
    pub wheelbase: f32,
    /// tan of the maximum steering angle at low speed.
    pub tan_steer_max: f32,
    /// Tyre friction coefficient on tarmac / off-road / while drifting (rear grip broken).
    pub mu_road: f32,
    pub mu_offroad: f32,
    pub mu_drift: f32,
    /// Yaw-rate response (1/s): higher = snappier turn-in.
    pub yaw_response: f32,
    /// Velocity-to-heading alignment rate (1/s) when gripping / drifting / stunned.
    pub align_grip: f32,
    pub align_drift: f32,
    pub align_stunned: f32,
    /// Fraction of road curvature followed automatically (arcade assist).
    pub road_assist: f32,
    /// Heading recentre rate (1/s) when not steering and not sliding (arcade assist).
    pub recentre: f32,
    /// Speed lost to cornering load (m/s² at the grip limit) and to sliding (m/s² at 90° slip).
    pub scrub_turn: f32,
    pub scrub_slide: f32,
    /// Max heading relative to the road while in control (rad).
    pub max_heading: f32,
    // --- off-road ---
    pub offroad_speed: f32,
    pub offroad_decel: f32,
    /// Hard verge limit (m); hitting it scrapes speed.
    pub x_limit: f32,
    pub scrape_decay: f32,
    // --- drift ---
    pub drift_min_speed: f32,
    pub drift_drag: f32,
    /// Yaw authority multiplier and inward rotation bias while drifting.
    pub drift_yaw_gain: f32,
    pub drift_bias: f32,
    pub drift_max_slip: f32,
    /// Slip angle (rad) that counts as "drifting" for charge, and as "tight" (2× charge).
    pub drift_charge_slip: f32,
    pub drift_tight_slip: f32,
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
    pub mass: f32,
    /// Arcade mass used against light traffic (lets you "check" cars out of the way).
    pub check_mass: f32,
    /// Speed change (m/s) from an impact above which the player spins out.
    pub spin_dv: f32,
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
    steer_in_time: 0.12,
    steer_out_time: 0.08,
    wheelbase: 2.6,
    tan_steer_max: 0.61, // ≈ 31°
    mu_road: 1.4,
    mu_offroad: 0.8,
    mu_drift: 1.25,
    yaw_response: 9.0,
    align_grip: 14.0,
    align_drift: 2.2,
    align_stunned: 0.6,
    road_assist: 0.15,
    recentre: 0.35,
    scrub_turn: 1.6,
    scrub_slide: 7.0,
    max_heading: 1.0,
    offroad_speed: 0.38,
    offroad_decel: 2.0 * G,
    x_limit: ROAD_HALF_WIDTH + 11.0,
    scrape_decay: 1.5,
    drift_min_speed: 120.0 / 3.6,
    drift_drag: 0.1 * G,
    drift_yaw_gain: 1.8,
    drift_bias: 0.3,
    drift_max_slip: 0.62,
    drift_charge_slip: 0.12,
    drift_tight_slip: 0.32,
    drift_tiers: [0.8, 1.8, 3.0],
    drift_boosts: [(0.08, 0.6, 5.0), (0.12, 1.2, 8.0), (0.15, 1.8, 12.0)],
    drift_nitro_per_s: 3.0,
    nitro_drain: 25.0,
    nitro_top_gain: 0.2,
    nitro_accel_gain: 0.25,
    nitro_start: 34.0,
    half_width: 0.95,
    half_length: 2.2,
    mass: 1300.0,
    check_mass: 2600.0,
    spin_dv: 14.0,
    spin_time: 1.5,
    crash_time: 2.2,
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
    /// Accumulated charge (s); doubles while the slip angle is tight.
    pub charge: f32,
    pub tier: u8,
    settle_time: f32,
    pub(crate) by_button: bool,
}

#[derive(Clone, Debug)]
pub struct Car {
    pub s: f32,
    pub x: f32,
    /// Speed along the direction of travel (m/s).
    pub v: f32,
    /// Shaped steering (−1 … 1).
    pub steer: f32,
    /// Body heading relative to the road tangent (rad, + = pointing right).
    pub heading: f32,
    /// Velocity direction relative to the road tangent (rad).
    pub travel: f32,
    /// World yaw rate of the body (rad/s).
    pub yaw_rate: f32,
    /// Lateral / longitudinal acceleration felt by the body (m/s²), for visuals and audio.
    pub lat_accel: f32,
    pub long_accel: f32,
    pub offroad: bool,
    pub drift: Drift,
    /// Nitro meter 0..100.
    pub nitro: f32,
    pub nitro_active: bool,
    pub boost_time: f32,
    pub boost_gain: f32,
    /// Seconds of lost control remaining (spin-out / crash).
    pub stun: f32,
    /// Extra visual rotation during a spin-out (rad).
    pub spin: f32,
    pub wobble: f32,
    pub gear: u8,
    pub rpm: f32,
    shift_timer: f32,
    brake_tap: f32,
    /// Tyre slip 0..1 (drives squeal audio and smoke).
    pub slip: f32,
}

/// Maximum world yaw rate (rad/s) at speed `v` with friction `mu`: steering geometry at low
/// speed, tyre grip at high speed.
pub fn yaw_cap(v: f32, mu: f32) -> f32 {
    let t = &TUNING;
    let geo = v * t.tan_steer_max / t.wheelbase;
    let grip = mu * G / v.max(1.0);
    geo.min(grip)
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
            heading: 0.0,
            travel: 0.0,
            yaw_rate: 0.0,
            lat_accel: 0.0,
            long_accel: 0.0,
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
            yaw: self.heading + self.spin,
            lat_accel: self.lat_accel,
            long_accel: self.long_accel,
            offroad: self.offroad,
            drifting: self.drift.active,
            drift_tier: self.drift.tier,
            nitro_active: self.nitro_active,
            boosting: self.boost_time > 0.0,
            stunned: self.stun > 0.0,
        }
    }

    /// Velocity components in track space: (along the road, across the road).
    pub fn velocity(&self) -> (f32, f32) {
        (self.v * cos(self.travel), self.v * sin(self.travel))
    }

    /// Sets the velocity from track-space components (after an impulse).
    pub fn set_velocity(&mut self, vs: f32, vx: f32) {
        self.v = (vs * vs + vx * vx).sqrt();
        if self.v > 0.05 {
            self.travel = crate::dmath::atan2(vx, vs);
        }
    }

    /// Lateral velocity (m/s), + = moving right.
    pub fn lateral_vel(&self) -> f32 {
        self.v * sin(self.travel)
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
        let v0 = self.v;

        // Steering shaping: ramp toward input, recentre faster.
        let recentring = target.abs() < self.steer.abs() || target * self.steer < 0.0;
        let time = if recentring { t.steer_out_time } else { t.steer_in_time };
        self.steer = approach(self.steer, target, dt / time);

        // --- drift state ---
        // Brake-tap drift: a short brake tap while staying on the throttle.
        let tapped_brake = if brake > 0.5 {
            if throttle > 0.5 {
                self.brake_tap += dt;
            } else {
                self.brake_tap = 1.0; // braking off-throttle is just braking
            }
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
        let slip_angle = self.heading - self.travel;
        if self.drift.active {
            let slipping = slip_angle * self.drift.dir;
            if slipping > t.drift_charge_slip {
                self.drift.charge += if slipping > t.drift_tight_slip { 2.0 * dt } else { dt };
            }
            let new_tier = t.drift_tiers.iter().filter(|&&c| self.drift.charge >= c).count() as u8;
            if new_tier > self.drift.tier {
                self.drift.tier = new_tier;
                events.push(SimEvent::DriftTier { tier: new_tier });
            }
            self.add_nitro(t.drift_nitro_per_s * dt);
            // Settled: steering centred and the slide has straightened out.
            let countering = self.steer * self.drift.dir < -0.5;
            self.drift.settle_time =
                if (self.steer.abs() < 0.15 || countering) && slipping < 0.08 { self.drift.settle_time + dt } else { 0.0 };
            let released = self.drift.by_button && !input.drift;
            if released || self.drift.settle_time > 0.2 || self.v < t.drift_min_speed * 0.75 {
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
            self.v -= 1.1 * G * dt; // skidding to a halt
        }
        if self.v > top {
            self.v = (self.v - 0.4 * G * dt).max(top);
        }
        self.v = self.v.max(0.0);

        // --- yaw: steering demands a yaw rate; the body responds with inertia ---
        let mu = if self.offroad {
            t.mu_offroad
        } else if self.drift.active {
            t.mu_drift
        } else {
            t.mu_road
        };
        let kappa = track.curve_at(self.s);
        let road_rate = kappa * self.v; // how fast the road tangent turns under us
        let mut cap_r = yaw_cap(self.v, mu);
        if self.wobble > 0.0 {
            cap_r *= 0.5;
        }
        let mut r_target = if controllable { self.steer * cap_r } else { 0.0 };
        r_target += road_rate * t.road_assist;
        if !controllable {
            r_target = self.yaw_rate; // keep spinning / sliding as it was
        }
        if self.drift.active {
            // Rear grip broken: the body holds a slip angle to the direction of travel —
            // wider when steering into the drift, narrower with countersteer — while the
            // velocity carves round at the grip limit (below).
            let into = (self.steer * self.drift.dir).clamp(-1.0, 1.0);
            // Full countersteer (into = −1) straightens the slide completely.
            let target_slip = self.drift.dir * (t.drift_bias + t.drift_yaw_gain * 0.15 * into).clamp(0.0, t.drift_max_slip);
            // The path carves with the drift, and countersteer turns it the other way.
            let carve = cap_r * (0.4 * self.drift.dir + 0.6 * self.steer) + road_rate * t.road_assist;
            r_target = carve + (target_slip - slip_angle) * 7.0;
            self.yaw_rate += (r_target - self.yaw_rate) * (2.0 * t.yaw_response * dt).min(1.0);
        } else {
            self.yaw_rate += (r_target - self.yaw_rate) * (t.yaw_response * dt).min(1.0);
        }
        self.heading += (self.yaw_rate - road_rate) * dt;

        // Arcade assist: with no steering and little slip, ease the body back toward the road line.
        if controllable && !self.drift.active && self.steer.abs() < 0.05 && slip_angle.abs() < 0.08 {
            self.heading -= self.heading * (t.recentre * dt).min(1.0);
        }
        if controllable {
            self.heading = self.heading.clamp(-t.max_heading, t.max_heading);
        }

        // --- grip pulls the velocity toward the heading (limited by available friction) ---
        let align = if !controllable {
            t.align_stunned
        } else if self.drift.active {
            t.align_drift
        } else {
            t.align_grip
        };
        let grip_rate = mu * G / self.v.max(1.0);
        let world_turn = (align * (self.heading - self.travel) + road_rate).clamp(-grip_rate, grip_rate);
        self.travel += (world_turn - road_rate) * dt;
        if self.drift.active {
            // Cap the drift angle: beyond it the rear "catches".
            let slip = self.heading - self.travel;
            if slip.abs() > t.drift_max_slip {
                self.heading = self.travel + t.drift_max_slip * slip.signum();
                self.yaw_rate = road_rate + world_turn;
            }
        }

        // --- scrub: cornering load and sliding cost speed ---
        let lat = self.v * world_turn; // centripetal acceleration of the path (world frame)
        let slide = sin(self.heading - self.travel).abs();
        let scrub =
            t.scrub_turn * (lat.abs() / (mu * G)).min(1.5) + t.scrub_slide * slide * if self.drift.active { 0.35 } else { 1.0 };
        self.v = (self.v - scrub * dt).max(0.0);

        // --- integrate position ---
        let vs = self.v * cos(self.travel);
        let vx = self.v * sin(self.travel);
        self.x += vx * dt;
        if self.x.abs() > t.x_limit {
            self.x = t.x_limit * self.x.signum();
            self.v -= self.v * t.scrape_decay * dt;
            self.travel *= 0.5;
            self.heading *= 0.7;
        }
        self.s = track.wrap(self.s + vs.max(0.0) * dt);

        self.lat_accel += (lat - self.lat_accel) * (dt * 10.0).min(1.0);
        self.long_accel += ((self.v - v0) / dt - self.long_accel) * (dt * 8.0).min(1.0);

        // --- spin-out visuals ---
        if self.stun > 0.0 {
            self.stun -= dt;
            self.spin += dt * (4.0 + self.v * 0.15);
            if self.stun <= 0.0 {
                // Regain control facing down the road the way we're now sliding.
                self.heading = self.travel.clamp(-0.4, 0.4);
                self.travel = self.heading;
                self.yaw_rate = road_rate;
            }
        } else {
            let turns = (self.spin / core::f32::consts::TAU + 0.5).floor();
            self.spin += (turns * core::f32::consts::TAU - self.spin) * (dt * 6.0).min(1.0);
        }
        if self.wobble > 0.0 {
            self.wobble -= dt;
        }

        // --- slip (for audio / smoke) ---
        let load = (lat.abs() / (mu * G) - 0.75).max(0.0) * 3.0;
        self.slip =
            if self.stun > 0.0 && self.v > 3.0 { 1.0 } else { (slide * 4.0 + load).clamp(0.0, 1.0) * (self.v / 25.0).min(1.0) };

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
        assert!(w.car.v > TUNING.v_max * 0.9, "v = {}", w.car.v);
    }

    #[test]
    fn no_steering_at_standstill() {
        let track = straight_track();
        let mut car = Car::new(1.0);
        drive(&mut car, &track, Input { steer: 1.0, ..Default::default() }, 1.0);
        assert_eq!(car.x, 1.0);
        assert_eq!(car.heading, 0.0);
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
        // To hold a hard bend the path must turn at κ·v; full steer delivers the grip limit.
        let kappa = 0.0016;
        let v = TUNING.v_max;
        let needed = kappa * v * (1.0 - TUNING.road_assist) / yaw_cap(v, TUNING.mu_road);
        assert!((0.6..0.85).contains(&needed), "hard curve needs {:.0}% steer", needed * 100.0);
    }

    #[test]
    fn lateral_motion_has_momentum_it_does_not_glide_to_a_stop() {
        // Steer right briefly, then let go: the car keeps travelling right (it points that way)
        // and needs countersteer to straighten up — it no longer stops sideways instantly.
        let track = straight_track();
        let mut car = Car::new(0.0);
        car.v = 50.0;
        drive(&mut car, &track, Input { steer: 1.0, throttle: 1.0, ..Default::default() }, 0.4);
        let x_release = car.x;
        assert!(car.heading > 0.05, "heading {}", car.heading);
        drive(&mut car, &track, Input { throttle: 1.0, ..Default::default() }, 0.5);
        assert!(car.x - x_release > 2.0, "kept moving only {:.2} m", car.x - x_release);
        // Countersteer brings the heading back.
        drive(&mut car, &track, Input { steer: -1.0, throttle: 1.0, ..Default::default() }, 0.6);
        assert!(car.heading < 0.05, "heading {}", car.heading);
    }

    #[test]
    fn yaw_is_grip_limited_at_speed_and_turning_scrubs_speed() {
        let track = straight_track();
        let mut a = Car::new(0.0);
        a.v = 70.0;
        let mut b = a.clone();
        drive(&mut a, &track, Input { steer: 1.0, ..Default::default() }, 1.0);
        drive(&mut b, &track, Input::default(), 1.0);
        assert!(a.yaw_rate <= yaw_cap(a.v, TUNING.mu_road) * 1.01 + 1e-3, "{} > {}", a.yaw_rate, yaw_cap(a.v, TUNING.mu_road));
        assert!(a.v < b.v - 0.5, "turning should cost speed: {} vs {}", a.v, b.v);
    }

    #[test]
    fn unsteered_car_drifts_wide_in_a_bend() {
        // Road curving right: with no steering, the car runs wide (to the left of the curve's inside).
        let mut track = Track::straight(4000);
        for s in track.segments_mut().iter_mut() {
            s.curve = 0.001;
        }
        let mut car = Car::new(0.0);
        car.v = 60.0;
        drive(&mut car, &track, Input { throttle: 0.5, ..Default::default() }, 2.0);
        assert!(car.x < -2.0, "x = {}", car.x);
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
    fn drift_slides_charges_tiers_and_releases_a_boost() {
        let track = straight_track();
        let mut car = Car::new(0.0);
        car.v = 55.0;
        let hold = Input { steer: 0.5, throttle: 1.0, drift: true, ..Default::default() };
        let mut ev = Vec::new();
        let mut tier_times = Vec::new();
        let mut max_slip: f32 = 0.0;
        let mut t = 0.0;
        while t < 4.0 {
            car.step(&track, &hold, DT, &mut ev);
            t += DT;
            max_slip = max_slip.max((car.heading - car.travel).abs());
            for e in ev.drain(..) {
                if let SimEvent::DriftTier { .. } = e {
                    tier_times.push(t);
                }
            }
            car.x = 0.0; // stay on the road for the test
            car.v = car.v.max(45.0);
        }
        assert!(max_slip > 0.2, "drift should slide: max slip {max_slip}");
        assert_eq!(tier_times.len(), 3, "{tier_times:?}");
        assert!(tier_times[0] < 1.2 && tier_times[2] < 3.6, "{tier_times:?}");
        let nitro_before = car.nitro;
        car.step(&track, &Input { steer: 0.5, throttle: 1.0, ..Default::default() }, DT, &mut ev);
        assert!(ev.iter().any(|e| matches!(e, SimEvent::DriftBoost { tier: 3 })));
        assert!(car.boost_time > 1.7 && car.nitro > nitro_before);
    }

    #[test]
    fn countersteer_straightens_and_ends_a_drift() {
        let track = straight_track();
        let mut car = Car::new(0.0);
        car.v = 55.0;
        drive(&mut car, &track, Input { steer: 1.0, throttle: 1.0, drift: true, ..Default::default() }, 1.0);
        assert!(car.drift.active && car.heading - car.travel > 0.2);
        // Brake-tap style drift (not held by button): full countersteer must recover it.
        car.drift.by_button = false;
        let ev = drive(&mut car, &track, Input { steer: -1.0, throttle: 1.0, ..Default::default() }, 1.5);
        assert!(!car.drift.active, "drift should end");
        assert!((car.heading - car.travel).abs() < 0.1);
        assert!(ev.iter().any(|e| matches!(e, SimEvent::DriftBoost { .. } | SimEvent::DriftEnd)));
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
