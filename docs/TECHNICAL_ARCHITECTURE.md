# SYNTHRACE — Technical Architecture

> Status: Phase 0 recommendation, **pending approval**. The evidence behind it is in [RESEARCH.md](RESEARCH.md).

## 1. Summary of decisions

| Area | Decision | Main alternative rejected and why |
|---|---|---|
| Language | Rust 2024 edition for the engine, simulation, rendering and audio DSP. A thin JS layer (~200 lines) only for boot and the AudioWorklet shim. | — |
| Graphics API | **wgpu 30** (`webgpu` + `webgl` features) in **one binary**, which picks the backend at runtime via `new_instance_with_webgpu_detection`. Shaders are written in WGSL. | **Bevy:** 3–6 MB gzip, needs two separate binaries to use WebGPU, and breaks its API every ~4 months; its ECS, asset server and editor features are unused. **Three.js:** the renderer would not be Rust. **Raw WebGL2:** no WebGPU future. **Macroquad:** limited to WebGL1. |
| Baseline target | **WebGL2 feature level** (`downlevel_webgl2_defaults`). WebGPU only adds optional tiers. | Targeting WebGPU first would exclude 12–17% of users, mostly on Android. |
| Engine structure | No ECS. Plain data-oriented Rust structs, with the simulation separated from rendering. | ECS frameworks add complexity without benefit for one scene with a few hundred entities. |
| Road model | **Hybrid "Neon Bent-World":** segment-curvature track used for authoring and simulation, real 3D geometry in a straightened "track space", and vertex-shader bending into the view. | Classic pseudo-3D has too low a visual ceiling. A true 3D spline track loses the OutRun horizon and sun composition. |
| HDR pipeline | RGBA16F scene target, a COD:AW-style bloom mip chain, AgX tonemapping plus a grading LUT, and a single fused composite pass. | MSAA on float targets isn't portable on WebGL2. TAA smears neon lines. |
| Audio | **Hybrid:** a Rust *conductor* inside the game WASM, plus a separate small **Rust DSP WASM running in an AudioWorklet**, with no wasm-bindgen and no SharedArrayBuffer. Parameters are passed with `postMessage`. | A graph of native Web Audio nodes can't produce quality supersaws or a synthetic engine sound, and timing is loose. The wasm-bindgen threads example needs COOP/COEP headers, which GitHub Pages can't send. |
| Build | **trunk 0.22** for hashed filenames, wasm-opt and `--public-url`. Pinned `wasm-bindgen` version. | wasm-pack is aimed at npm packages. |
| Deploy | GitHub Actions → `upload-pages-artifact@v5` → `deploy-pages@v5`, followed by a Playwright smoke test against the live URL. | — |
| Threads | **None.** Everything is single-threaded on the main thread, plus the audio render thread. | coi-serviceworker forces a reload on first visit and breaks Safari's credentialless mode. |

## 2. Repository layout

```
synthrace/
├─ Cargo.toml                  # workspace; shared release profile
├─ rust-toolchain.toml         # pinned stable + wasm32-unknown-unknown
├─ CLAUDE.md
├─ docs/                       # RESEARCH, TECHNICAL_ARCHITECTURE, GAME_DESIGN, ROADMAP
├─ crates/
│  ├─ sim/          # pure, deterministic game simulation. NO web/wgpu deps. Native unit tests.
│  │                #   track (segments, profiles, biomes, forks), vehicle, traffic,
│  │                #   collisions, scoring, nitro/combo, run state machine, rng (PCG32)
│  ├─ procgen/      # procedural content: terrain heightfields, car meshes, palm/prop meshes,
│  │                #   city layouts, track generation from seeds. Pure, native-testable.
│  ├─ render/       # wgpu renderer: passes, pipelines, WGSL shaders, quality tiers, HUD/text
│  ├─ music/        # conductor: tempo, patterns, chords, layer state machine, stinger queue.
│  │                #   Pure; emits timestamped events. Native-testable.
│  ├─ dsp/          # #![no_std]-friendly synth engine (oscillators, filters, FX, voices).
│  │                #   Compiles to (a) the AudioWorklet cdylib and (b) native for tests/offline render.
│  ├─ dsp-worklet/  # thin cdylib wrapper: extern "C" alloc/process/param fns, no wasm-bindgen
│  ├─ game/         # wasm entry (wasm-bindgen): main loop, platform (input, storage,
│  │                #   audio bridge, resize, visibility), UI/menu state, glue between crates
│  └─ desktop/      # native winit runner of the same game for fast iteration,
│                   #   headless screenshots and perf captures (dev only, never shipped)
├─ web/
│  ├─ index.html    # trunk entry; canvas, boot overlay, no-WebGL2 fallback message
│  ├─ boot.js       # audio unlock, worklet module loading, iOS audioSession
│  └─ worklet.js    # AudioWorkletProcessor shim that instantiates dsp-worklet.wasm
├─ tests/e2e/       # Playwright: smoke, backend forcing, screenshots, perf traces
├─ tools/           # LUT baker, font/atlas baker, size report scripts
└─ .github/workflows/  # ci.yml (fmt, clippy, tests, size budget), pages.yml (deploy + verify)
```

