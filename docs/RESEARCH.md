# SYNTHRACE — Phase 0 Research

Research date: 2026-11-24. Version numbers and browser-support figures were checked against crates.io, npm, the gpuweb implementation-status wiki, caniuse, web3dsurvey and MDN on that date.

- Statements marked **(est.)** are engineering estimates that M0 has to measure.
- Statements marked **(unverified)** could not be confirmed from a primary source.

Contents:
1. Platform facts that shape every decision
2. Rust / browser architecture options
3. Rendering techniques
4. Audio and procedural music
5. Arcade racing design research
6. Deployment research
7. Risks and opportunities
8. Sources

---

## 1. Platform facts that shape every decision

| Fact | Evidence | Consequence |
|---|---|---|
| WebGPU reaches about 83–89% of users. The gaps are mostly Android (about 70% of devices get a working adapter), Linux, Firefox outside Windows/macOS, and Samsung Xclipse GPUs. | caniuse puts global support at 88.6%. web3dsurvey shows 83.2% of devices get a working adapter. The gpuweb wiki was edited 2026-10-02. | **A WebGL2 fallback is mandatory.** WebGL2 must be the design baseline, with WebGPU as an enhancement. |
| WebGL2 allows **no** compute shaders, no storage buffers and no storage textures, and limits uniform buffers to 16 KiB. | `wgpu::Limits::downlevel_webgl2_defaults()` | Particles, culling and similar work go on the CPU or in vertex shaders. No compute is allowed on the baseline path. |
| `EXT_color_buffer_float` is available on 99.95% of WebGL2 devices (99.88% Android, 99.99% iOS). `EXT_color_buffer_half_float` is only 90.7%. | web3dsurvey | An RGBA16F HDR scene target is safe on WebGL2 through `EXT_color_buffer_float`. Avoid 32F blending, which iOS lacks via `EXT_float_blend`. |
| Multisampled float render targets on WebGL2 depend on the driver. | MDN's base list for `renderbufferStorageMultisample` has no float formats. | Don't depend on MSAA in the HDR target. Use FXAA/SMAA-lite in post, plus analytic `fwidth` anti-aliasing of the line art. MSAA is an optional enhancement on WebGPU. |
| GitHub Pages serves `.wasm` as `application/wasm`. It applies **gzip only** (no Brotli, even when the browser asks for it), uses `Cache-Control: max-age=600`, and allows **no custom headers**. | Measured with curl against a real Pages-hosted wasm. | Streaming compile works. Every size budget is a gzip budget. Filenames must be hashed. There is no COOP/COEP, so **no SharedArrayBuffer and no wasm threads**. |
| AudioWorklet has been Baseline in every major browser since 2023, including Safari 14.1 and iOS 14.5. | web-features explorer, caniuse | A custom DSP in Rust inside an AudioWorklet is viable everywhere. |
| iOS mutes Web Audio when the silent switch is on, suspends the AudioContext in the background, and requires a user gesture to unlock audio. | WebKit bugs 252746, 251532 and 261554 | Set `navigator.audioSession.type = 'playback'` (iOS 17+). Resume on `visibilitychange`, and show a "tap to resume" overlay. |
| Current versions: wgpu 30.0.2, Bevy 0.20.1, macroquad 0.4.16, glow 0.18, wasm-bindgen 0.2.130, trunk 0.22.2, three r187, Rust 1.100 (local). | crates.io, npm | The planning baseline. Pin exact versions in `Cargo.lock` and CI. |

---

## 2. Rust / browser architecture options

Five approaches were compared:

- **A: raw wgpu.** wgpu 30 with WGSL shaders, automatic WebGPU→WebGL2 fallback in a single binary via `wgpu::util::new_instance_with_webgpu_detection`. We write our own small renderer.
- **B: Bevy 0.20.** Full ECS engine with built-in bloom and tonemapping. A binary has **one** backend: the `webgpu` feature overrides `webgl2`, so supporting both needs two builds and a JS loader that picks one.
- **C: Rust simulation + Three.js r187.** `WebGPURenderer` falls back to WebGL2 automatically and has a mature TSL post-processing stack. Rendering would live in JS.
- **D: raw WebGL2** via glow or web-sys, with hand-written GLSL ES 3.0.
- **E: Macroquad / miniquad.** WebGL **1** only on the web, through its own `gl.js` loader.

