//! SYNTHRACE real-time synth engine (M0 spike).
//!
//! Rules: no allocation, locking or panicking in `process`; all state is fixed-size.
//! The M0 spike sequences a short i–VI–III–VII loop internally to prove quality and
//! timing; from M3 the `music` conductor drives voices with timestamped events.

use core::f32::consts::TAU;

pub const BLOCK: usize = 128;

pub mod param {
    pub const MASTER: u32 = 0;
    /// Game speed 0..1: opens the music filter and enables layers.
    pub const SPEED: u32 = 1;
    pub const MUTE: u32 = 2;
}

const BPM: f32 = 118.0;
const SAWS: usize = 7;
const PAD_VOICES: usize = 3;
/// (chord tones as MIDI notes, bass root) for Am – F – C – G.
const PROGRESSION: [([u8; PAD_VOICES], u8); 4] = [([57, 60, 64], 33), ([57, 60, 65], 29), ([55, 60, 64], 36), ([55, 59, 62], 31)];
const DETUNE_CENTS: [f32; SAWS] = [-19.0, -12.0, -5.0, 0.0, 6.0, 13.0, 20.0];

fn midi_hz(n: f32) -> f32 {
    440.0 * libm_exp2((n - 69.0) / 12.0)
}

fn libm_exp2(x: f32) -> f32 {
    x.exp2()
}

/// PolyBLEP residual for band-limited discontinuities.
#[inline]
fn poly_blep(t: f32, dt: f32) -> f32 {
    if t < dt {
        let x = t / dt;
        x + x - x * x - 1.0
    } else if t > 1.0 - dt {
        let x = (t - 1.0) / dt;
        x * x + x + x + 1.0
    } else {
        0.0
    }
}

#[derive(Clone, Copy, Default)]
struct Saw {
    phase: f32,
}

impl Saw {
    #[inline]
    fn next(&mut self, dt: f32) -> f32 {
        let v = 2.0 * self.phase - 1.0 - poly_blep(self.phase, dt);
        self.phase += dt;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        v
    }
}

/// Topology-preserving-transform state-variable filter (Cytomic / Zavalishin), low-pass output.
#[derive(Clone, Copy, Default)]
struct Svf {
    ic1: f32,
    ic2: f32,
}

impl Svf {
    #[inline]
    fn lp(&mut self, x: f32, g: f32, k: f32) -> f32 {
        let a1 = 1.0 / (1.0 + g * (g + k));
        let v3 = x - self.ic2;
        let v1 = a1 * self.ic1 + g * a1 * v3;
        let v2 = self.ic2 + g * v1;
        self.ic1 = flush(2.0 * v1 - self.ic1);
        self.ic2 = flush(2.0 * v2 - self.ic2);
        v2
    }
}

#[inline]
fn flush(x: f32) -> f32 {
    if x.abs() < 1e-20 { 0.0 } else { x }
}

#[inline]
fn svf_g(cutoff: f32, sr: f32) -> f32 {
    (core::f32::consts::PI * (cutoff / sr).min(0.45)).tan()
}

struct Rng(u32);
impl Rng {
    #[inline]
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
}

/// One-pole parameter smoother.
#[derive(Clone, Copy)]
struct Smooth {
    v: f32,
    coef: f32,
}
impl Smooth {
    fn new(v: f32, time_s: f32, sr: f32) -> Self {
        Self { v, coef: 1.0 - (-1.0 / (time_s * sr)).exp() }
    }
    #[inline]
    fn step(&mut self, target: f32) -> f32 {
        self.v += (target - self.v) * self.coef;
        self.v
    }
}

pub struct Synth {
    sr: f32,
    sample: u64,
    samples_per_step: f32, // 16th note
    // params (targets)
    master: f32,
    speed: f32,
    muted: bool,
    // smoothed
    s_master: Smooth,
    s_cutoff: Smooth,
    s_drums: Smooth,
    s_arp: Smooth,
    // voices
    pad: [[Saw; SAWS]; PAD_VOICES],
    pad_filt: [Svf; 2],
    bass: Saw,
    bass_sub: Saw,
    bass_filt: Svf,
    arp: Saw,
    arp_filt: Svf,
    kick_phase: f32,
    kick_t: f32,
    hat_t: f32,
    hat_hp: f32,
    arp_t: f32,
    rng: Rng,
    duck: f32,
    pub out_l: [f32; BLOCK],
    pub out_r: [f32; BLOCK],
}

