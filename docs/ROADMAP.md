# SYNTHRACE — Roadmap

> Status: Phase 0 proposal, **pending approval**.
>
> - Every milestone ends with a **deployed build** on GitHub Pages, **screenshots**, a **performance report**, and a short **review note**: what was done, what was measured, what is known to be weak.
> - A milestone is not done because it compiles.
> - Durations are rough effort estimates, not commitments.

## Dependency overview

```
M0 Spikes ──► M1 First Drive ──► M2 Speed & Feel ──► M3 Arcade Loop + Music ──► M4 Worlds ──► M5 Presentation ──► M6 Polish & v1.0
   (architecture gate)  (playable prototype)                       ▲                    │
                                                                  └── audio DSP from M0 ┘ biomes feed music themes
```

Two gates need user approval:

- **after M0**, to confirm the architecture is viable;
- **after M1**, to confirm the art direction and handling feel.

---

## M0 — Technical spikes and pipeline (≈1 week)

**Goal:** prove the risky parts before building on them. Most of this code is disposable, except the workspace and CI.

**Work:**

- Workspace skeleton (per TECHNICAL_ARCHITECTURE §2), pinned toolchain, trunk config, CI and Pages workflows.
- **Render spike:**
  - wgpu with automatic backend detection; draws a bent grid plus the analytic sun into RGBA16F.
  - Bloom chain and AgX tonemap.
  - Runs on WebGPU and on forced WebGL2 (`?backend=gl`).
- **Audio spike:**
  - A no-bindgen `dsp-worklet.wasm` playing a PolyBLEP supersaw chord plus a kick through the AudioWorklet.
  - Unlocked by a tap; `postMessage` parameters change the filter cutoff live.
- **Measurements:**
  - gzip size of the game WASM (opt-level 3 vs s; wasm-opt z vs 3);
  - time to first frame;
  - GPU frame time on desktop and one Android device.

**Acceptance criteria:**

- [ ] The public Pages URL loads and renders the sun-and-grid scene on Chrome (WebGPU), Firefox and Safari (or WebKit via Playwright), and on Chrome with WebGL2 forced.
- [x] The post-deploy verify job passes: wasm served with the right MIME type, non-black canvas, no console errors.
- [ ] The audio spike plays without glitches for 60 s on desktop Chrome, Firefox and iOS Safari (manual), and it resumes after the tab goes to the background.
- [x] Measured sizes are recorded in `docs/perf/M0.md`. The game WASM is **2.5 MB or less gzip** (if not, trigger the fallback in TECHNICAL_ARCHITECTURE §10).
- [x] naga validation of every shader for the GLSL ES target runs in CI.

**Gate:** I present the measurements and the go/no-go on wgpu and the worklet. **The user approves before M1.**

---

## M1 — "First Drive": playable prototype (≈2 weeks)

**Goal:** the earliest version that is fun to steer, which should already look like *this* game and not a generic one.

**Work:**

- **Simulation:** track segments and profiles; a seeded straight/curve/hill generator; fixed-step 120 Hz loop with interpolation; vehicle model (speed, steering shaping, centrifugal push, off-road); keyboard input.
- **Rendering:**
  - Neon Bent-World road (lanes, emissive stripes, rumble strips);
  - grid floor and ridged wireframe mountains;
  - sky, sun and stars;
  - fog sampled from the sky colour;
  - a first procedural player car (low-poly hull, rim light, HDR tail lights);
  - chase camera with speed-based FOV;
  - composite pass (bloom, AgX, grade LUT v1, vignette, grain).
- **Debug overlay:** frame times, backend, draw calls, speed and segment.
- **Desktop runner:** headless screenshot capture.

**Acceptance criteria:**

