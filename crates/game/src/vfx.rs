//! Visual feedback driven by the simulation: CPU particles (sparks, nitro flames, dust, smoke,
//! speed streaks), camera shake ("trauma"), screen flash, hit-stop and FOV kick.
//! Purely visual: never feeds back into `sim`.

use render::Particle;
use sim::{Pcg32, SimEvent, TUNING, World};

pub const MAX_PARTICLES: usize = render::MAX_PARTICLES;

#[derive(Clone, Copy)]
struct P {
    s: f32,
    x: f32,
    y: f32,
    vs: f32,
    vx: f32,
    vy: f32,
    life: f32,
    max_life: f32,
    size: f32,
    grow: f32,
    color: [f32; 3],
    streak: f32,
    gravity: f32,
}

pub struct Vfx {
    parts: Vec<P>,
    out: Vec<Particle>,
    rng: Pcg32,
    /// Camera shake energy 0..1 (shake amplitude ∝ trauma²).
    trauma: f32,
    flash: [f32; 4],
    /// Seconds of simulation freeze remaining (hit-stop).
    pub hitstop: f32,
    fov_kick: f32,
    nitro_level: f32,
    time: f32,
}

impl Default for Vfx {
    fn default() -> Self {
        Self::new()
    }
}

const TIER_COLORS: [[f32; 3]; 4] = [[1.0, 0.45, 0.12], [0.15, 0.85, 1.0], [1.0, 0.15, 0.75], [0.85, 0.75, 1.0]];

impl Vfx {
    pub fn new() -> Self {
        Self {
            parts: Vec::with_capacity(MAX_PARTICLES),
            out: Vec::with_capacity(MAX_PARTICLES),
            rng: Pcg32::new(0x5eed_cafe, 7),
            trauma: 0.0,
            flash: [0.0; 4],
            hitstop: 0.0,
            fov_kick: 0.0,
            nitro_level: 0.0,
            time: 0.0,
        }
    }

    fn spawn(&mut self, p: P) {
        if self.parts.len() < MAX_PARTICLES {
            self.parts.push(p);
        }
    }

    fn burst(&mut self, world: &World, n: usize, side: f32, color: [f32; 3], speed: f32) {
        let c = &world.car;
        for _ in 0..n {
            let r = &mut self.rng;
            let p = P {
                s: c.s + r.range(0.5, 2.2),
                x: c.x + side * r.range(0.6, 1.0),
                y: r.range(0.2, 0.9),
                vs: c.v * r.range(0.3, 0.9),
                vx: side * r.range(1.0, speed) + r.range(-2.0, 2.0),
                vy: r.range(1.0, speed * 0.7),
                life: 0.0,
                max_life: r.range(0.3, 0.8),
                size: r.range(0.025, 0.05),
                grow: 0.0,
                color: [color[0] * 10.0, color[1] * 10.0, color[2] * 10.0],
                streak: r.range(0.15, 0.4),
                gravity: -9.8,
            };
            self.spawn(p);
        }
    }

    pub fn on_event(&mut self, e: &SimEvent, world: &World) {
        match *e {
            SimEvent::Bump { strength, side } => {
                self.trauma = (self.trauma + 0.35 * strength).min(1.0);
                self.burst(world, 40, side, [1.0, 0.6, 0.25], 6.0);
            }
            SimEvent::SpinOut => {
                self.trauma = (self.trauma + 0.8).min(1.0);
                self.hitstop = self.hitstop.max(0.06);
                self.flash = [1.0, 0.25, 0.45, 0.8];
                self.burst(world, 120, 0.0, [1.0, 0.55, 0.2], 9.0);
            }
            SimEvent::Crash => {
                self.trauma = 1.0;
                self.hitstop = self.hitstop.max(0.08);
                self.flash = [1.0, 0.2, 0.4, 1.5];
                self.burst(world, 160, 0.0, [1.0, 0.5, 0.2], 10.0);
            }
            SimEvent::Scrape { side } => {
                self.trauma = (self.trauma + 0.25).min(1.0);
                self.burst(world, 50, side, [1.0, 0.7, 0.3], 5.0);
            }
            SimEvent::NearMiss { close, side, .. } => {
                self.trauma = (self.trauma + if close { 0.12 } else { 0.05 }).min(1.0);
                if close {
                    self.hitstop = self.hitstop.max(0.035);
                }
                // A short cyan streak flurry on the passing side.
                let c = &world.car;
                for _ in 0..if close { 18 } else { 8 } {
                    let r = &mut self.rng;
                    let p = P {
                        s: c.s + r.range(-3.0, 6.0),
                        x: c.x + side * r.range(1.2, 2.2),
                        y: r.range(0.3, 1.6),
                        vs: c.v * 0.2,
                        vx: side * 2.0,
                        vy: 0.0,
                        life: 0.0,
                        max_life: r.range(0.2, 0.35),
                        size: 0.02,
                        grow: 0.0,
                        color: [0.3, 1.6, 2.4],
                        streak: r.range(2.0, 5.0),
                        gravity: 0.0,
                    };
                    self.spawn(p);
                }
            }
            SimEvent::DriftBoost { tier } => {
                self.trauma = (self.trauma + 0.1).min(1.0);
                let col = TIER_COLORS[tier as usize];
                self.flash = [col[0], col[1], col[2], 0.15 * tier as f32];
                self.fov_kick = self.fov_kick.max(3.0 + 2.0 * tier as f32);
            }
            SimEvent::NitroStart => {
                self.flash = [0.3, 0.5, 1.0, 0.35];
            }
            _ => {}
        }
    }