**Dependency policy:**

- Runtime crates are limited to `wgpu`, `glam`, `bytemuck`, `wasm-bindgen`, `wasm-bindgen-futures`, `web-sys`, `js-sys`, `log` and `console_error_panic_hook`.
- `winit` and `pollster` are allowed only in `desktop`.
- Adding any other crate needs a one-line justification in its PR.
- Randomness uses our own PCG32 so the simulation is deterministic. That also avoids the `getrandom` dependency.

## 3. Runtime architecture

```
                     requestAnimationFrame (main thread)
                                   │
 ┌──────────── game (wasm-bindgen) ────────────────────────────────────────────┐
 │ Platform: Input (keyboard, Pointer Events, Gamepad API) · Storage (localStorage) │
 │           Resize/DPR · Visibility · Audio bridge (port.postMessage)          │
 │                                                                              │
 │  Frame:                                                                      │
 │   1. poll input → InputState (digital shaping, analog curves)                │
 │   2. accumulator: while acc ≥ 1/120 s → sim.step(input)   (deterministic)    │
 │   3. sim events (near-miss, overtake, crash, checkpoint…) → music.conductor  │
 │      → timestamped AudioEvents + continuous params (rpm, slip, speed)        │
 │   4. render.frame(sim snapshot (interpolated α), beat phase, quality)        │
 └──────────────────────────────────────────────────────────────────────────────┘
          │ postMessage (≤60 Hz, small structs)              │ wgpu
          ▼                                                  ▼
 ┌─ AudioWorklet thread ─────────────────┐       WebGPU  or  WebGL2
 │ worklet.js → dsp-worklet.wasm          │
 │  voices: music (pad, bass, arp, lead,  │
 │  drums), engine, tyres, 4× traffic,    │
 │  stingers; FX busses; master limiter   │
 └────────────────────────────────────────┘
```

- **Fixed timestep:** the simulation runs at 120 Hz, and rendering interpolates between the previous and current states. At most 8 simulation steps run per frame, to avoid a death spiral; after a long hitch we drop time rather than catch up.
- **Snapshot boundary:** the renderer reads an immutable `RenderSnapshot` that the simulation builds each frame (car transforms in track space, traffic, particles, track window, events). The renderer never mutates game state.
- **Clock domains:**
  - The **audio clock** (`AudioContext.currentTime`) is the musical master clock.
  - The worklet sends `beat_phase` and `bar` updates back each render quantum, but never more than about 60 Hz.
  - The game uses them for beat-synced visuals and quantised UI callouts.
  - The simulation keeps its own fixed-step clock and never waits for audio.

## 4. Simulation (`sim` crate)

**Track:**

- A track is a sequence of `Segment { length, curve, hill, bank_visual, biome_mix, lanes, oncoming, props_seed }`, each about 5 m long.
- Tracks are built from a high-level, seeded *stage script* by `procgen`. The script contains elements like straight, sweeper, S-bend, crest and tunnel, plus the fork.
- **Profiles:** the per-segment `curve` and `hill` values are accumulated into the lateral offset `C(z)` and height `H(z)` relative to the camera. These are the same arrays the renderer uploads, so what you see always matches what you drive.

**Coordinates:**

- **Every gameplay quantity is in track space:** `s` is distance along the track, `x` is lateral position (−1 to 1 is the road), and `y` is height.
- Collision tests are 1D or 2D checks in track space. The visual bend is never used for gameplay.

**Vehicle and traffic:**