| | A wgpu | B Bevy | C Three.js | D raw WebGL2 | E Macroquad |
|---|---|---|---|---|---|
| Browser reach | ~100%; WebGPU where available, in one binary | ~100% on WebGL2; WebGPU needs a 2nd binary | ~100% | ~100% | ~100% (WebGL1) |
| Shader / post flexibility | Full WGSL (Naga translates it to GLSL ES) | High, with bloom and tonemapping built in | High (TSL) | Full GLSL | Low (RGBA16F only, WebGL1) |
| WASM gzip (est.) | 0.8–2 MB | 3–6 MB | <0.3 MB plus ~0.2 MB JS | 0.2–0.5 MB | 0.3–0.6 MB |
| Compile and iteration | Moderate | Slow | Fast | Fast | Very fast |
| Core in Rust? | Yes | Yes | Simulation only; **the renderer is JS** | Yes | Yes |
| Long-term risk | wgpu releases a major version about every 4 months (moderate churn); Naga edge cases | Breaking API churn every ~4 months; download size | Split codebase; violates "engine in Rust" | No WebGPU future | Bus factor; WebGL1 ceiling |

**Findings:**

- **E is ruled out.** WebGL1, no robust HDR, and it doesn't interoperate with wasm-bindgen.
- **C is ruled out** as the primary option, because the brief requires the engine and rendering to be Rust. It stays the documented fallback if the Rust renderer fails its M0 spike.
- **B is attractive**, since bloom and tonemapping come built in, but it is heavy for a downloadable arcade game:
  - its ECS, asset server and render graph solve problems we don't have (a single scene, procedural assets, no editor);
  - its upgrade churn is a tax.
- **D gives the smallest binary**, but it permanently gives up WebGPU, and the extra WebGL boilerplate is roughly what wgpu already handles for us.
- **A is the sweet spot.** All-Rust and WGSL-everywhere. WebGPU where available with automatic fallback. We write about 2–3k lines of focused rendering code, and we own every pass, which a distinctive look needs anyway.

**Data point for download size:** egui's demo on Pages is 12.0 MB raw and 4.5 MB gzip, so a full immediate-mode GUI plus a renderer gets heavy. We will **not** ship egui in release builds. The HUD will be drawn by our own renderer. M0 measures the size of a minimal wgpu `webgpu+webgl` binary.

### Tooling

- **trunk 0.22.2** for builds. It runs wasm-bindgen and wasm-opt, hashes filenames, and takes `--public-url /<repo>/`. We don't need wasm-pack, which is aimed at npm packaging.
- **Release profile:** `opt-level` of `"s"` or `3` (benchmark both; `"z"` can cost frame time), `lto="fat"`, `codegen-units=1`, `panic="abort"`, `strip=true`, and wasm-opt `-Oz --enable-bulk-memory`.
- **getrandom 0.4:** needs the `wasm_js` feature. Audit the tree with `cargo tree -i getrandom`.
- **Gamepads:** use web-sys `navigator.getGamepads()` with `mapping=="standard"`, which also exposes rumble through `vibrationActuator`. gilrs works on wasm but adds weight we don't need on a web-only target.
- **Touch:** Pointer Events, with `touch-action:none` on the canvas.

---

## 3. Rendering techniques

Each technique below is rated for whether it suits this project: **Yes**, **Maybe**, or **No**.

### 3.1 Road representation

