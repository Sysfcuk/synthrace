# CLAUDE.md — SYNTHRACE

SYNTHRACE is a browser synthwave arcade racer. The engine, simulation, rendering and audio DSP are written in Rust, compiled to WebAssembly, and deployed to GitHub Pages.

Read these documents before making non-trivial changes:

- [docs/TECHNICAL_ARCHITECTURE.md](docs/TECHNICAL_ARCHITECTURE.md): stack, crate layout, render passes, audio design, budgets.
- [docs/GAME_DESIGN.md](docs/GAME_DESIGN.md): mechanics and tuning values.
- [docs/ROADMAP.md](docs/ROADMAP.md): the current milestone and its acceptance criteria.
- [docs/RESEARCH.md](docs/RESEARCH.md): the evidence behind each decision.

## Current phase

**M2 "Speed and Feel" is complete and deployed** (https://sysfcuk.github.io/synthrace/); see `docs/reviews/M2.md`. It is waiting on the user to playtest feel, gamepad, touch and audio. M3 "Arcade Loop and Adaptive Music" is next, but don't start it until the user says so. After each milestone, stop for review.

## Lessons learned (keep this list current)

- **naga accepts WGSL that Chrome's Tint rejects.** For example, `a * b ^ c` needs parentheses. Always run the browser build on both backends before claiming a shader works.
- **wasm-opt needs `--enable-bulk-memory`** and the other post-MVP feature flags for modules from Rust 1.100. These are set via `data-wasm-opt-params` in `web/index.html`.
- **The `trunk serve` watcher doesn't always pick up changes** under `crates/render/shaders/`. Rebuild with `trunk build --release` if in doubt.
- **The golden simulation hash** (`sim::GOLDEN_HASH_SEED42_60S`) must match between native and wasm. Any intentional change to handling or tuning changes it: update the constant and say so in the commit.
- **The world bend is purely visual.** Visual exaggeration (`VISUAL_CURVE_GAIN` and `VISUAL_HILL_GAIN` in `game::scene`) is safe only because every drawable goes through `bend()`. Never place anything by any other route.
- **Traffic fairness is tested physically** (a free lateral gap of 2.5 m or more), not by lane occupancy. Cars mid-lane-change only half-block.
- **Emissive marker codes** (`procgen::marker`, values of 50 and up) let the mesh shader light brake lights and indicators per instance. Ordinary emissive values must stay below 40.
- **The desktop runner's `--warm N` runs the live simulation with the autopilot,** and `--stress` forces 40+ cars and about 5,000 particles for performance checks.
- **A hidden browser pane throttles `requestAnimationFrame`.** In-browser frame times are meaningless unless the pane is visible.

## Non-negotiable constraints

- **The core is in Rust.** JS is limited to `web/boot.js` (audio unlock, worklet loading) and `web/worklet.js` (processor shim). Keep each under about 200 lines, with no game logic.
- **WebGL2 is the baseline.** Every feature must work within `wgpu::Limits::downlevel_webgl2_defaults()`:
  - no compute shaders;
  - no storage buffers or storage textures;
  - uniform buffers of 16 KiB or less.
  
  WebGPU-only work is allowed only as an *additive* Ultra-tier extra.
- **No threads and no SharedArrayBuffer.** GitHub Pages cannot send COOP/COEP headers.
- **Simulation stays in track space.** The world bend is purely visual and must never feed back into gameplay. The `sim` crate has no dependency on `wgpu`, `web-sys`, or any rendering or audio crate.
- **Determinism.** `sim` must give bit-identical results for the same seed and inputs. Use our PCG32 for randomness and never `getrandom` or `rand` in simulation code. Fixed 120 Hz step.
- **Real-time audio rules.** In `dsp`, never allocate, lock or panic inside `process`. Use fixed voice pools and flush denormals.
- **Original content only.** No OutRun or other copyrighted music, names, logos, tracks, characters or art. Procedural generation comes first. Any third-party asset or crate must have a licence compatible with MIT/Apache-2.0 or OFL, recorded in `docs/CREDITS.md`.
- **Budgets** (TECHNICAL_ARCHITECTURE §5.4). Exceeding one needs explicit user approval.
  - Game WASM 2.5 MB or less gzip; DSP WASM 150 KB or less gzip.
  - Draw calls 150 or fewer (desktop), 80 or fewer (mobile).
  - Desktop Medium p95 frame time 16.7 ms or less.
- **Paths are repo-relative.** Never use a leading `/` in asset URLs. trunk's `--public-url` handles the Pages subpath.

## Engineering standards

- Rust 2024 edition, stable toolchain pinned in `rust-toolchain.toml`. `cargo fmt` and `cargo clippy --workspace --all-targets -- -D warnings` must pass, as must `cargo clippy -p game -p dsp-worklet --target wasm32-unknown-unknown -- -D warnings`.
- **Dependencies:** minimal and pinned. The allowed runtime crates are `wgpu`, `glam`, `bytemuck`, `wasm-bindgen`, `wasm-bindgen-futures`, `web-sys`, `js-sys`, `log` and `console_error_panic_hook`. `pollster` and `png` are allowed only in `crates/desktop` (a dev tool, never shipped). `naga` is a dev-dependency of `render`, for shader tests only. Adding anything else needs a written justification and, for runtime crates, user approval. Use `default-features = false` where possible.
- **No ECS framework and no general-purpose engine.** Use plain data-oriented structs. Prefer simple, explicit code over abstraction layers. No speculative generality.
- **Shaders:** WGSL in `crates/render/shaders/`, composed with our `#include` preprocessor, embedded with `include_str!`, and validated by the naga test for both the WGSL and GLSL ES 3.00 backends.
- **Tuning values** (handling, scoring, music thresholds) live in one named `const` or config struct per system, documented with units, and are mirrored in GAME_DESIGN.md when they change meaningfully.
- **Errors:** no `unwrap()` on browser API results in the `game` crate; degrade gracefully (for example, no audio means a notice, not a crash). Panics in release builds are treated as bugs.
- Comments explain *why*, not *what*. Keep modules focused; split a file once it passes about 600 lines.

## Development workflow

1. Work milestone by milestone, following ROADMAP.md. Keep the milestone's acceptance checklist up to date.
2. Branch per feature, using `feat/m<N>-<topic>`. Commit only when the user asks. Never push directly to `main` unless told to.
3. Iterate with the native runner. `cargo run -p desktop --release -- --out out/shot.png` takes a headless screenshot; add `--gl` for the GLES path and `--bench 300` for timing. Then verify in the browser: `trunk serve --release`, and open `http://127.0.0.1:8080/` and `http://127.0.0.1:8080/?backend=gl`.
4. Before declaring any feature done:
   - native tests pass (`cargo test --workspace`);
   - the wasm build succeeds (`trunk build --release`) and is within size budget;
   - **you have run it in a browser** on **both** backends (default, and `?backend=gl`), with no console errors;
   - you have taken screenshots of the visual result and looked at them critically;
   - performance with `?debug` / `?bench` is within budget.
5. Write the milestone review in `docs/reviews/M<N>.md`: what was done, the measurements, screenshots, known weaknesses, and what wasn't verified.

## Verification honesty

- A successful compile is **not** evidence that the game works. Use screenshots, frame-time measurements, test output, and recordings of playing it.
- Never describe something as finished, polished or tested unless you have the evidence. Say plainly what was skipped or could not be checked (for example, "not tested on a real iOS device").
- Visual quality and handling feel are judged by the user. Present evidence and ask; don't self-certify.

## Quality bar

Visual quality and art direction come first, then speed sensation, then handling, then audio-gameplay integration, then performance and compatibility, then maintainability. Placeholder art is acceptable during a milestone, but it must be listed as a known weakness and never shipped as final. Static, lifeless scenes and music that just loops are defects.

## Useful commands

```
cargo test --workspace                                   # native tests incl. naga WGSL→GLSL ES validation
cargo run -p desktop --release -- --out out/shot.png     # headless screenshot (add --gl, --bench 300, --s <dist>, --speed <0..1>)
trunk serve --release                                    # local web build at http://127.0.0.1:8080 (?backend=gl forces WebGL2)
trunk build --release --public-url /synthrace/           # production bundle in dist/
bash tools/size_report.sh dist                           # gzip sizes + budget check
```
