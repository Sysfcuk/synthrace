//! Builds render frames from simulation snapshots: chase camera, car and scenery instances.
//! Platform-independent so the native `desktop` runner renders exactly what the browser does.

use crate::vfx::Vfx;
use procgen::{PLAYER_WHEELS, VehicleKind, WheelSlot, wheel_layout};
use render::{Camera, FrameParams, Fx, Instance, MAX_PALMS, MAX_TRAFFIC_PER_KIND, MAX_WHEELS, PROFILE_SAMPLES, PROFILE_STEP};
use sim::{Archetype, DT, TUNING, World, traffic::INDICATOR_TIME};

/// Distance from the camera to the car's centre along the track (m).
pub const CAR_Z: f32 = 7.2;
const CAM_HEIGHT: f32 = 2.35;
const BASE_PITCH: f32 = -0.075;
const DRAW_DISTANCE: f32 = 1250.0;
/// Visual exaggeration of the road's swing and hills (OutRun-style drama). Purely visual:
/// the bend is applied uniformly to everything drawn, and gameplay stays in track space.
const VISUAL_CURVE_GAIN: f32 = 1.8;
const VISUAL_HILL_GAIN: f32 = 1.3;

/// Body roll per g of lateral acceleration (rad) and pitch per g of longitudinal acceleration.
const ROLL_PER_G: f32 = 0.075;
const PITCH_PER_G: f32 = 0.04;
/// Suspension spring (1/s²) and damping ratio for the visual body motion.
const BODY_SPRING: f32 = 70.0;
const BODY_DAMPING: f32 = 0.45;
/// Chase camera: how much of the car's heading it turns with, and how quickly it catches up.
const CAM_YAW_FOLLOW: f32 = 0.7;
const CAM_POS_RATE: f32 = 10.0;
const CAM_YAW_RATE: f32 = 6.0;

/// Visual wheel spin is capped below the six-spoke strobe limit at 60 fps (rad/s).
const MAX_WHEEL_SPIN: f32 = 30.0;
/// Front-wheel steering angle at full lock (rad), visual only.
const WHEEL_STEER: f32 = 0.38;

/// Wheel instances for a vehicle at (x, z) with body yaw `yaw`, spin angle and steer angle.
fn push_wheels(out: &mut Vec<Instance>, slots: &[WheelSlot], x: f32, z: f32, yaw: f32, spin: f32, steer: f32) {
    let (s, c) = yaw.sin_cos();
    for w in slots {
        if out.len() >= MAX_WHEELS {
            return;
        }
        // Same rotation as the mesh shader's rot_y.
        let ox = w.x * c + w.z * s;
        let oz = -w.x * s + w.z * c;
        let wheel_yaw = yaw + if w.front { steer } else { 0.0 };
        out.push(Instance::new(x + ox, w.radius, z + oz, wheel_yaw, w.radius, 0.0).with_pitch(-spin));
    }
}

/// Damped spring for visual body motion (gives the car a sense of mass).
#[derive(Default, Clone, Copy)]
struct Spring {
    x: f32,
    v: f32,
}

impl Spring {
    fn step(&mut self, target: f32, dt: f32) -> f32 {
        let dt = dt.min(1.0 / 30.0);
        let c = 2.0 * BODY_DAMPING * BODY_SPRING.sqrt();
        let a = BODY_SPRING * (target - self.x) - c * self.v;
        self.v += a * dt;
        self.x += self.v * dt;
        self.x
    }
}

pub struct Scene {
    cam_x: f32,
    cam_yaw: f32,
    body_roll: Spring,
    body_pitch: Spring,
    wheel_angle: f32,
    wheels: Vec<Instance>,
    pitch: f32,
    fov: f32,
    profile: [[f32; 2]; PROFILE_SAMPLES],
    palms: Vec<Instance>,
    traffic: [Vec<Instance>; 4],
}

impl Default for Scene {
    fn default() -> Self {
        Self::new()
    }
}

impl Scene {
    pub fn new() -> Self {
        Self {
            cam_x: 0.0,
            cam_yaw: 0.0,
            body_roll: Spring::default(),
            body_pitch: Spring::default(),
            wheel_angle: 0.0,
            wheels: Vec::with_capacity(MAX_WHEELS),
            pitch: BASE_PITCH,
            fov: 60.0,
            profile: [[0.0; 2]; PROFILE_SAMPLES],
            palms: Vec::with_capacity(MAX_PALMS),
            traffic: Default::default(),
        }
    }

    /// Snap the camera to its targets (no smoothing), e.g. for screenshots.
    pub fn snap(&mut self, world: &World) {
        let car = world.car.view();
        self.cam_x = car.x;
        self.fov = fov_for(car.v);
        self.pitch = BASE_PITCH;
    }