- The vehicle model follows GAME_DESIGN §4: speed, steering, centrifugal push, off-road, drift and gears.
- Traffic is a ring buffer of up to 64 cars, driven by lane-following behaviour. Rows are spawned by a fairness-checked spawner.

**Event bus and state machines:**

- Each step pushes typed `SimEvent`s, such as `NearMiss{gap, side}`, `Overtake`, `Crash{kind}`, `Checkpoint{bonus}`, `DriftTier(n)`, `NitroStart` and `NitroEnd`, consumed by scoring, music and VFX.
- The run state machine is `Countdown → Racing → ForkChoice → … → Goal | TimeOver → Results`.

**Determinism:**

- Given the same seed and inputs, the simulation must produce bit-identical results.
- We use `f32` with no fast-math and no platform-dependent transcendental functions in critical paths. `sin` and `cos` use our own approximations where results must match across targets.
- This makes input-log replay tests and future ghost replays possible.

## 5. Rendering (`render` crate)

### 5.1 Capability tiers

| Tier | Selected when | Additions |
|---|---|---|
| **Low** | Mobile, or the frame-time governor demands it | 0.6–0.75× dynamic resolution, dual-Kawase bloom with 4 mips, 2k particles, about 60% fog distance, silhouette strips for mountains |
| **Medium** | The default on WebGL2 desktop | Full Neon Bent-World stack at 1.0×, 6-mip COD:AW bloom, 8-tap radial blur |
| **High** | Desktop with headroom | 12-tap radial blur, more instances and draw distance, render scale up to 1.25× |
| **Ultra** | WebGPU only | Compute particles (100k), optional screen-space reflections and 4× MSAA where supported. These must **only add** to an already-complete look. |

An **automatic quality governor** tracks rolling GPU and CPU frame time. It first adjusts render scale, then steps the tier up or down, with hysteresis so it doesn't flip back and forth.

### 5.2 Frame graph

The frame graph is a fixed, hand-written list of passes, not a generic render graph.