impl Synth {
    pub fn new(sample_rate: f32) -> Self {
        let sr = sample_rate.max(8000.0);
        let mut rng = Rng(0x9E37_79B9);
        let mut pad = [[Saw::default(); SAWS]; PAD_VOICES];
        for v in pad.iter_mut() {
            for s in v.iter_mut() {
                s.phase = (rng.next() + 1.0) * 0.5; // random phases avoid a flanged attack
            }
        }
        Self {
            sr,
            sample: 0,
            samples_per_step: sr * 60.0 / BPM / 4.0,
            master: 0.8,
            speed: 0.0,
            muted: false,
            s_master: Smooth::new(0.0, 0.05, sr),
            s_cutoff: Smooth::new(500.0, 0.2, sr),
            s_drums: Smooth::new(0.0, 0.4, sr),
            s_arp: Smooth::new(0.0, 0.6, sr),
            pad,
            pad_filt: [Svf::default(); 2],
            bass: Saw::default(),
            bass_sub: Saw::default(),
            bass_filt: Svf::default(),
            arp: Saw::default(),
            arp_filt: Svf::default(),
            kick_phase: 0.0,
            kick_t: 10.0,
            hat_t: 10.0,
            hat_hp: 0.0,
            arp_t: 10.0,
            rng,
            duck: 0.0,
            out_l: [0.0; BLOCK],
            out_r: [0.0; BLOCK],
        }
    }

    pub fn set_param(&mut self, id: u32, v: f32) {
        if !v.is_finite() {
            return;
        }
        match id {
            param::MASTER => self.master = v.clamp(0.0, 1.0),
            param::SPEED => self.speed = v.clamp(0.0, 1.0),
            param::MUTE => self.muted = v > 0.5,
            _ => {}
        }
    }

    /// Beat phase 0..1 (0 = on the beat) at the current position; drives visual pulses.
    pub fn beat_phase(&self) -> f32 {
        let beat = self.samples_per_step * 4.0;
        (self.sample as f32 % beat) / beat
    }

