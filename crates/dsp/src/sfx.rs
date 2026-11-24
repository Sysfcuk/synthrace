//! Game sound effects, all synthesised: engine, tyre squeal, traffic pass-bys and one-shots.
//! Same real-time rules as the music: fixed voice pools, no allocation in `process`.

use crate::{Rng, Saw, Smooth, Svf, flush, svf_g};
use core::f32::consts::TAU;

pub const TRAFFIC_VOICES: usize = 4;

/// One-shot kinds for [`Sfx::trigger`].
pub mod shot {
    pub const IMPACT: u32 = 0;
    pub const WHOOSH: u32 = 1;
    pub const NITRO: u32 = 2;
    pub const SHIFT: u32 = 3;
    pub const BLIP: u32 = 4;
}

#[derive(Clone, Copy, Default)]
struct Traffic {
    osc: Saw,
    sub: Saw,
    filt: Svf,
    gain: f32,
    pitch: f32,
    pan: f32,
    g_s: f32,
    p_s: f32,
}

#[derive(Clone, Copy, Default)]
struct Shot {
    t: f32,
    len: f32,
    amount: f32,
    pan: f32,
    freq: f32,
    phase: f32,
    filt: Svf,
}

impl Shot {
    fn active(&self) -> bool {
        self.t < self.len
    }
}

pub struct Sfx {
    sr: f32,
    rng: Rng,
    // engine
    rpm: Smooth,
    throttle: Smooth,
    rpm_target: f32,
    throttle_target: f32,
    eng_a: Saw,
    eng_b: Saw,
    eng_sub: Saw,
    eng_filt: Svf,
    jitter: f32,
    crackle_t: f32,
    last_throttle: f32,
    // tyres
    slip: Smooth,
    slip_target: f32,
    squeal_filt: Svf,
    squeal_phase: f32,
    lfo: f32,
    // nitro hiss
    nitro: Smooth,
    nitro_target: f32,
    nitro_filt: Svf,
    traffic: [Traffic; TRAFFIC_VOICES],
    impacts: [Shot; 2],
    whooshes: [Shot; 2],
    ignite: Shot,
    shift: Shot,
    blips: [Shot; 3],
}

#[inline]
fn pan_gains(pan: f32) -> (f32, f32) {
    let p = pan.clamp(-1.0, 1.0);
    ((0.5 * (1.0 - p)).sqrt(), (0.5 * (1.0 + p)).sqrt())
}

impl Sfx {
    pub fn new(sr: f32) -> Self {
        Self {
            sr,
            rng: Rng(0x1234_5678),
            rpm: Smooth::new(1100.0, 0.03, sr),
            throttle: Smooth::new(0.0, 0.05, sr),
            rpm_target: 1100.0,
            throttle_target: 0.0,
            eng_a: Saw::default(),
            eng_b: Saw { phase: 0.37 },
            eng_sub: Saw::default(),
            eng_filt: Svf::default(),
            jitter: 0.0,
            crackle_t: 1.0,
            last_throttle: 0.0,
            slip: Smooth::new(0.0, 0.06, sr),
            slip_target: 0.0,
            squeal_filt: Svf::default(),
            squeal_phase: 0.0,
            lfo: 0.0,
            nitro: Smooth::new(0.0, 0.08, sr),
            nitro_target: 0.0,
            nitro_filt: Svf::default(),
            traffic: [Traffic::default(); TRAFFIC_VOICES],
            impacts: [Shot { t: 1.0, ..Default::default() }; 2],
            whooshes: [Shot { t: 1.0, ..Default::default() }; 2],
            ignite: Shot { t: 1.0, ..Default::default() },
            shift: Shot { t: 1.0, ..Default::default() },
            blips: [Shot { t: 1.0, ..Default::default() }; 3],
        }
    }

    pub fn set_rpm(&mut self, rpm: f32) {
        self.rpm_target = rpm.clamp(500.0, 9000.0);
    }

