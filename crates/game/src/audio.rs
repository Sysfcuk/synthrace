//! Audio bridge: creates the AudioContext on a user gesture, loads the worklet shim
//! from a Blob URL, hands it the embedded DSP wasm bytes, and relays parameters.

use std::cell::Cell;
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use web_sys::{AudioContext, AudioWorkletNode, AudioWorkletNodeOptions, Blob, BlobPropertyBag, MessageEvent, Url};

const WORKLET_JS: &str = include_str!("../../../web/worklet.js");
const DSP_WASM: &[u8] =
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/wasm32-unknown-unknown/release/dsp_worklet.wasm"));

const BPM: f64 = 118.0;

pub struct Audio {
    ctx: AudioContext,
    node: AudioWorkletNode,
    /// (beat phase reported by the worklet, AudioContext time it was reported at)
    beat: Rc<Cell<(f32, f64)>>,
    _on_msg: Closure<dyn FnMut(MessageEvent)>,
}

impl Audio {
    /// Must be called from inside a user-gesture handler (autoplay policy).
    pub fn create_context() -> Result<AudioContext, JsValue> {
        let ctx = AudioContext::new()?;
        // iOS: route Web Audio as "playback" so the silent switch doesn't mute the game (iOS 17+).
        let nav = web_sys::window().unwrap().navigator();
        if let Ok(session) = js_sys::Reflect::get(&nav, &"audioSession".into())
            && !session.is_undefined()
        {
            let _ = js_sys::Reflect::set(&session, &"type".into(), &"playback".into());
        }
        let _ = ctx.resume();
        Ok(ctx)
    }

    pub async fn start(ctx: AudioContext) -> Result<Audio, JsValue> {
        let parts = js_sys::Array::of1(&JsValue::from_str(WORKLET_JS));
        let bag = BlobPropertyBag::new();
        bag.set_type("text/javascript");
        let blob = Blob::new_with_str_sequence_and_options(&parts, &bag)?;
        let url = Url::create_object_url_with_blob(&blob)?;
        JsFuture::from(ctx.audio_worklet()?.add_module(&url)?).await?;
        let _ = Url::revoke_object_url(&url);

        let wasm = js_sys::Uint8Array::from(DSP_WASM).buffer();
        let proc_opts = js_sys::Object::new();
        js_sys::Reflect::set(&proc_opts, &"wasm".into(), &wasm)?;
        let opts = AudioWorkletNodeOptions::new();
        opts.set_number_of_inputs(0);
        opts.set_output_channel_count(&js_sys::Array::of1(&2.into()));
        opts.set_processor_options(Some(&proc_opts));
        let node = AudioWorkletNode::new_with_options(&ctx, "synthrace-synth", &opts)?;
        node.connect_with_audio_node(&ctx.destination())?;

        let beat = Rc::new(Cell::new((0.0f32, 0.0f64)));
        let beat_in = beat.clone();
        let on_msg = Closure::<dyn FnMut(MessageEvent)>::new(move |e: MessageEvent| {
            let d = e.data();
            let phase = js_sys::Reflect::get(&d, &"phase".into()).ok().and_then(|v| v.as_f64());
            let at = js_sys::Reflect::get(&d, &"at".into()).ok().and_then(|v| v.as_f64());
            if let (Some(p), Some(a)) = (phase, at) {
                beat_in.set((p as f32, a));
            }
        });
        node.port()?.set_onmessage(Some(on_msg.as_ref().unchecked_ref()));
        Ok(Audio { ctx, node, beat, _on_msg: on_msg })
    }

    pub fn set_param(&self, id: u32, value: f32) {
        let msg = js_sys::Object::new();
        let _ = js_sys::Reflect::set(&msg, &"t".into(), &"p".into());
        let _ = js_sys::Reflect::set(&msg, &"id".into(), &id.into());
        let _ = js_sys::Reflect::set(&msg, &"v".into(), &value.into());
        if let Ok(port) = self.node.port() {
            let _ = port.post_message(&msg);
        }
    }

    fn post(&self, msg: &js_sys::Object) {
        if let Ok(port) = self.node.port() {
            let _ = port.post_message(msg);
        }
    }

    /// Sets consecutive parameters `id..id + values.len()` in one message.
    pub fn set_params(&self, id: u32, values: &[f32]) {
        let msg = js_sys::Object::new();
        let _ = js_sys::Reflect::set(&msg, &"t".into(), &"pp".into());
        let _ = js_sys::Reflect::set(&msg, &"id".into(), &id.into());
        let _ = js_sys::Reflect::set(&msg, &"v".into(), &js_sys::Float32Array::from(values));
        self.post(&msg);
    }

    /// Fires a one-shot effect (see `dsp::sfx::shot`).
    pub fn trigger(&self, kind: u32, value: f32) {
        let msg = js_sys::Object::new();
        let _ = js_sys::Reflect::set(&msg, &"t".into(), &"e".into());
        let _ = js_sys::Reflect::set(&msg, &"k".into(), &kind.into());
        let _ = js_sys::Reflect::set(&msg, &"v".into(), &value.into());
        self.post(&msg);
    }

    /// Beat phase extrapolated from the worklet's last report to "now" on the audio clock.
    pub fn beat_phase(&self) -> f32 {
        let (phase, at) = self.beat.get();
        let dt = (self.ctx.current_time() - at).max(0.0);
        ((phase as f64 + dt * BPM / 60.0).fract()) as f32
    }

    pub fn state(&self) -> &'static str {
        match self.ctx.state() {
            web_sys::AudioContextState::Running => "running",
            web_sys::AudioContextState::Suspended => "suspended",
            web_sys::AudioContextState::Closed => "closed",
            _ => "interrupted",
        }
    }

    pub fn suspend(&self) {
        let _ = self.ctx.suspend();
    }

    pub fn resume(&self) {
        let _ = self.ctx.resume();
    }

    pub fn sample_rate(&self) -> f32 {
        self.ctx.sample_rate()
    }
}
