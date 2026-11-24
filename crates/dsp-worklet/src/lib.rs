//! AudioWorklet ABI for the `dsp` synth. Instantiated synchronously with no imports
//! inside AudioWorkletGlobalScope (see web/worklet.js). Single-threaded by construction.

use core::cell::UnsafeCell;
use dsp::Synth;

struct Global(UnsafeCell<Option<Synth>>);
// SAFETY: the worklet instance is only ever called from the single audio render thread.
unsafe impl Sync for Global {}
static SYNTH: Global = Global(UnsafeCell::new(None));

#[allow(clippy::mut_from_ref)]
fn synth() -> Option<&'static mut Synth> {
    // SAFETY: see `Global`; no re-entrancy from JS within one call.
    unsafe { (*SYNTH.0.get()).as_mut() }
}

#[unsafe(no_mangle)]
pub extern "C" fn init(sample_rate: f32) {
    // SAFETY: see `Global`.
    unsafe { *SYNTH.0.get() = Some(Synth::new(sample_rate)) };
}

#[unsafe(no_mangle)]
pub extern "C" fn process(frames: u32) {
    if let Some(s) = synth() {
        s.process(frames as usize);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn out_left() -> *const f32 {
    synth().map_or(core::ptr::null(), |s| s.out_l.as_ptr())
}

#[unsafe(no_mangle)]
pub extern "C" fn out_right() -> *const f32 {
    synth().map_or(core::ptr::null(), |s| s.out_r.as_ptr())
}

#[unsafe(no_mangle)]
pub extern "C" fn set_param(id: u32, value: f32) {
    if let Some(s) = synth() {
        s.set_param(id, value);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn beat_phase() -> f32 {
    synth().map_or(0.0, |s| s.beat_phase())
}