- [ ] 60 fps or better (p95 frame of 16.7 ms or less) on the reference desktop at 1080p on both backends. Report checked in.
- [ ] Driving feel: steering responds within one frame. Curves at top speed need visible counter-steer. Off-road clearly penalises. Reviewed by the user by playing the deployed build.
- [x] Visual: 4 reference screenshots (straight, curve, crest, dip) checked in, and they read clearly as synthwave/OutRun at a glance. **The art-direction review is with the user.**
- [x] Unit tests: speed, steering and centrifugal invariants; a determinism hash test over a 60 s recorded input log (native).
- [x] No gameplay or visual disagreement: the car's lateral position on screen matches its simulation position through curves (screenshot test at 3 curve intensities).

**Gate:** the user approves the art direction and handling baseline.

---

## M2 — Speed and Feel (≈2 weeks)

**Goal:** make it thrilling, through traffic, risk and the sensation of speed.

**Work:**

- Traffic system with a fairness-checked spawner, lane changes with indicators, and vehicle archetypes (procedural meshes).
- Collisions: bounce, spin-out and scrape.
- Near-miss and overtake detection.
- Drift with charge tiers; nitro meter and boost.
- Speed effects: radial blur, speed lines, chromatic aberration, FOV kick, camera shake, hit-stop.
- CPU particle system for sparks, nitro plume and dust.
- Gamepad support (standard mapping, analog curves, rumble).
- Touch controls (on-screen zones, auto-accelerate) and the landscape prompt.
- Engine, tyre-squeal, traffic-doppler and impact SFX in the DSP worklet.

**Acceptance criteria:**

- [x] Unit tests:
  - the spawner never produces an unpassable row, or a spawn inside the 1.2 s reaction window (property test over 10k seeds);
  - near-miss detection thresholds;
  - drift tier timings.
- [x] 60 fps on desktop with 40 traffic cars and 5k particles. Draw calls 150 or fewer.
- [ ] Playable with keyboard, an Xbox/PlayStation-layout gamepad, and touch on an Android phone and an iPhone (manual checklist).
- [ ] Engine pitch audibly follows RPM and gear changes. No clicks or pops in a 5-minute session (manual, plus an offline DSP test checking for NaN, clipping and denormals).
- [ ] A recorded gameplay clip or screenshot sequence shows near miss, drift sparks and nitro effects.

---

## M3 — Arcade Loop and Adaptive Music (≈2 weeks)

**Goal:** a complete 5-stage run where the music reacts to how you drive.

**Work:**

- Run state machine: countdown on the downbeat, a timer, checkpoints with time extension, Goal and Time Over.
- Scoring: multiplier, combo decay and the speed factor.
- HUD v1: score, multiplier, timer, distance, speedometer, gear, nitro, callouts.
- `music` conductor:
  - one complete biome theme (pattern banks for pad, bass, arp, lead and drums);
  - 5 layers with hysteresis;
  - quantised stingers;
  - speed-linked filter;
  - nitro riser, crash tape-stop and checkpoint fill.
- Key-locked near-miss accents.
- Beat-synced visual uniforms (grid, sun, bloom pulse).
- Pause menu with volume controls and mute.

**Acceptance criteria:**

- [ ] A full run from start to Goal or Time Over in 3–5 minutes, with correct score and time accounting (unit tests on scoring rules and timer).
- [ ] Music tests (native):
  - every stinger and layer change lands on its quantisation boundary (sample-accurate check);
  - layers never toggle within 2 bars (hysteresis);
  - at most 1 stinger per beat.
- [ ] Listening review: 3 recorded runs (cautious, aggressive, crash-heavy) sound musically coherent and clearly different from one another. Audio captures are checked in to `docs/reviews/`.
- [ ] Visual pulses are visibly locked to the beat (frame-capture review).

---

## M4 — Worlds (≈3 weeks)

**Goal:** four distinct, living biomes and a branching route.

**Work:**