    /// Renders `n` (≤ BLOCK) stereo frames into `out_l` / `out_r`.
    pub fn process(&mut self, n: usize) {
        let n = n.min(BLOCK);
        let sr = self.sr;
        let inv_sr = 1.0 / sr;
        let step_len = self.samples_per_step;
        let bar_len = step_len * 16.0;
        // Layer targets with hysteresis-free thresholds (the conductor adds hysteresis from M3).
        let drums_target = if self.speed > 0.15 { 1.0 } else { 0.0 };
        let arp_target = if self.speed > 0.6 { 1.0 } else { 0.0 };
        let cutoff_target = 350.0 + self.speed * self.speed * 5200.0;
        let master_target = if self.muted { 0.0 } else { self.master };

        for i in 0..n {
            let pos = self.sample as f32;
            let bar = (pos / bar_len) as usize;
            let in_bar = pos - bar as f32 * bar_len;
            let step_f = in_bar / step_len;
            let step = step_f as usize;
            let in_step = in_bar - step as f32 * step_len;
            let (chord, root) = PROGRESSION[bar % PROGRESSION.len()];

            // Triggers on step boundaries.
            if in_step < 1.0 {
                if step.is_multiple_of(4) {
                    self.kick_t = 0.0;
                    self.kick_phase = 0.0;
                }
                if step % 4 == 2 {
                    self.hat_t = 0.0;
                }
                self.arp_t = 0.0;
            }

            let drums = self.s_drums.step(drums_target);
            let arp_g = self.s_arp.step(arp_target);
            let cutoff = self.s_cutoff.step(cutoff_target);
            let master = self.s_master.step(master_target);

            // Sidechain envelope follows the kick when drums are in.
            let kt = self.kick_t;
            let duck_target = if kt < 0.18 { 1.0 - kt / 0.18 } else { 0.0 };
            self.duck = duck_target * drums;
            let pump = 1.0 - 0.45 * self.duck;

            // Pad: 3 supersaw voices, odd saws left, even saws right.
            let t_bar = in_bar * inv_sr;
            let bar_s = bar_len * inv_sr;
            let pad_env = (t_bar / 0.35).min(1.0) * ((bar_s - t_bar) / 0.12).clamp(0.0, 1.0);
            let (mut pl, mut pr) = (0.0f32, 0.0f32);
            for (v, &note) in self.pad.iter_mut().zip(chord.iter()) {
                let f = midi_hz(note as f32);
                for (k, saw) in v.iter_mut().enumerate() {
                    let dt = f * libm_exp2(DETUNE_CENTS[k] / 1200.0) * inv_sr;
                    let s = saw.next(dt);
                    if k.is_multiple_of(2) { pl += s } else { pr += s }
                }
            }
            let g = svf_g(cutoff, sr);
            pl = self.pad_filt[0].lp(pl, g, 1.2) * 0.055 * pad_env * pump;
            pr = self.pad_filt[1].lp(pr, g, 1.2) * 0.055 * pad_env * pump;

            // Octave bass on 8ths.
            let octave = if (step / 2) % 2 == 1 { 12.0 } else { 0.0 };
            let bf = midi_hz(root as f32 + 12.0 + octave);
            let t_eighth = (in_bar % (step_len * 2.0)) * inv_sr;
            let benv = (-t_eighth * 9.0).exp();
            let b = self.bass.next(bf * inv_sr) * 0.7 + self.bass_sub.next(bf * 0.5 * inv_sr) * 0.5;
            let bcut = 180.0 + benv * (500.0 + cutoff * 0.25);
            let bass = self.bass_filt.lp(b, svf_g(bcut, sr), 0.9) * 0.22 * (0.35 + 0.65 * benv) * pump;

            // Kick: sine with exponential pitch drop 150 → 48 Hz.
            let kf = 48.0 + 102.0 * (-kt * 28.0).exp();
            self.kick_phase = (self.kick_phase + kf * inv_sr).fract();
            let kick = (self.kick_phase * TAU).sin() * (-kt * 7.0).exp() * 0.55 * drums;
            self.kick_t += inv_sr;

            // Off-beat hat: high-passed noise burst.
            let noise = self.rng.next();
            self.hat_hp += (noise - self.hat_hp) * 0.35;
            let hat = (noise - self.hat_hp) * (-self.hat_t * 55.0).exp() * 0.10 * drums;
            self.hat_t += inv_sr;

            // Arp: 16ths over chord tones an octave up, plucky filter.
            let arp_note = chord[step % PAD_VOICES] as f32 + 12.0 + if step % 8 >= 4 { 12.0 } else { 0.0 };
            let aenv = (-self.arp_t * 14.0).exp();
            let a = self.arp.next(midi_hz(arp_note) * inv_sr);
            let arp = self.arp_filt.lp(a, svf_g(400.0 + aenv * 4500.0, sr), 1.0) * aenv * 0.09 * arp_g;
            self.arp_t += inv_sr;

            let l = pl + bass + kick + hat * 0.8 + arp * 0.7;
            let r = pr + bass + kick + hat * 1.0 + arp * 1.0;
            // Gentle tape-style saturation on the master bus.
            self.out_l[i] = (l * 1.2).tanh() * 0.8 * master;
            self.out_r[i] = (r * 1.2).tanh() * 0.8 * master;
            self.sample += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(sr: f32, seconds: f32, speed: f32) -> (Vec<f32>, Vec<f32>) {
        let mut s = Synth::new(sr);
        s.set_param(param::SPEED, speed);
        let (mut l, mut r) = (Vec::new(), Vec::new());
        let blocks = (seconds * sr / BLOCK as f32) as usize;
        for _ in 0..blocks {
            s.process(BLOCK);
            l.extend_from_slice(&s.out_l);
            r.extend_from_slice(&s.out_r);
        }
        (l, r)
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
    }

    #[test]
    fn output_is_finite_bounded_and_audible_at_all_speeds() {
        for sr in [44_100.0, 48_000.0] {
            for speed in [0.0, 0.5, 1.0] {
                let (l, r) = render(sr, 8.0, speed);
                for x in l.iter().chain(r.iter()) {
                    assert!(x.is_finite());
                    assert!(x.abs() <= 1.0, "clipping above full scale");
                    assert!(*x == 0.0 || x.abs() > 1e-30, "denormal in output");
                }
                let level = rms(&l[(sr as usize)..]);
                assert!(level > 0.01 && level < 0.5, "sr={sr} speed={speed} rms={level}");
            }
        }
    }

    #[test]
    fn speed_opens_filter_and_adds_layers() {
        let (slow, _) = render(48_000.0, 6.0, 0.0);
        let (fast, _) = render(48_000.0, 6.0, 1.0);
        // High-frequency energy proxy: RMS of the first difference.
        let hf = |x: &[f32]| rms(&x.windows(2).map(|w| w[1] - w[0]).collect::<Vec<_>>());
        assert!(hf(&fast[48_000..]) > 2.0 * hf(&slow[48_000..]));
    }

    #[test]
    fn mute_silences_output() {
        let mut s = Synth::new(48_000.0);
        s.set_param(param::MUTE, 1.0);
        for _ in 0..2000 {
            s.process(BLOCK);
        }
        assert!(s.out_l.iter().all(|v| v.abs() < 1e-4));
    }

    #[test]
    fn non_finite_params_are_ignored() {
        let mut s = Synth::new(48_000.0);
        s.set_param(param::SPEED, f32::NAN);
        s.process(BLOCK);
        assert!(s.out_l.iter().all(|v| v.is_finite()));
    }
}