| Technique | How it works | Benefit | Cost | Complexity | Verdict |
|---|---|---|---|---|---|
| **Classic pseudo-3D segments** (Lou's Pseudo-3D page; Code inComplete's javascript-racer) | Segments with `curve` and `hill` values are projected near to far. Curvature accumulates as `dx += curve`. Sprites scale with 1/z, and hills are clipped with `maxy`. | An authentic swinging road. Curves are not tied to real geometry. Gameplay is effectively 1D. | Negligible | Low | **Yes as the authoring and gameplay model; No as the renderer.** There is no real depth or normals, sprites sort and pop badly, and the visual ceiling is low. |
| **Full 3D spline road** | Integrate heading and pitch into a centreline frame, extrude a ribbon mesh, and stream it in chunks. | Real depth, lighting and reflections | Low | Medium | **No (as the primary).** Real 90° bends yaw the camera and lose the iconic sun-at-the-vanishing-point view. |
| **Hybrid "bent world"** (Animal Crossing / Subway Surfers style world bending; Blightbound postmortem) | Geometry is authored in straightened *track space*. In the vertex shader, `x += C(z_rel)` and `y += H(z_rel)`, where C and H are the double-integrated curve and hill profiles, uploaded each frame as about 256 samples. Plus a global `y -= k·z_rel²` to curve the horizon. | All geometry bends consistently. The camera always looks forward, so the sun stays pinned. You get real 3D depth, lighting, fog and reflections with the pseudo-3D feel. | 1 texture fetch per vertex | Medium | **Yes (recommended).** Caveats: tessellate along Z every 2–5 m, pad culling bounds for the bend, and accept approximate normals (fine for emissive and neon). |

**A novel twist we'll use:** because the bend is a shader uniform profile, we can **exaggerate curvature visually beyond the physics**, which is the cinematic "swing" of the 1986 original. We can also animate the horizon curvature factor `k` for dramatic crests, such as the summit reveal in Mountain Pass.

### 3.2 Terrain and mountains

- **Ridged-FBM heightfield strips** flanking the track (Inigo Quilez, *fbm*). Generated in Rust per chunk, about 20–60k triangles, and they bend along with the world. **Yes.**
- **Analytic anti-aliased grid and wireframe lines:**
  - The line is `d = abs(fract(uv·N)−0.5)/fwidth(uv·N)` and `line = 1−smoothstep(0,1.5,min(d))`, which gives constant pixel width and is anti-aliased without MSAA.
  - For a true triangle wireframe, use barycentrics with the Euclidean length of dFdx/dFdy (tchayen; fauder 2025).
  - Output at HDR intensity above 1 so it blooms, and fade it with distance to suppress moiré.
  - Nearly free. **Yes; this is the signature element.**
- **Layered silhouette strips** for the far distance: 3–4 strips fogged towards the sky colour, with an emissive rim line. **Yes** (far tier and mobile).

### 3.3 Sky, sun and fog

