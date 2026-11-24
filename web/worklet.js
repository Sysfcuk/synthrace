// AudioWorkletProcessor shim: instantiates the Rust DSP module (no imports, no
// wasm-bindgen) synchronously and copies its output buffers each render quantum.
// Loaded from a Blob URL created by the game crate; keep this file tiny.
class SynthProcessor extends AudioWorkletProcessor {
  constructor(options) {
    super();
    const bytes = options.processorOptions.wasm;
    const instance = new WebAssembly.Instance(new WebAssembly.Module(bytes), {});
    this.dsp = instance.exports;
    this.dsp.init(sampleRate);
    this.blocks = 0;
    this.buf = null;
    this.port.onmessage = (e) => {
      const d = e.data;
      if (d && d.t === "p") this.dsp.set_param(d.id, d.v);
    };
  }

  process(_inputs, outputs) {
    const out = outputs[0];
    const n = out[0].length;
    this.dsp.process(n);
    const mem = this.dsp.memory.buffer;
    if (this.buf !== mem) {
      this.buf = mem;
      this.l = new Float32Array(mem, this.dsp.out_left(), 128);
      this.r = new Float32Array(mem, this.dsp.out_right(), 128);
    }
    out[0].set(n === 128 ? this.l : this.l.subarray(0, n));
    if (out.length > 1) out[1].set(n === 128 ? this.r : this.r.subarray(0, n));
    // Report the musical clock ~every 21 ms so visuals can lock to the beat.
    if ((this.blocks++ & 7) === 0) {
      this.port.postMessage({ t: "beat", phase: this.dsp.beat_phase(), at: currentTime });
    }
    return true;
  }
}
registerProcessor("synthrace-synth", SynthProcessor);