    pub fn set_throttle(&mut self, throttle: f32) {
        self.throttle_target = throttle.clamp(0.0, 1.0);
    }

    pub fn set_slip(&mut self, slip: f32) {
        self.slip_target = slip.clamp(0.0, 1.0);
    }

    pub fn set_nitro(&mut self, on: f32) {
        self.nitro_target = on.clamp(0.0, 1.0);
    }

    pub fn set_traffic(&mut self, i: usize, gain: f32, pitch_hz: f32, pan: f32) {
        if let Some(t) = self.traffic.get_mut(i) {
            t.gain = gain.clamp(0.0, 1.0);
            t.pitch = pitch_hz.clamp(20.0, 400.0);
            t.pan = pan.clamp(-1.0, 1.0);
        }
    }

    /// Starts a one-shot. `value` meaning depends on `kind` (strength, pan, or pitch in Hz).
    pub fn trigger(&mut self, kind: u32, value: f32) {
        let pick = |pool: &mut [Shot]| -> usize {
            let mut best = 0;
            for (i, s) in pool.iter().enumerate() {
                if !s.active() {
                    return i;
                }
                if s.t > pool[best].t {
                    best = i;
                }
            }
            best
        };
        match kind {
            shot::IMPACT => {
                let i = pick(&mut self.impacts);
                self.impacts[i] = Shot { t: 0.0, len: 0.55, amount: value.clamp(0.1, 1.0), ..Default::default() };
            }
            shot::WHOOSH => {
                let i = pick(&mut self.whooshes);
                self.whooshes[i] = Shot { t: 0.0, len: 0.4, amount: 1.0, pan: value, ..Default::default() };
            }
            shot::NITRO => self.ignite = Shot { t: 0.0, len: 0.6, amount: 1.0, ..Default::default() },
            shot::SHIFT => self.shift = Shot { t: 0.0, len: 0.09, amount: 1.0, ..Default::default() },
            shot::BLIP => {
                let i = pick(&mut self.blips);
                self.blips[i] = Shot { t: 0.0, len: 0.35, amount: 1.0, freq: value.clamp(100.0, 4000.0), ..Default::default() };
            }
            _ => {}
        }
    }