- **Analytic sun:**
  - an SDF disc with a gold→magenta gradient;
  - horizontal slits from `step(fract((y+t·s)·f), cutoff(y))`, scrolling slowly;
  - an HDR core of 4–8 so it blooms into a halo (examples: Godot retro-sun, Maxime Heckel's vaporwave scene).
- **Sky:** a gradient, hashed star cells with twinkle, and optional magenta-bellied cirrus bands.
- **Fog** (Inigo Quilez, *fog*):
  - amount `1−exp(−d·b)`, with height falloff;
  - the **fog colour is sampled from the same sky function** along the view ray, plus a sun-direction in-scatter term `pow(max(dot(rd,sun),0),8)`.
  - Distant geometry then dissolves into the horizon glow, which also hides the draw distance.
- Cost under 0.2 ms. **Yes.**

### 3.4 Scenery

- **Instancing:** palms, lamps, signs and arches as instanced meshes. Instance data is `(trackZ, lateralX, scale, variant, tint)` in track space, so the bend applies automatically. One draw per mesh type. **Yes.**
- **Procedural neon city:**
  - instanced boxes with window cells hashed from world position into lit or unlit, coloured from the palette;
  - emissive edge trims from a box-space `fwidth` edge mask, and blinking antennae;
  - 2–5k buildings in 1–3 draws.
  - **Yes.**
- **Impostors and LOD:** beyond about 600 m, swap to camera-facing quads, or let objects drop into the fog. Skylines beyond fog range become a single silhouette strip. **Yes.**

### 3.5 HDR, bloom and post-processing

- **Bloom:** the COD: Advanced Warfare chain (Jimenez, SIGGRAPH 2014; LearnOpenGL "Phys. Based Bloom"):
  - 13-tap downsample, with a Karis average on the first mip to kill fireflies;
  - 9-tap tent-filter upsample with additive blend;
  - 5–6 mips, about 0.5 ms at 1080p.
  - Bevy's and kiss3d's WGSL implementations are compact references (study them, don't depend on them).
  - On mobile, use ARM's dual-Kawase filter instead (Bjørge, SIGGRAPH 2015).
  - **Essential.**
- **Tonemapping:**
  - **ACES** (fitted) hue-shifts saturated magentas and blues.
  - **AgX** desaturates highlights gracefully, so neon cores go white with coloured halos. That is ideal for neon.
  - **Tony McMapface** (a 48³ LUT, Bevy's default) is very neutral.
  - **Decision:** AgX, followed by an authored 32³ synthwave grading LUT ("punchy" look). ACES as an option.
- **Composite-pass effects**, all fused into one pass for about 0.4–0.6 ms:
  - radial chromatic aberration scaled by speed;
  - film grain (after tonemapping);
  - vignette;
  - optional CRT (scanlines, barrel distortion, aperture mask);
  - optional VHS (chroma bleed, tracking bands);
  - optional ordered-dither palette ("8-BIT").
- **Motion and speed:**
  - Cheap **radial blur** about the vanishing point: 8–12 taps, scaled with speed, with a sharp centre. About 0.3 ms. Reads perfectly in a forward-scrolling racer. **Yes.**
  - Velocity-buffer blur (McGuire): **Maybe**, desktop-high only.
  - Speed lines as instanced view-space quads; FOV kick of 60°→75° on boost; camera shake as trauma² × noise. **Yes.**
- **TAA: No.** It smears thin neon lines, which is exactly what defines the look.

### 3.6 Wet-road reflections (ranked by value)

1. **Fake reflection from the bloom mip chain:**
   - The road shader samples mip 1–2 of the **previous frame's** bloom chain at screen UV mirrored about the horizon.
   - Apply a Fresnel term, a vertical streak jitter, and a wetness/puddle mask.
   - The result is long neon smears in the rain-soaked synthwave style, for about 0 ms extra. **Yes.**
2. **Additive streak quads** under each lamp and tail light. **Yes**, and they stack with option 1.
3. **Screen-space reflections:** 0.5–1 ms and edge artefacts. **Maybe**, desktop-high only.
4. **Flipped geometry:** doubles the draws and conflicts with the bend. **Maybe**, for a few hero emitters only.

### 3.7 Particles

- **Baseline:** particles simulated on the CPU in Rust (5–20k, about 0.3 ms) and drawn as instanced additive HDR quads. Deterministic and portable. **Yes.**
- **WebGL2 transform feedback** is not exposed by wgpu. **No.**
- **WebGPU compute particles** (100k+). **Maybe**, as an Ultra-tier enhancement only, because the WebGL2 baseline must look complete without them.
- **Uses:** tyre sparks that change colour with drift tier, nitro plume, off-road dust, crash debris, rain in Neon City, and fireworks.

### 3.8 Cars

- **Procedural low-poly meshes**, 500–3k triangles:
  - flat palette shading;
  - Fresnel rim light keyed to sky colours (cyan above, magenta below);
  - HDR tail-light strips at intensity 6–10, which bloom into trails;
  - an additive underglow decal.
- **Yes.** Pixel-art sprites clash with the lighting and the bend, so they're at most a "classic" skin. **Maybe, post-v1.**

### 3.9 Ideas for a distinctive identity

| Idea | Cost | Verdict |
|---|---|---|
| **Beat-synced uniforms.** The music conductor's beat phase drives grid intensity, sun-band scroll, bloom (+20% on the kick), window flicker and stripe colour on each bar. | 0 | **Yes. Highest return of any idea here, and it ties audio and visuals together.** |
| **Datamosh crash:** freeze the frame and push it around with a blocky 16×16 noise field for about 0.5 s | ~0.2 ms | **Yes** (hero moment; disabled by "reduced flashing") |
| **VHS** in pause, menus and on "signal lost" | ~0.1 ms | **Yes** (contextual) |
| **8-BIT dither mode:** Bayer matrix, 16–32 colour palette, low resolution | Saves time | **Yes** (doubles as a performance mode) |
| **Vector-display skin:** draw only edges, with phosphor persistence (blend in the previous frame at 0.85) | ~0.2 ms | **Maybe** (unlockable) |
| **Horizon-curvature drama:** animate `k` at crests and summits | 0 | **Yes** |

### 3.10 Distant-scenery efficiency

- **Culling:** the track is ordered, so culling is a 1D range query over segments, plus a bent-bounds frustum test for objects off to the side.
- **Draw distance:** fog reaches about 95% at the far plane, so nothing pops in.
- **LOD:** two mesh levels plus an impostor, with parallax backdrops beyond the far plane.
- **Draw-call budget: 150 or fewer.** Each WebGL call crosses the WASM→JS boundary, and that is the real CPU limiter.

---

## 4. Audio and procedural music

### 4.1 Architecture options

| Option | Pros | Cons |
|---|---|---|
| **(1) Rust DSP in an AudioWorklet.** A separate `no_std`-style `cdylib` with plain `extern "C"` exports and no wasm-bindgen, because the worklet scope lacks TextDecoder and similar. The module is compiled on the main thread and posted to the processor. Parameters travel via `port.postMessage`, with no SharedArrayBuffer. References: Chrome's AudioWorklet design-pattern article, PaulBatchelor/rust-wasm-audioworklet, waw-rs, Glicol. | Sample-accurate timing. Any DSP we want: PolyBLEP supersaws, ladder filters, chorus, saturation, a custom engine. Deterministic. Rust code is shared with the rest of the game. | A second WASM instance (about 100–300 KB). Real-time discipline (no allocation in `process`). Reported Safari underruns (WebKit 214258). |
| **(2) Native Web Audio node graph plus a look-ahead scheduler** (Chris Wilson, *A Tale of Two Clocks*) | Very robust on iOS. Built-in nodes are cheap. | A supersaw needs 7 OscillatorNodes per voice, with no control over aliasing. Churn from creating and destroying nodes. Game reactions lag by the look-ahead window. A synthetic engine is hard. |
| **(3) Hybrid (recommended).** A Rust *conductor* in the game WASM owns tempo, bars, the intensity state machine and the stinger queue, and sends timestamped events about 100 ms ahead. A single Rust worklet renders music, engine, tyres and traffic. One-shot SFX can be pre-rendered by the same DSP at load time into AudioBuffers. | The best of (1), with musical logic next to the game state. | Two WASM builds to manage. |

The official wasm-bindgen AudioWorklet example needs nightly Rust, atomics and COOP/COEP. **That is not viable on Pages**, which is why we use the no-bindgen pattern.

### 4.2 Synthwave sound recipes

All of these can be synthesised, so no samples are needed.

- **Tempo and harmony:** 112–124 BPM for driving; half-time feel for menus. Minor keys. Progressions i–VI–III–VII and i–VI–VII, with sus2, add9 and m7 colours.
- **Supersaw pad:** 7 PolyBLEP saws, ±10–25 cents of detune with random phases, a 2–4 kHz low-pass, slow attack and long release, then chorus and a big reverb.
- **PWM pad:** an LFO at 0.3–3 Hz sweeping pulse width between 10% and 90%.
- **Juno-style chorus:** two delays of 2–5 ms swept by a triangle LFO at about 0.5 Hz or 0.8 Hz, in anti-phase between left and right. (Commonly cited values; unverified.)
- **Octave bass:** straight 8ths, root–octave, through a low-pass with a short envelope, ducked by the kick.
- **Arpeggio:** 16th notes over chord tones, plucky filter envelope, dotted-8th ping-pong delay.
- **Drums:**
  - **Kick:** a sine sweeping 150→50 Hz over about 100 ms, with a 0.3–0.8 s decay, plus a click.
  - **Snare:** a triangle at about 200 Hz plus high-passed noise, through a **gated reverb** (reverb return × a rectangular 150–250 ms envelope).
  - **Hats:** 6 detuned squares, or noise through a 7–10 kHz band-pass.
- **Sidechain pump:** an envelope driven by the sequencer's kick events, ducking 4–8 dB with 150–250 ms recovery. No compressor needed, and the timing is perfect.
- **Master bus:** `tanh` saturation for a tape feel, with optional wow and flutter (an LFO on a 5 ms delay).

### 4.3 Adaptive music without chaos

**Precedents:** iMUSE (1991) used pre-marked transition points. FMOD uses transition regions quantised to beats and bars, and Wwise uses stinger segments. Winifred Phillips' GDC 2021 talks cover vertical layering and horizontal resequencing.

**Rules we adopt:**
1. **Quantise everything.** Accents land on the next 1/8 note, layer changes on the next bar, section changes on the next 4-bar phrase.
2. **Hysteresis.** A layer turns on at threshold T and off only below T−Δ, and stays on for at least 2 bars.
3. **One stinger at a time,** at most one per beat, from a priority queue: checkpoint > nitro > overtake > near-miss.
4. **Key-locked accents.** Near-miss blips are tones of the current chord, climbing with the combo.
5. **Never transpose the music.** Use filters and layers for intensity instead.
6. **Duck the music** 2–3 dB under stingers.

**Effects:**
- **Speed:** the master bus low-pass opens from about 800 Hz to fully open, smoothed over about 200 ms.
- **Nitro:** a noise riser of 1–2 bars that lands the lead layer on the downbeat.
- **Crash:** a tape-stop (playback rate 1→0 over 300–500 ms) or a bitcrush pulse, re-entering on the next bar.

### 4.4 Engine and effects

- **Engine:**
  - Industry tools are granular (AudioMotors, REV), but they need recordings, which conflicts with original content only.
  - **We use a synthetic engine:** firing frequency `rpm/60 · cyl/2`, a pulse train or harmonic saws plus a sub-harmonic at f/2, a resonant low-pass that follows throttle, and `tanh` drive.
  - On-throttle and off-throttle layers (off-throttle adds crackle), ±0.5% jitter, and a 50 ms dip plus a pop on each gear shift.
  - It sounds stylised and on-theme.
- **Tyre squeal:** noise band-passed at 1–3 kHz plus a 600–1200 Hz sine with slight FM. Gain follows slip.
- **Traffic pass-by:**
  - Doppler factor `c/(c − v_rel·dir)`, clamped; gain proportional to 1/distance; stereo pan from lateral offset.
  - At most 4 voices, nearest first.
  - A positioned whoosh plays on near misses.

---

## 5. Arcade racing design research

- **OutRun (1986) and OutRun 2:**
  - Checkpoint time extension, where remaining time carries over and builds slack as you master the game.
  - Branching forks forming a route pyramid with 5 goals, the main source of replay value.
  - Runs of about 5 minutes.
  - OutRun 2's *Heart Attack* mode gives graded per-section micro-objectives. That's a strong template for variety on a fixed course.
- **Burnout 2/3:**
  - Boost is earned through risk: near misses, the oncoming lane, drifts.
  - Burnout 3 made boost spendable at any time.
  - Courses deliberately stage risk opportunities.
- **Mario Kart mini-turbo:** holding a drift charges colour-coded tiers, and the boost length on release scales with the tier.
- **Ridge Racer:** the power-slide is the core skill, and the key decision is when to drift and when to grip.
- **Fairness:**
  - Lane-based traffic rows with a guaranteed gap.
  - Spacing by time headway.
  - Nothing spawns inside the reaction window (time to impact under about 1.2 s).
  - Lane changes are telegraphed.
  - Density ramps per stage.
  - No rubber-banding, because the clock is the opponent.
- **Juice:** screen shake scaled by impact, 40–80 ms of hit-stop, FOV kick, chunky callouts tied to quantised stingers, and a road-colour flash at checkpoints.
- **Handling references:** Code inComplete's centrifugal push `x −= curve·(v/Vmax)²·k`; Frank Force's Hue Jumper off-road damping.

The concrete synthesis is in [GAME_DESIGN.md](GAME_DESIGN.md). Its numbers are starting proposals, not sourced figures.

---

## 6. Deployment research

- **Pipeline:**
  - GitHub Actions: `dtolnay/rust-toolchain` (wasm32 target) → install trunk (prebuilt binary, pinned) → `trunk build --release --public-url /<repo>/`.
  - Then `actions/upload-pages-artifact@v5` → `actions/deploy-pages@v5`, with permissions `pages: write` and `id-token: write`.
- **MIME types:** Pages serves `application/wasm` correctly, so `WebAssembly.instantiateStreaming` works. The AudioWorklet module is a `.js` file loaded with `audioWorklet.addModule`, using a URL relative to the document.
- **Caching:**
  - Everything is cached for 600 s. trunk's content-hashed filenames make the wasm and JS immutable in practice.
  - `index.html` may be up to 10 minutes stale, but it always references a consistent set of hashed files.
  - Assets loaded by our own code (the worklet `.js` and the audio `.wasm`) must also be hashed, or have `?v=<git sha>` appended.
- **Paths:** use relative URLs everywhere, and never a leading `/`. Use `--public-url` for anything trunk emits.
- **Compression:** gzip only, so budget gzip sizes. Precompressed `.br` files are ignored.
- **No headers:** we avoid needing SharedArrayBuffer or threads, so we don't need coi-serviceworker. That workaround forces a reload on first visit and breaks Safari's `credentialless` mode.
- **Verification after deploy:**
  - Run a CI job (Playwright against the live URL) that checks:
    - the page returns 200;
    - the `.wasm` is served as `application/wasm`;
    - the canvas renders non-black pixels;
    - there are no console errors;
    - the reported backend is WebGPU, or WebGL2 when forced.

---

## 7. Risks and opportunities

### Risks

| # | Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|---|
| R1 | Features built against WebGPU limits silently break on the WebGL2 fallback | High | High | Design to `downlevel_webgl2_defaults` only. CI and manual runs force WebGL2 (`?backend=gl`). M0 spike proves both paths. |
| R2 | wgpu WebGL2 build size goes over budget | Medium | Medium | Measure in M0 with twiggy. Budget: game WASM 2.5 MB or less gzip. Fallback: drop the `webgpu` feature if needed (WebGL2 alone covers 100%). |
| R3 | AudioWorklet underruns on Safari, or iOS unlock problems | Medium | High | M0 audio spike on real iOS. Process in 128-frame blocks with no allocations. Fall back to a reduced voice count. "Tap to resume" UI. |
| R4 | The bent-world projection makes gameplay and visuals disagree (collision vs what you see) | Medium | High | Simulation is entirely in track space, and the bend is purely visual and applied uniformly to everything, including cars. Proven in the M1 prototype. |
| R5 | Mobile GPU fill-rate (HDR plus bloom) | High | Medium | Quality tiers, dynamic resolution (0.6–1.0×), dual-Kawase bloom, an automatic quality governor based on frame time. |
| R6 | Procedural art looks "programmer-art" | Medium | **Very high** (pillar #1) | Art direction comes first. Palette and LUT decided in M1. Screenshot review gates at every milestone. Hero features (sun, grid, neon city, reflections) prioritised over breadth. |
| R7 | wgpu / wasm-bindgen version churn | High | Low | Pin exact versions. Upgrade deliberately, between milestones. |
| R8 | WASM→JS call overhead for WebGL draws | Medium | Medium | Instancing and draw budget of 150 or fewer. Batch uniforms into one buffer per frame. |
| R9 | Adaptive music sounds chaotic, or musically weak | Medium | High | Quantisation and hysteresis rules (§4.3). Hand-composed pattern banks per biome, varied procedurally rather than fully generative. Listening reviews at each milestone. |

### Opportunities

- **The music clock as the master pulse for visuals:** beat-synced uniforms. Cheap, and a distinctive selling point.
- **The world bend as a creative tool:** cinematic over-curvature at crests and horizon drama.
- **Fake reflections from the bloom chain:** a wet-neon look at near-zero cost.
- **All-procedural content:** tiny download, infinite variety, a daily seeded run.
- **Deterministic simulation:** enables replays and ghosts, plus golden-image and regression tests. Post-v1 ghost racing is cheap to add.

---

## 8. Sources

**Rendering**
- Lou's Pseudo 3D page: https://extentofthejam.com/pseudo/
- Code inComplete racer series: https://codeincomplete.com/articles/javascript-racer-v1-straight (also the v2-curves, v3-hills and v4-final articles)
- Frank Force, Hue Jumper: https://frankforce.com/how-i-made-a-3d-game-in-only-2k-of-javascript/
- World bending: https://notslot.com/tutorials/2020/04/world-bending-effect · Blightbound postmortem: https://gamedeveloper.com/business/bending-blightbound-s-world-to-lower-the-horizon
- Inigo Quilez: https://iquilezles.org/articles/fbm · https://iquilezles.org/articles/fog/
- Wireframes: https://tchayen.github.io/posts/wireframes-with-barycentric-coordinates · https://fauder.github.io/posts/2025/04/09/optimized-wireframe-rendering-part-i
- Bloom: https://www.iryoku.com/next-generation-post-processing-in-call-of-duty-advanced-warfare/ · https://learnopengl.com/Guest-Articles/2022/Phys.-Based-Bloom · ARM dual-Kawase (Bjørge, SIGGRAPH 2015)
- Tonemapping: https://bevy.org/examples/3d-rendering/tonemapping/
- Synthwave scenes: https://blog.maximeheckel.com/posts/vaporwave-3d-scene-with-threejs · https://godotshaders.com/shader/retro-sun-2d/ · https://github.com/Moukrea/retrowave-scene · Codrops light trails: https://tympanus.net/codrops/?p=44516

**Platform**
- WebGPU implementation status: https://github.com/gpuweb/gpuweb/wiki/Implementation-Status
- WebGPU support: https://caniuse.com/webgpu · https://web3dsurvey.com/webgpu
- `EXT_color_buffer_float`: https://web3dsurvey.com/webgl2/extensions/EXT_color_buffer_float
- wgpu: https://docs.rs/wgpu/latest/wgpu/struct.Limits.html · https://github.com/gfx-rs/wgpu/releases
- Bevy: https://github.com/bevyengine/bevy/blob/main/docs/cargo_features.md
- miniquad: https://github.com/not-fl3/miniquad
- Three.js WebGPU post-processing: https://threejs.org/manual/en/webgpu-postprocessing.html

**Audio**
- AudioWorklet design pattern: https://developer.chrome.com/blog/audio-worklet-design-pattern
- Rust in an AudioWorklet: https://github.com/PaulBatchelor/rust-wasm-audioworklet · https://github.com/chaosprint/glicol
- Audio scheduling: https://web.dev/articles/audio-scheduling
- WebKit audio bugs: https://bugs.webkit.org/show_bug.cgi?id=251532 · https://bugs.webkit.org/show_bug.cgi?id=214258
- Drum synthesis: https://dev.opera.com/articles/drum-sounds-webaudio
- Adaptive music: https://www.gamedeveloper.com/audio/horizontal-resequencing-and-dynamic-transitions-for-game-music-composers-from-spyder-to-sackboy-gdc-2021- · https://www.fmod.com/docs/2.00/studio/authoring-events.html
- Engine sound: https://www.audiokinetic.com/engine-sound-modeling-from-sampling-to-granular-synthesis-in-wwise

**Design**
- OutRun 2: https://en.wikipedia.org/wiki/OutRun_2
- Burnout: https://burnout.fandom.com/wiki/Driving_Skills
- Mini-Turbo: https://www.mariowiki.com/Mini-Turbo

**Deployment**
- GitHub Pages custom workflows: https://docs.github.com/en/pages/getting-started-with-github-pages/using-custom-workflows-with-github-pages
- coi-serviceworker: https://github.com/gzuidhof/coi-serviceworker
- getrandom: https://docs.rs/getrandom/latest/getrandom/