| # | Pass | Target |
|---|---|---|
| 0 | Upload: profile texture (256 × RGBA16F: C, H, bank, fog density), per-frame UBO, instance buffers, particles | — |
| 1 | **Opaque scene**: road (lanes, stripes, wet mask, fake reflection sampling the previous frame's bloom mip 2), grid floor and terrain (analytic `fwidth` lines), cars (rim light), instanced props and city. Fog and sky-matched in-scatter are computed inline. | RGBA16F + Depth24 |
| 2 | **Sky**: gradient, banded sun, stars, clouds, drawn at depth = far | same |
| 3 | **Transparent and additive**: particles, light streaks, speed lines, nitro plume, tail-light trails | same |
| 4–5 | **Bloom** downsample and upsample chain (Karis average on the first mip) | RGBA16F mips |
| 6 | **Composite**: bloom add, radial speed blur, chromatic aberration, exposure, AgX, 32³ grade LUT, vignette, grain, FXAA-lite, then optional CRT, VHS, dither and datamosh | swapchain (sRGB) |
| 7 | **HUD and UI**: SDF/vector glyphs and shapes in screen space, with emissive elements drawn back into the bloom input | swapchain |

The HUD bloom is done by drawing HUD emissive elements into the scene target before pass 4, and the HUD's crisp layer after pass 6.

### 5.3 The bend (vertex stage, shared WGSL include)

```wgsl
// track-space position p (x lateral, y up, z ahead of camera) → bent view-space position
fn bend(p: vec3f) -> vec3f {
    let t = sample_profile(p.z);              // C(z), H(z) from 256-texel profile
    return vec3f(p.x + t.x, p.y + t.y - u.horizon_k * p.z * p.z, p.z);
}
```

- **Every** mesh goes through `bend()`. Mesh normals are bent approximately, using the finite-difference slope of the profile.
- **Culling** is a range query over segments, followed by a test of bounding boxes padded for the bend.
- Geometry is tessellated along Z every 2–5 m, so long triangles still curve smoothly.

### 5.4 Budgets (target)

| Metric | Desktop Medium (1080p, RTX 3060 / RX 6600 class) | Mobile Low (2023 mid-range phone) |
|---|---|---|
| GPU frame | 5 ms or less | 10 ms or less |
| CPU frame (sim + render prep + audio bridge) | 4 ms or less | 8 ms or less |
| Draw calls | 150 or fewer | 80 or fewer |
| Game WASM (gzip) | 2.5 MB or less | same |
| DSP WASM (gzip) | 150 KB or less | same |
| Total first-load transfer | 3.5 MB or less | same |
| Time to first frame (cold, fast 4G) | 4 s or less | same |

Shaders are WGSL. They are included at compile time with `include_str!` and composed with a small `#include` preprocessor, so they need no runtime loading. They are checked by `naga` in a unit test, which compiles every shader to both the WGSL and GLSL ES 3.0 backends.

### 5.5 Procedural assets (`procgen` crate)

Everything visual is generated in code at load time, from seeds and biome parameter sets:

- terrain;
- car bodies, built from a parametric hull plus wheels and light strips;
- palms, cacti, pylons, lamps and signs;
- city blocks;
- the star field;
- the title wordmark, as SDF geometry.

**Committed binary assets are limited to:**

- the 32³ grade LUTs, baked by `tools/` from code and stored as a small PNG;
- at most one OFL-licensed font atlas, if the vector-font approach is rejected in M4.

## 6. Audio architecture

```
 game wasm                             AudioWorklet (dsp-worklet.wasm, own memory)
 ─────────                             ───────────────────────────────────────────
 music::Conductor                      Voice pool + FX:
   tempo, bar/beat                       pad (7× PolyBLEP saw, chorus) · bass · arp
   pattern banks per biome               lead (PWM) · drums (kick/snare/hats, gated verb)
   layer state machine (hysteresis)      engine (pulse train + sub, LP, tanh, 2 load layers)
   stinger queue (priority, quantised)   tyre squeal · 4 traffic voices (doppler, pan)
   → AudioEvent{t_audio, kind, …}        stingers/whooshes/impacts · UI blips
 continuous params @≤60 Hz:              busses: music (LP sweep, sidechain env, tape-stop)
   rpm, throttle, slip, speed,                   sfx · master (soft clip, limiter)
   nitro, traffic[4] {dist, dv, pan}   → posts back {bar, beat_phase} for visuals
```

**Loading the worklet:**

1. The main thread fetches and compiles `dsp-*.wasm` (`WebAssembly.compileStreaming`).
2. It calls `audioWorklet.addModule('worklet-<hash>.js')`.
3. It creates the `AudioWorkletNode` and passes in the `WebAssembly.Module` via `processorOptions`.
4. The processor instantiates the module synchronously with no imports, calls `init(sample_rate)`, and then calls `process(n)` on each 128-frame block.

**Real-time rules:**

- No allocation after `init`.
- Fixed voice pools.
- Denormal flushing.
- The parameter-message queue is drained at the start of each block.

**Unlock and lifecycle:**

- A "PRESS START" gesture creates and resumes the `AudioContext`.
- It also sets `navigator.audioSession.type='playback'` where supported.
- We listen to `visibilitychange` and to context `statechange` events. If the context is interrupted, a "tap to resume" overlay appears.

**Settings:** Master, Music and SFX gains and mute are worklet parameters, persisted in `localStorage`.

**Fallback:** if the AudioWorklet or its WASM fails, the game keeps running and shows an "audio unavailable" notice. It never crashes.

**Testing:** `dsp` and `music` run natively. Tests can render deterministic WAV files offline, check them for NaN, denormals and clipping, and run perceptual sanity checks such as RMS and spectral centroid against expected ranges.

## 7. Browser integration

- **Canvas and resolution:**
  - A full-viewport canvas, with backing-store size = CSS size × `min(devicePixelRatio, 2)` × render scale.
  - Resizes are handled with a `ResizeObserver`.
  - Safe-area insets come from `env(safe-area-inset-*)` and are passed to the HUD layout.
- **Input:**
  - `keydown`/`keyup` on `window`, using `event.code` so controls follow physical key position on any keyboard layout. Rebindable.
  - Pointer Events on the canvas with `touch-action:none` and multi-pointer tracking. On-screen controls are rendered by our renderer.
  - Optional tilt steering through `DeviceOrientationEvent`, requested through iOS's permission prompt on a user gesture.
  - Gamepad API polled once per frame, with standard mapping, deadzone and response curve. Rumble through `vibrationActuator` where available.
- **Lifecycle:** pausing on blur or visibility loss automatically pauses the game and suspends audio.
- **Fullscreen and orientation:** a fullscreen button, and a landscape prompt on phones.
- **Fallbacks:**
  - If neither WebGPU nor WebGL2 is available, a static HTML message explains what's required.
  - The backend can be forced with `?backend=gl|webgpu` for testing.
  - `?debug` shows an overlay with frame times, tier, backend and draw-call count.

## 8. Build and deployment

**Release profile:**

```toml
[profile.release]
opt-level = 3          # benchmark vs "s" in M0; choose by size+fps evidence
lto = "fat"
codegen-units = 1
panic = "abort"
strip = true
```

- **trunk** builds `crates/game` with `data-wasm-opt="z"` (or `"3"`, decided by measurement in M0) and copies `web/`.
- **The DSP WASM** is built by a trunk pre-build hook: `cargo build -p dsp-worklet --target wasm32-unknown-unknown --release`, then `wasm-opt`.
  - trunk then copies it as a hashed asset.
  - Its hashed URL is injected into `index.html` as `data-` attributes read by `boot.js`.
- **Pages workflow** (`pages.yml`, triggered on push to `main`):
  1. `actions/checkout@v5` and a pinned Rust toolchain with the wasm32 target.
  2. Install trunk and wasm-bindgen-cli from pinned prebuilt binaries (cached).
  3. `cargo test --workspace` (native).
  4. `trunk build --release --public-url /<repo-name>/`.
  5. Size check: fail if any budget in §5.4 is exceeded, measured as gzip size.
  6. `actions/upload-pages-artifact@v5`, then `actions/deploy-pages@v5` (permissions `pages: write`, `id-token: write`).
  7. **Verify job**, Playwright against the deployed URL:
     - HTTP 200;
     - `application/wasm` MIME type;
     - the canvas has non-black pixels after the boot click;
     - no console errors;
     - both WebGPU and forced WebGL2 backends render.
- **Caching:** all generated files have hashed names. `index.html` is the only file whose name never changes, and Pages caches it for 600 s.
- **Paths:** relative URLs only. `--public-url` handles the repository subpath.
- **CI** (`ci.yml`, on pull requests): `cargo fmt --check`, `clippy -D warnings`, native tests, naga validation of every shader, a wasm build, and a size report comment.

## 9. Testing architecture

| Layer | Tooling | What it proves |
|---|---|---|
| Unit | `cargo test` (native) on `sim`, `procgen`, `music` and `dsp` | Physics invariants, scoring rules, fairness of the traffic spawner, quantisation and hysteresis of the music, DSP stability |
| Determinism | Replays of recorded input logs, checking state hashes at checkpoints | Identical results across runs, and across native and wasm (the wasm side runs in a Playwright job) |
| Shader | naga parses and validates every WGSL file and translates it to GLSL ES 3.00 | Shaders compile for the WebGL2 path, without needing a browser |
| Golden images | The `desktop` runner renders seeded scenes headless; screenshots are compared with a perceptual tolerance | No accidental visual regressions. Updates need human review. |
| E2E | Playwright with Chromium (WebGPU and forced GL), Firefox and WebKit | Boot, audio unlock flow, input, pause/resume, settings persistence, no console errors |
| Performance | `?bench`: a deterministic 60 s attract-mode run reporting p50/p95/p99 frame times, draw calls and tier | Each milestone meets its budget; regressions are tracked |
| Manual | Milestone review checklist covering feel, look and sound, on desktop, Android and iOS | Things automated tests can't judge |

## 10. Architectural risks and fallback plans

| If… | Then… |
|---|---|
| wgpu with WebGL2 can't hit the size budget or causes blocking bugs in the M0 spike | Ship WebGL2 only (drop the `webgpu` feature), which still reaches 100% of browsers. If wgpu itself proves unusable, fall back to `glow` with the same WGSL shaders translated by naga at build time. |
| The Rust DSP AudioWorklet is unreliable on iOS | Move SFX to native Web Audio nodes driven from Rust and keep the music in the worklet, or pre-render music stems offline with the same DSP and play them as layered AudioBuffers. |
| The bent world produces artefacts, such as cars and road disagreeing at extreme curvature | Clamp the visual over-curvature, and bend cars using their own track-space z. Proven in M1. |
| The procedural art doesn't meet the quality bar | Escalate to the user. The options are authoring a handful of hero meshes as code-defined geometry, or commissioning or creating original assets. Never use unlicensed assets. |