    /// Adds one stereo sample of effects into (l, r).
    #[inline]
    pub fn sample(&mut self, l: &mut f32, r: &mut f32) {
        let sr = self.sr;
        let inv = 1.0 / sr;
        let rpm = self.rpm.step(self.rpm_target);
        let thr = self.throttle.step(self.throttle_target);
        let noise = self.rng.next();

        // --- engine: 4-cylinder firing frequency, two detuned saws + sub, throttle-driven filter ---
        self.jitter += (noise * 0.004 - self.jitter) * 0.002;
        let f0 = rpm / 30.0 * (1.0 + self.jitter);
        let rpm01 = ((rpm - 1100.0) / 6500.0).clamp(0.0, 1.0);
        let e =
            self.eng_a.next(f0 * inv) * 0.6 + self.eng_b.next(f0 * 1.007 * inv) * 0.4 + self.eng_sub.next(f0 * 0.5 * inv) * 0.7;
        let cutoff = 180.0 + thr * 1400.0 + rpm01 * 1800.0;
        let e = self.eng_filt.lp(e, svf_g(cutoff, sr), 0.6);
        let e = (e * (1.5 + thr * 2.0)).tanh();
        let mut engine = e * (0.10 + 0.10 * thr + 0.05 * rpm01);
        // Off-throttle crackle at high rpm: sparse pops.
        if self.last_throttle - thr > 0.004 && rpm01 > 0.5 {
            self.crackle_t = 0.0;
        }
        self.last_throttle = thr;
        if self.crackle_t < 0.35 {
            if self.rng.next() > 0.993 {
                engine += self.rng.next() * 0.35;
            }
            self.crackle_t += inv;
        }
        // Gear shift: brief dip plus a pop.
        if self.shift.active() {
            let k = self.shift.t / self.shift.len;
            engine *= 0.35 + 0.65 * k;
            engine += noise * (1.0 - k) * 0.12;
            self.shift.t += inv;
        }
        *l += engine;
        *r += engine;

        // --- tyre squeal: resonant band-passed noise + wobbling sine ---
        let slip = self.slip.step(self.slip_target);
        if slip > 0.002 {
            self.lfo = (self.lfo + 6.5 * inv).fract();
            let wob = (self.lfo * TAU).sin();
            self.squeal_phase = (self.squeal_phase + (880.0 + 90.0 * wob) * inv).fract();
            let tone = (self.squeal_phase * TAU).sin() * 0.5;
            let band = self.squeal_filt.bp(noise, svf_g(1900.0 + 300.0 * wob, sr), 0.15);
            let sq = (tone + band * 0.9) * slip * slip * 0.12;
            *l += sq;
            *r += sq;
        }

        // --- nitro: rushing hiss ---
        let n = self.nitro.step(self.nitro_target);
        if n > 0.002 {
            let h = self.nitro_filt.bp(noise, svf_g(2600.0, sr), 0.7) * n * 0.10;
            *l += h;
            *r += h;
        }

        // --- traffic pass-bys ---
        for t in self.traffic.iter_mut() {
            t.g_s += (t.gain - t.g_s) * 0.0015;
            t.p_s += (t.pitch - t.p_s) * 0.003;
            if t.g_s < 1e-4 {
                continue;
            }
            let s = t.osc.next(t.p_s * inv) * 0.6 + t.sub.next(t.p_s * 0.5 * inv);
            let s = t.filt.lp(s, svf_g(520.0, sr), 0.8) * t.g_s * 0.16;
            let (gl, gr) = pan_gains(t.pan);
            *l += s * gl;
            *r += s * gr;
        }

        // --- one-shots ---
        for sh in self.impacts.iter_mut().filter(|s| s.active()) {
            let k = sh.t / sh.len;
            sh.phase = (sh.phase + (55.0 + 40.0 * (1.0 - k)) * inv).fract();
            let thump = (sh.phase * TAU).sin() * (1.0 - k) * (1.0 - k);
            let crunch = sh.filt.lp(noise, svf_g(3000.0 * (1.0 - k) + 300.0, sr), 0.9) * (1.0 - k).powi(3);
            let v = (thump * 0.8 + crunch * 0.9) * sh.amount * 0.5;
            *l += v;
            *r += v;
            sh.t += inv;
        }
        for sh in self.whooshes.iter_mut().filter(|s| s.active()) {
            let k = sh.t / sh.len;
            let env = (k * 6.0).min(1.0) * (1.0 - k);
            let f = 2400.0 - 1900.0 * k;
            let v = sh.filt.bp(noise, svf_g(f, sr), 0.5) * env * 0.28;
            let (gl, gr) = pan_gains(sh.pan);
            *l += v * gl;
            *r += v * gr;
            sh.t += inv;
        }
        if self.ignite.active() {
            let sh = &mut self.ignite;
            let k = sh.t / sh.len;
            sh.phase = (sh.phase + (90.0 + 260.0 * k) * inv).fract();
            let roar = sh.filt.lp(noise, svf_g(400.0 + 3000.0 * k, sr), 1.2) * (1.0 - k);
            let rise = (sh.phase * TAU).sin() * k * (1.0 - k) * 2.0;
            let v = (roar * 0.5 + rise * 0.25) * 0.6;
            *l += v;
            *r += v;
            sh.t += inv;
        }
        for sh in self.blips.iter_mut().filter(|s| s.active()) {
            let k = sh.t / sh.len;
            sh.phase = (sh.phase + sh.freq * inv).fract();
            let tri = 1.0 - 4.0 * (sh.phase - 0.5).abs();
            let v = tri * (-k * 9.0).exp() * 0.12;
            *l += v;
            *r += v;
            sh.t += inv;
        }
        *l = flush(*l);
        *r = flush(*r);
    }
}
