//! Arcade handling model (GAME_DESIGN §4). Units: metres, seconds, m/s.

use crate::CarView;
use crate::track::{ROAD_HALF_WIDTH, Track};

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
};

/// Player input for one step. `steer` −1 (left) … 1 (right); throttle/brake 0 … 1.
#[derive(Clone, Copy, Debug, Default)]
pub struct Input {
    pub steer: f32,
    pub throttle: f32,
    pub brake: f32,
}

#[derive(Clone, Debug)]
pub struct Car {
    pub s: f32,
    pub x: f32,
    pub v: f32,
    /// Shaped steering (−1 … 1).
    pub steer: f32,
    /// Visual yaw (rad), eased toward the direction of lateral travel.
    pub yaw: f32,
    pub lateral_vel: f32,
    pub offroad: bool,
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
        Self { s: 0.0, x, v: 0.0, steer: 0.0, yaw: 0.0, lateral_vel: 0.0, offroad: false }
    }

    pub fn view(&self) -> CarView {
        CarView { s: self.s, x: self.x, v: self.v, steer: self.steer, yaw: self.yaw, offroad: self.offroad }
    }

    pub fn step(&mut self, track: &Track, input: &Input, dt: f32) {
        let t = &TUNING;
        let target = input.steer.clamp(-1.0, 1.0);
        let throttle = input.throttle.clamp(0.0, 1.0);
        let brake = input.brake.clamp(0.0, 1.0);

        // Steering shaping: ramp toward input, recentre faster.
        let recentring = target.abs() < self.steer.abs() || target * self.steer < 0.0;
        let time = if recentring { t.steer_out_time } else { t.steer_in_time };
        self.steer = approach(self.steer, target, dt / time);

        // Longitudinal.
        let u = (self.v / t.v_max).clamp(0.0, 1.0);
        let accel = t.accel * (1.0 - u) * (1.0 + 0.5 * u);
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
        self.v = self.v.clamp(0.0, t.v_max);

        // Lateral: steering vs centrifugal push from the curve.
        let pen = if self.offroad { t.offroad_steer } else { 1.0 };
        let curve = track.curve_at(self.s);
        let lat = self.steer * t.steer_rate * grip(self.v) * pen - curve * self.v * self.v * t.centrifugal;
        self.lateral_vel = lat;
        self.x += lat * dt;
        if self.x.abs() > t.x_limit {
            self.x = t.x_limit * self.x.signum();
            self.v -= self.v * t.scrape_decay * dt;
        }

        // Visual yaw eases toward the direction of travel relative to the road.
        // Floor the divisor so low-speed steering doesn't spin the body round.
        let yaw_target = (lat / self.v.max(18.0)).clamp(-0.3, 0.3);
        self.yaw += (yaw_target - self.yaw) * (dt * 8.0).min(1.0);

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

    #[test]
    fn zero_to_200_kmh_in_five_to_seven_seconds() {
        let track = straight_track();
        let mut car = Car::new(0.0);
        let full = Input { throttle: 1.0, ..Default::default() };
        let mut t = 0.0;
        while car.v < 200.0 / 3.6 {
            car.step(&track, &full, DT);
            t += DT;
            assert!(t < 20.0);
        }
        assert!((5.0..7.0).contains(&t), "0-200 km/h took {t:.2}s");
    }

    #[test]
    fn speed_never_exceeds_top_speed() {
        let mut w = World::new(5);
        for _ in 0..60 * STEP_HZ {
            let steer = crate::autopilot(&w, 2.25).steer;
            w.step(&Input { steer, throttle: 1.0, brake: 0.0 });
            assert!(w.car.v <= TUNING.v_max + 1e-3);
        }
        assert!(w.car.v > TUNING.v_max * 0.97);
    }

    #[test]
    fn no_steering_at_standstill() {
        let track = straight_track();
        let mut car = Car::new(1.0);
        for _ in 0..120 {
            car.step(&track, &Input { steer: 1.0, ..Default::default() }, DT);
        }
        assert_eq!(car.x, 1.0);
    }

    #[test]
    fn steering_ramps_in_over_about_120ms() {
        let track = straight_track();
        let mut car = Car::new(0.0);
        car.v = 30.0;
        let right = Input { steer: 1.0, throttle: 1.0, brake: 0.0 };
        for _ in 0..(0.06 / DT) as usize {
            car.step(&track, &right, DT);
        }
        assert!((0.4..0.6).contains(&car.steer), "steer after 60ms = {}", car.steer);
        for _ in 0..(0.08 / DT) as usize {
            car.step(&track, &right, DT);
        }
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
        for _ in 0..(3.0 / DT) as usize {
            car.step(&track, &Input { throttle: 1.0, ..Default::default() }, DT);
        }
        assert!(car.offroad);
        assert!((car.v - TUNING.offroad_speed * TUNING.v_max).abs() < 1.0, "v = {}", car.v);

        car.x = 0.0;
        let mut t = 0.0;
        while car.v > 0.0 {
            car.step(&track, &Input { brake: 1.0, ..Default::default() }, DT);
            t += DT;
        }
        assert!(t < 3.0, "braking from {} m/s took {t:.2}s", TUNING.offroad_speed * TUNING.v_max);
    }
}