- Biome parameter system (palette, LUT, fog, terrain, prop sets, music theme).
- **Coastal:** ocean grid with sun reflection, palms, pier.
- **Desert:** mesas, pylons, cacti.
- **Neon City:** instanced towers with procedural windows, signs, tunnels, wet road with fake reflection plus streaks, rain particles.
- **Mountain Pass:** steep hills, guardrails, horizon-curvature summit reveal.
- Forks: split geometry, signage, route pyramid with 16 routes.
- Biome transitions under the checkpoint gantry (LUT and fog crossfade, music key and theme change on the bar).
- LOD and impostors, culling with bend-padded bounds.
- One music theme per biome.

**Acceptance criteria:**

- [ ] Golden-image screenshots of each biome at 3 times along a stage, reviewed with the user for distinctiveness and quality.
- [ ] All 16 routes are drivable to completion (automated AI-driver run per route in native tests).
- [ ] Performance budgets are held in the heaviest biome (Neon City) on desktop Medium and mobile Low.

---

## M5 — Presentation (≈2 weeks)

**Goal:** arcade-cabinet polish around the game.

**Work:**

- Original logo and wordmark (SDF chrome with scanlines). The name needs user approval.
- Boot sequence with "PRESS START" (audio unlock).
- Title screen with attract mode (AI driver plus a high-score cycle).
- Main menu, settings (graphics, audio, controls with rebinding, units), car colour select.
- Results screen with breakdown and grades; 3-letter name entry; local high scores.
- Transitions: VHS wipes and a beat-synced menu UI.
- Menu SFX.
- Optional CRT, VHS and 8-BIT modes; reduced-flashing option.
- HUD final art.
- Text rendering: an anti-aliased vector stroke font (generated by us or Hershey-derived, public domain) or an OFL font atlas. Decided here.

**Acceptance criteria:**

- [ ] The whole flow is navigable by keyboard only, gamepad only, and touch only (E2E plus manual).
- [ ] Settings and high scores persist across reloads (E2E).
- [ ] With reduced flashing on, no strobe, VHS or datamosh effects appear (E2E screenshot check).
- [ ] User review of the title screen and attract mode.

---

## M6 — Polish, scaling and v1.0 (≈2 weeks)

**Work:**

- Automatic quality governor and tier tuning.
- Mobile optimisation pass.
- Daily Run (date seed) and Heart-Attack-style stage objectives.
- Accessibility pass.
- Cross-browser bug sweep (Chrome, Edge, Firefox, Safari on macOS and iOS, Android Chrome).
- Size and load-time optimisation.
- Credits and licence page.
- README with the play link.

**Acceptance criteria (release):**

- [ ] Performance:
  - desktop Medium: p95 16.7 ms or less;
  - mobile reference phone holds 60 fps on Low (or a stable 30 fps fallback);
  - first-load transfer 3.5 MB or less gzip;
  - time to first frame 4 s or less on fast 4G.
- [ ] No console errors across the browser matrix. WebGL2 forced mode is equal in content to WebGPU (aside from Ultra extras).
- [ ] A complete licence audit of every dependency (`cargo deny` licences) and every asset.
- [ ] User sign-off playtest.

---

## Testing strategy (all milestones)

1. **Automated, on every PR:**
   - `fmt`, `clippy -D warnings`;
   - native unit and property tests (`sim`, `procgen`, `music`, `dsp`);
   - naga shader validation for both backends;
   - wasm build with a size budget.
2. **Automated, on every deploy:** Playwright smoke test against the live Pages URL (MIME types, render, no errors, both backends).
3. **Golden images:** seeded scenes are rendered headless by the desktop runner. Diffs above tolerance fail the run, and accepting new goldens requires human review.
4. **Performance:** the `?bench` run (deterministic attract mode) reports p50/p95/p99 frame times, draw calls and tier. Results are recorded in `docs/perf/M<n>.md`, so regressions over 10% are flagged.
5. **Manual milestone review:** a feel, look and sound checklist covering desktop, Android and iOS, with screenshots and audio captures checked in under `docs/reviews/`.
6. **Honesty rule:** every review note lists known weaknesses and anything not verified. Features are never reported as finished without evidence.
