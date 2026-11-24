//! Builds render frames from simulation snapshots: chase camera, car and scenery instances.
//! Platform-independent so the native `desktop` runner renders exactly what the browser does.

use crate::vfx::Vfx;
use render::{Camera, FrameParams, Fx, Instance, MAX_PALMS, MAX_TRAFFIC_PER_KIND, PROFILE_SAMPLES, PROFILE_STEP};
use sim::{Archetype, CarView, DT, TUNING, World, traffic::INDICATOR_TIME};

/// Distance from the camera to the car's centre along the track (m).
pub const CAR_Z: f32 = 7.2;
const CAM_HEIGHT: f32 = 2.35;
const BASE_PITCH: f32 = -0.075;
const DRAW_DISTANCE: f32 = 1250.0;
/// Visual exaggeration of the road's swing and hills (OutRun-style drama). Purely visual:
/// the bend is applied uniformly to everything drawn, and gameplay stays in track space.
const VISUAL_CURVE_GAIN: f32 = 1.8;
const VISUAL_HILL_GAIN: f32 = 1.3;

pub struct Scene {
    cam_x: f32,
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
        self.cam_x = car.x * 0.8;
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
        let k = (dt * 6.0).min(1.0);
        self.cam_x += (car.x * 0.8 - self.cam_x) * k;
        let slope = (self.profile[2][1] - self.profile[0][1]) / (2.0 * PROFILE_STEP);
        let pitch_target = BASE_PITCH + slope.atan() * 0.55;
        self.pitch += (pitch_target - self.pitch) * (dt * 3.0).min(1.0);
        self.fov += (fov_for(car.v) + vfx.fov_kick() - self.fov) * (dt * 3.0).min(1.0);

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
            let yaw = if moving { tc.indicator() * 0.06 } else { 0.0 };
            let k = match tc.kind {
                Archetype::Coupe => 0,
                Archetype::Van => 1,
                Archetype::Truck => 2,
                Archetype::Sports => 3,
            };
            if self.traffic[k].len() < MAX_TRAFFIC_PER_KIND {
                self.traffic[k].push(Instance::new(tc.x, 0.0, z, yaw, 1.0, 0.0).with_lights(tc.brake, tc.indicator()));
            }
        }

        let speed01 = car.v / TUNING.v_max;
        let [t0, t1, t2, t3] = &self.traffic;
        FrameParams {
            time,
            scroll: s_cam,
            speed01,
            beat_phase,
            camera: Camera { x: self.cam_x, height: CAM_HEIGHT, pitch: self.pitch, fov_deg: self.fov },
            fx: Fx { shake: vfx.shake(), nitro: vfx.nitro_level(), flash: vfx.flash() },
            profile: self.profile,
            car: car_instance(&car, speed01),
            palms: &self.palms,
            traffic: [t0, t1, t2, t3],
            particles: vfx.particles(world, s_cam),
        }
    }
}

fn fov_for(v: f32) -> f32 {
    let u = (v / TUNING.v_max).clamp(0.0, 1.0);
    58.0 + 14.0 * u * u
}

fn car_instance(car: &CarView, speed01: f32) -> Instance {
    // Body roll leans out of the turn a little; yaw follows lateral travel.
    let roll = -car.steer * 0.035 * speed01;
    Instance::new(car.x, 0.0, CAR_Z, car.yaw, 1.0, roll)
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