    pub fn build<'a>(
        &'a mut self,
        world: &World,
        vfx: &'a mut Vfx,
        alpha: f32,
        time: f32,
        beat_phase: f32,
        dt: f32,
    ) -> FrameParams<'a> {
        let car = world.interpolated(alpha);
        let track = &world.track;
        let s_cam = track.wrap(car.s - CAR_Z);
        track.profile(s_cam, PROFILE_STEP, &mut self.profile);
        for p in self.profile.iter_mut() {
            p[0] *= VISUAL_CURVE_GAIN;
            p[1] *= VISUAL_HILL_GAIN;
        }

        // Chase camera: lags the car laterally; anticipates crests and dips.
        // Chase camera: sits behind the car along its (partly followed) heading so the car stays
        // centred; spin-outs don't drag the camera round.
        let heading = if car.stunned { 0.0 } else { car.yaw.clamp(-0.6, 0.6) };
        let yaw_target = heading * CAM_YAW_FOLLOW;
        self.cam_yaw += (yaw_target - self.cam_yaw) * (dt * CAM_YAW_RATE).min(1.0);
        let x_target = car.x - self.cam_yaw.sin() * CAR_Z;
        self.cam_x += (x_target - self.cam_x) * (dt * CAM_POS_RATE).min(1.0);
        let slope = (self.profile[2][1] - self.profile[0][1]) / (2.0 * PROFILE_STEP);
        let pitch_target = BASE_PITCH + slope.atan() * 0.55;
        self.pitch += (pitch_target - self.pitch) * (dt * 3.0).min(1.0);
        self.fov += (fov_for(car.v) + vfx.fov_kick() - self.fov) * (dt * 3.0).min(1.0);
        // Weight transfer: roll out of the turn, squat under power, dive under braking.
        let g = 9.81;
        let roll = self.body_roll.step((car.lat_accel / g * ROLL_PER_G).clamp(-0.12, 0.12), dt);
        let pitch = self.body_pitch.step((car.long_accel / g * PITCH_PER_G).clamp(-0.07, 0.05), dt);

        self.palms.clear();
        let len = track.length();
        for p in &track.palms {
            let mut z = track.wrap(p.s - s_cam);
            if z > len - 20.0 {
                z -= len;
            }
            if (-20.0..DRAW_DISTANCE).contains(&z) && self.palms.len() < MAX_PALMS {
                self.palms.push(Instance::new(p.x, 0.0, z, p.yaw, p.scale, p.lean));
            }
        }

        // Wheels: the player's roll with speed (capped visually) and steer at the front.
        self.wheels.clear();
        self.wheel_angle = (self.wheel_angle + (car.v / 0.33).min(MAX_WHEEL_SPIN) * dt) % core::f32::consts::TAU;
        push_wheels(&mut self.wheels, &PLAYER_WHEELS, car.x, CAR_Z, car.yaw, self.wheel_angle, car.steer * WHEEL_STEER);

        // Traffic, interpolated back from the current step like the player.
        for v in self.traffic.iter_mut() {
            v.clear();
        }
        for tc in &world.traffic.cars {
            let s = tc.s - tc.v * DT * (1.0 - alpha);
            let z = track.delta(s, s_cam);
            if !(-30.0..DRAW_DISTANCE).contains(&z) {
                continue;
            }
            let moving = tc.change_t > INDICATOR_TIME;
            let yaw = if moving { tc.indicator() * 0.06 } else { 0.0 } + tc.spin;
            let x = tc.x - tc.vx * DT * (1.0 - alpha);
            let k = match tc.kind {
                Archetype::Coupe => 0,
                Archetype::Van => 1,
                Archetype::Truck => 2,
                Archetype::Sports => 3,
            };
            if self.traffic[k].len() < MAX_TRAFFIC_PER_KIND {
                self.traffic[k].push(Instance::new(x, 0.0, z, yaw, 1.0, 0.0).with_lights(tc.brake, tc.indicator()));
                let vk = [VehicleKind::Coupe, VehicleKind::Van, VehicleKind::Truck, VehicleKind::Sports][k];
                let slots = wheel_layout(vk);
                let spin = time * (tc.v / slots[0].radius).min(MAX_WHEEL_SPIN) + tc.id as f32;
                if z < 400.0 {
                    push_wheels(&mut self.wheels, slots, x, z, yaw, spin, 0.0);
                }
            }
        }

        let speed01 = car.v / TUNING.v_max;
        let [t0, t1, t2, t3] = &self.traffic;
        FrameParams {
            time,
            scroll: s_cam,
            speed01,
            beat_phase,
            camera: Camera { x: self.cam_x, yaw: self.cam_yaw, height: CAM_HEIGHT, pitch: self.pitch, fov_deg: self.fov },
            fx: Fx { shake: vfx.shake(), nitro: vfx.nitro_level(), flash: vfx.flash() },
            profile: self.profile,
            car: Instance::new(car.x, 0.0, CAR_Z, car.yaw, 1.0, roll).with_pitch(pitch),
            palms: &self.palms,
            traffic: [t0, t1, t2, t3],
            wheels: &self.wheels,
            particles: vfx.particles(world, s_cam),
        }
    }
}

fn fov_for(v: f32) -> f32 {
    let u = (v / TUNING.v_max).clamp(0.0, 1.0);
    58.0 + 14.0 * u * u
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palms_are_culled_to_the_draw_distance_and_sorted_out_of_the_seam() {
        let world = World::new(42);
        let mut scene = Scene::new();
        let mut vfx = Vfx::new();
        let f = scene.build(&world, &mut vfx, 1.0, 0.0, 0.0, 1.0 / 60.0);
        assert!(!f.palms.is_empty());
        assert!(f.palms.iter().all(|p| p.pos_yaw[2] >= -20.0 && p.pos_yaw[2] < DRAW_DISTANCE));
    }

    #[test]
    fn car_sits_where_the_sim_says() {
        let mut world = World::new(3);
        world.car.x = 4.0;
        world.prev = world.car.view();
        let mut scene = Scene::new();
        let mut vfx = Vfx::new();
        let f = scene.build(&world, &mut vfx, 1.0, 0.0, 0.0, 1.0 / 60.0);
        assert_eq!(f.car.pos_yaw[0], 4.0);
        assert_eq!(f.car.pos_yaw[2], CAR_Z);
    }
}