    /// Continuous emitters + integration. `dt` is real frame time (s).
    pub fn update(&mut self, world: &World, dt: f32) {
        self.time += dt;
        let c = &world.car;
        let speed01 = c.v / TUNING.v_max;
        let nitro_target = if c.nitro_active { 1.0 } else { 0.0 };
        self.nitro_level += (nitro_target - self.nitro_level) * (dt * 6.0).min(1.0);

        // Expected spawn count this frame for a rate (per second), with stochastic rounding.
        let count = |rng: &mut Pcg32, rate: f32| -> usize {
            let n = rate * dt;
            n as usize + if rng.f32() < n.fract() { 1 } else { 0 }
        };

        // Drift sparks + smoke from the rear wheels.
        if c.drift.active {
            let col = TIER_COLORS[c.drift.tier as usize];
            for side in [-1.0f32, 1.0] {
                for _ in 0..count(&mut self.rng, 140.0) {
                    let r = &mut self.rng;
                    let p = P {
                        s: c.s - 1.35,
                        x: c.x + side * 0.85,
                        y: 0.08,
                        vs: c.v * r.range(0.7, 0.95),
                        vx: -c.drift.dir * r.range(1.0, 5.0) + r.range(-1.0, 1.0),
                        vy: r.range(0.5, 3.5),
                        life: 0.0,
                        max_life: r.range(0.2, 0.45),
                        size: 0.03,
                        grow: 0.0,
                        color: [col[0] * 9.0, col[1] * 9.0, col[2] * 9.0],
                        streak: r.range(0.2, 0.45),
                        gravity: -9.8,
                    };
                    self.spawn(p);
                }
                for _ in 0..count(&mut self.rng, 30.0) {
                    let r = &mut self.rng;
                    let p = P {
                        s: c.s - 1.4,
                        x: c.x + side * 0.85,
                        y: 0.3,
                        vs: c.v * r.range(0.75, 0.9),
                        vx: r.range(-1.0, 1.0),
                        vy: r.range(0.3, 1.0),
                        life: 0.0,
                        max_life: r.range(0.5, 0.9),
                        size: 0.35,
                        grow: 2.2,
                        color: [0.10, 0.06, 0.14],
                        streak: 0.0,
                        gravity: 0.0,
                    };
                    self.spawn(p);
                }
            }
        }

        // Nitro flames from twin exhausts.
        if c.nitro_active {
            for side in [-1.0f32, 1.0] {
                for _ in 0..count(&mut self.rng, 160.0) {
                    let r = &mut self.rng;
                    let hot = r.f32();
                    let p = P {
                        s: c.s - 2.3,
                        x: c.x + side * 0.42 + r.range(-0.05, 0.05),
                        y: 0.32 + r.range(-0.04, 0.04),
                        vs: c.v - r.range(14.0, 26.0),
                        vx: r.range(-0.6, 0.6),
                        vy: r.range(-0.2, 0.4),
                        life: 0.0,
                        max_life: r.range(0.08, 0.2),
                        size: r.range(0.12, 0.22),
                        grow: -0.6,
                        color: [0.4 + 0.6 * hot, 0.35 * (1.0 - hot) + 0.1, 1.0],
                        streak: 0.0,
                        gravity: 0.0,
                    };
                    let mut p = p;
                    p.color = [p.color[0] * 7.0, p.color[1] * 7.0, p.color[2] * 7.0];
                    self.spawn(p);
                }
            }
        }

        // Off-road dust.
        if c.offroad && c.v > 8.0 {
            for side in [-1.0f32, 1.0] {
                for _ in 0..count(&mut self.rng, 40.0 * speed01.max(0.3)) {
                    let r = &mut self.rng;
                    let p = P {
                        s: c.s - 1.4,
                        x: c.x + side * 0.9,
                        y: 0.2,
                        vs: c.v * r.range(0.4, 0.8),
                        vx: side * r.range(0.5, 2.5),
                        vy: r.range(0.5, 2.0),
                        life: 0.0,
                        max_life: r.range(0.5, 1.0),
                        size: 0.4,
                        grow: 1.6,
                        color: [0.22, 0.05, 0.16],
                        streak: 0.0,
                        gravity: 0.0,
                    };
                    self.spawn(p);
                }
            }
        }

        // Speed streaks: world-anchored lines the car rushes past.
        let streak_rate = ((speed01 - 0.55).max(0.0) * 220.0) + self.nitro_level * 260.0;
        for _ in 0..count(&mut self.rng, streak_rate) {
            let r = &mut self.rng;
            let side = r.sign();
            let p = P {
                s: c.s + r.range(25.0, 70.0),
                x: c.x + side * r.range(2.5, 9.0),
                y: r.range(0.3, 4.5),
                vs: 0.0,
                vx: 0.0,
                vy: 0.0,
                life: 0.0,
                max_life: r.range(0.5, 0.9),
                size: 0.012,
                grow: 0.0,
                color: [0.6 + self.nitro_level, 0.9, 1.4 + self.nitro_level],
                streak: r.range(6.0, 16.0) * (0.6 + speed01),
                gravity: 0.0,
            };
            self.spawn(p);
        }

        // Integrate.
        let track = &world.track;
        for p in self.parts.iter_mut() {
            p.life += dt;
            p.s = track.wrap(p.s + p.vs * dt);
            p.x += p.vx * dt;
            p.vy += p.gravity * dt;
            p.y += p.vy * dt;
            if p.y < 0.02 && p.vy < 0.0 {
                p.y = 0.02;
                p.vy *= -0.3;
                p.vx *= 0.6;
            }
            p.size = (p.size + p.grow * dt).max(0.005);
        }
        self.parts.retain(|p| p.life < p.max_life);

        // Camera feedback decay.
        self.trauma = (self.trauma - dt * 1.6).max(0.0);
        self.flash[3] = (self.flash[3] - dt * 4.0).max(0.0);
        self.fov_kick = (self.fov_kick - dt * 6.0).max(0.0);
        if self.hitstop > 0.0 {
            self.hitstop -= dt;
        }
    }

    /// Particle instances relative to the camera at track distance `s_cam`.
    pub fn particles(&mut self, world: &World, s_cam: f32) -> &[Particle] {
        self.out.clear();
        for p in &self.parts {
            let z = world.track.delta(p.s, s_cam);
            if !(-5.0..700.0).contains(&z) {
                continue;
            }
            let k = 1.0 - p.life / p.max_life;
            let fade = k * k;
            self.out.push(Particle {
                pos_size: [p.x, p.y, z, p.size],
                color: [p.color[0] * fade, p.color[1] * fade, p.color[2] * fade, p.streak],
            });
        }
        &self.out
    }

    /// Camera shake offset (m): trauma² scaled, smooth pseudo-noise.
    pub fn shake(&self) -> [f32; 2] {
        let a = self.trauma * self.trauma * 0.35;
        let t = self.time * 31.0;
        [a * ((t * 1.3).sin() + 0.5 * (t * 2.9).sin()), a * 0.6 * ((t * 1.7 + 1.3).sin() + 0.5 * (t * 3.7).sin())]
    }

    pub fn flash(&self) -> [f32; 4] {
        self.flash
    }

    pub fn nitro_level(&self) -> f32 {
        self.nitro_level
    }

    /// Extra field of view (degrees) from nitro and drift boosts.
    pub fn fov_kick(&self) -> f32 {
        self.fov_kick + 12.0 * self.nitro_level
    }

    /// Stress-test helper: spawn `n` spark particles around the car without camera effects.
    pub fn debug_fill(&mut self, world: &World, n: usize) {
        self.burst(world, n, 0.0, [1.0, 0.55, 0.2], 9.0);
    }

    pub fn live_particles(&self) -> usize {
        self.parts.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn particles_are_capped_and_expire() {
        let mut w = World::new(1);
        w.car.v = TUNING.v_max;
        w.car.nitro_active = true;
        w.car.offroad = true;
        let mut v = Vfx::new();
        for _ in 0..600 {
            v.update(&w, 1.0 / 60.0);
            for _ in 0..3 {
                v.on_event(&SimEvent::Crash, &w);
            }
        }
        assert!(v.live_particles() <= MAX_PARTICLES);
        w.car.nitro_active = false;
        w.car.offroad = false;
        w.car.v = 0.0;
        for _ in 0..120 {
            v.update(&w, 1.0 / 60.0);
        }
        assert_eq!(v.live_particles(), 0);
    }

    #[test]
    fn crash_shakes_flashes_and_hitstops() {
        let w = World::new(1);
        let mut v = Vfx::new();
        v.on_event(&SimEvent::Crash, &w);
        assert!(v.hitstop > 0.0 && v.flash()[3] > 0.0);
        v.update(&w, 0.016);
        let s = v.shake();
        assert!(s[0] != 0.0 || s[1] != 0.0);
    }
}
