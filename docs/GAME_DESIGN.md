# SYNTHRACE — Game Design Document

> Status: Phase 0 draft for approval. Every number here is a **starting value to tune by feel**, not a fixed spec. Research behind these choices is in [RESEARCH.md](RESEARCH.md) §4–§5.

## 1. Concept

**Working title:** SYNTHRACE. This is a placeholder; the final name and logo are an approval item.

**One line:** You are racing the setting sun down an endless neon highway. Risky driving charges your nitro, nitro buys time, and the soundtrack builds with every near miss.

**Pillars**, in priority order. Every design decision is checked against these.

1. **Speed you can feel.** Speed should read clearly on screen through FOV, speed lines, radial blur, stripe frequency and the sound of the engine, without making driving any harder.
2. **Risk is the fuel.** Near misses, drifting, driving into oncoming traffic and overtakes all earn nitro and multiplier. Playing safe is viable, but slow and low-scoring.
3. **The music is the scoreboard.** The adaptive soundtrack reflects your current state: combo, nitro and danger.
4. **One more run.** Runs last 3–5 minutes, the player is driving within 5 seconds of pressing start, route forks give replay variety, and there is a daily seeded run.

**Inspiration:** the spirit of 1986-era checkpoint racing, meaning the clock as the opponent and branching routes, combined with Burnout-style risk rewards and Mario Kart-style drift charging. We borrow none of their names, characters, art, music or track layouts. All content is original and procedural.

## 2. Core loop

```
   ┌──────────── RISK ─────────────┐
   │ near-miss · oncoming · drift  │
   │ overtake                      │
   ▼                               │
 NITRO meter ──spend──► SPEED ──► more traffic/sec, more risk chances
   │                       │
   ▼                       ▼
 MULTIPLIER ───────► SCORE    CHECKPOINTS (+time)
   ▲                       │
   └── crashes reset it ◄──┘    the CLOCK is the brake on the loop
```

The clock and crashes are what stop the loop. A crash costs time (you lose speed and spend time recovering), resets the multiplier, and halves your nitro.

## 3. Game modes

| Mode | Description | Phase |
|---|---|---|
| **Arcade Run** (primary) | 5 stages, each ending in a checkpoint and a fork. This gives a route pyramid with 16 possible paths and 5 different goal scenes. Time extends at each checkpoint, and leftover time is banked as score at the goal. | M3–M5 |
| **Daily Run** | Arcade Run with a fixed date-based seed for routes and traffic, so everyone plays the same run that day. Has its own local high-score table. | M6 |
| **Free Drive / Attract** | No timer and endless procedural road. The attract mode on the title screen uses this with an AI driver. | M2 / M6 |
| **Objective layer** (optional) | One micro-objective per stage, such as "5 near misses in 15 s", "drift through 3 S-bends", or "stay in the oncoming lane for 4 s". Each is graded AAA–E for a bonus, and they double as a tutorial. | M6 |

There are no online leaderboards in v1, because GitHub Pages is static hosting. A future backend for them would be an approval item.

## 4. Driving model

The track is a sequence of segments, each with a `curve` and `hill` value. The player's state is `(track_distance, lateral_x, speed, yaw_visual, drift_state, rpm, gear)`. The road spans `x ∈ [-1, 1]`; anything beyond that is off-road. The simulation runs at a fixed 120 Hz and is deterministic. See TECHNICAL_ARCHITECTURE §4.

### 4.1 Speed

| Parameter | Start value |
|---|---|
| Top speed | 300 km/h, or 360 km/h with nitro |
| Acceleration | `a0·(1 − v/Vmax)^0.6`, with a0 ≈ 1.2 g, giving 0–200 km/h in about 6 s |
| Braking | 1.5 g |
| Coast drag | 0.15 g |
| Off-road | Decelerates toward 38% of Vmax at about 2 g; 50% steering penalty; rumble and a dust layer |

### 4.2 Steering

- Lateral rate is `steer · k0 · (0.4 + 0.6·(1 − v/Vmax))`, so steering is agile at low speed and stable at high speed.
- Steering input ramps in over 120 ms and recentres over 80 ms. This shaping is applied to digital keyboard and touch input; analog sticks get a deadzone and a response curve instead.
- **Centrifugal push:** `x -= curve · (v/Vmax)² · k_c · dt`, tuned so that a hard bend at top speed needs 70–80% steering to hold the line.

### 4.3 Drift (the skill layer)

- **Entry:** hold the drift button while steering above 120 km/h, or tap the brake (under 200 ms) while steering.
- **While drifting:** centrifugal push is multiplied by 0.45, so you can hold a tighter line. Speed decays at 0.1 g, the visual yaw eases to 20–35°, and countersteering widens the line.
- **Charge tiers**, shown as tyre sparks changing colour from cyan to magenta to white:

| Tier | Hold time | Boost on release |
|---|---|---|
| 1 | 0.8 s | +8% for 0.6 s |
| 2 | 1.8 s | +12% for 1.2 s |
| 3 | 3.0 s | +15% for 1.8 s |

  The charge rate doubles when the drift angle is in the top third of its range. This rewards committing to the drift.
- **Exit:** release drift, or centre the steering for more than 150 ms. Yaw snaps back over 200 ms.

### 4.4 Gears and RPM

Six automatic gears with taller spacing at the top: 3.2 / 2.1 / 1.5 / 1.15 / 0.92 / 0.78. Upshifts happen at 95% of redline, with a 120 ms torque cut. The engine synth follows RPM directly. An optional manual mode gives +3% acceleration as a skill reward.

### 4.5 Collisions

These are designed to be readable and fair, never to cause instant death.

| Case | Result |
|---|---|
| Rear contact with traffic at Δv under 40 km/h | Bounce: −30% speed, lateral push, 0.5 s of steering wobble |
| Head-on, or high Δv | Spin-out: 1.5–2.0 s without control, then auto-relaunch from about 0 |
| Roadside object, glancing (angle under 25°) | Scrape: sparks and −15% speed |
| Roadside object, square hit | Crash: about 2.5 s of time lost. Triggers the datamosh/VHS effect and a music tape-stop. |

## 5. Traffic

- **Lane-based rows** of traffic, each guaranteed to have at least one gap. Rows are spaced by time headway (at least 0.6 s at the player's speed).
- **No spawning inside the reaction window.** Traffic is never spawned where time to impact is under 1.2 s.
- **Lane changes are telegraphed** by 1 s of blinking indicators, rendered as emissive strips that bloom.
- **Vehicle archetypes** come from one procedural low-poly generator, with different speeds and silhouettes: commuter coupé, van, lorry (wide and slow), sports car (fast, changes lanes), and oncoming traffic in the opposite lanes on two-way stages.
- **Density and behaviour** ramp per stage, not randomly. The harder branch at a fork means denser, faster traffic.

## 6. Nitro, multiplier and scoring

### 6.1 Nitro meter

The meter runs 0–100 in 3 segments and can be spent at any time.

| Source | Gain |
|---|---|
| Near miss: lateral gap under 0.35 car-widths at Δv above 30 km/h | +6 |
| "CLOSE!" near miss: gap under 0.15 | +10 |
| Oncoming lane above 150 km/h | +4 per second |
| Drifting | +3 per second, plus 5 / 8 / 12 on release (tier 1/2/3) |
| Clean overtake (no contact for 1 s either side) | +2 |

**Spending nitro:** it drains 25 per second and gives +20% top speed and +25% acceleration. Near misses during nitro earn 1.5× ("boost chaining").

### 6.2 Combo multiplier

- Goes up one step every 3 risk events: x1 → x2 → x3 → x4 → x6 → x8.
- Drops one step after 2.5 s without a risk event.
- A crash resets it to x1.

### 6.3 Points

Every event is worth its base points × multiplier × speed factor, where the speed factor is `0.5 + v/Vmax`.

| Event | Base points |
|---|---|
| Near miss | 100 |
| CLOSE! | 250 |
| Oncoming | 50 per second |
| Drift | 40 per second, plus 200 / 400 / 800 tier bonus |
| Overtake | 50 |
| Distance | 1 per 10 m |
| Goal | remaining seconds × 1000 |

### 6.4 Time and checkpoints

- The run starts with 60 s on the clock.
- Each checkpoint adds 20–30 s, tuned per stage. The amount is announced with an "EXTENDED PLAY" stinger, and remaining time carries over.
- A stage takes about 55–70 s of driving, so a run lasts 3–5 minutes.
- When time runs out, the car coasts to a stop, the screen goes to a VHS "signal lost" effect, and the results screen appears.

### 6.5 Difficulty progression

Stage 1 is gentle: sparse traffic and wide bends, effectively a tutorial of the core loop. Each stage adds traffic density, more lane-changing traffic, tighter curve sequences, bigger hills that hide what's ahead, and oncoming lanes. The right-hand fork is always the harder branch and carries a ×1.2 stage multiplier. There is **no rubber-banding**, because the clock is the only opponent.

## 7. Environments

There are four biomes. Each is a parameter set fed to the same procedural systems, not a separate engine. Every stage of a run is one biome, and the transitions happen under a checkpoint gantry with a colour-grade crossfade.

| Biome | Palette | Terrain | Scenery | Signature moment |
|---|---|---|---|---|
| **Coastal Highway** | Peach, coral and magenta sky; teal sea | Flat coast on one side, the sea on the other: an animated grid ocean with reflected sun bands | Palm rows, beach lamps, lifeguard towers, a neon pier | The sun sitting on the sea horizon, with its bands reflected in the water |
| **Desert Run** | Deep orange to violet; cyan grid | Ridged-FBM wireframe mesas and a distant mountain layer | Cacti silhouettes, billboards, power pylons with glowing insulators | A huge sun between two mesas, with the road dropping into a valley |
| **Neon City** | Black-blue with hot pink and cyan accents | Flat, with overpasses and tunnels | Instanced towers with procedural windows, neon signs, wet road | Wet-road reflections of the whole skyline, and a tunnel lit by strobing lights |
| **Mountain Pass** | Gold to purple dusk; snow tinted pink | Steep hills, switchbacks and cliffs | Guardrails with reflectors, pines, an observatory dome | Cresting a summit with the whole valley and sunset below; the music drops to the pad layer for one bar |

Every biome also has animated life: birds or bats, blinking aircraft lights, scrolling billboards, distant fireworks, lightning on the horizon, and beat-synced pulses on the grid and neon.

## 8. Visual identity

- **Look:** "Neon Bent-World." Real 3D geometry is bent in the vertex shader so the horizon curves away and the sun stays fixed at the vanishing point, as in classic pseudo-3D. Everything emissive is HDR and blooms.
- **Signature elements:**
  - an analytic banded sun;
  - anti-aliased wireframe grid terrain;
  - a fog colour sampled from the sky, so distant geometry melts into the horizon glow;
  - neon light streaks reflected on wet roads;
  - tail-light streaks;
  - beat-synced pulses.
- **Palette:** magenta `#FF2E97`, hot pink `#FF6EC7`, violet `#7B2FF7`, cyan `#00F0FF`, electric blue `#2D6BFF`, sun gold `#FFC94D`, night `#0B0221`. The final colour grading is done with a LUT.
- **Cars:** procedural low-poly bodies with HDR tail-light strips, Fresnel rim light keyed to sky colours, and underglow on the player car.
- **Optional modes** in Settings: CRT (scanlines and curvature), VHS, and "8-BIT" (dithered palette at lower resolution, which doubles as a performance mode).
- **Typography:** an original geometric/chrome title logo rendered in-engine (an SDF wordmark with a chrome gradient and scanlines). The HUD uses an open-licence display font (OFL), to be confirmed in M4.

## 9. Audio experience

All audio is synthesised in real time. There are no samples and no licensed music.

- **Soundtrack:**
  - Minor keys at 112–124 BPM, using progressions such as i–VI–III–VII and i–VI–VII, with sus2/add9 colour.
  - Each biome has its own key, tempo and lead patch.
  - Song sections follow a verse/chorus loop with a fill at each checkpoint.
- **Adaptive layers**, with hysteresis and a minimum of 2 bars on:

| Layer | Content | On when |
|---|---|---|
| L0 | Pads and bass | Always |
| L1 | Drums | Speed above 40% |
| L2 | Arpeggio | Multiplier x2 or higher |
| L3 | Lead | Nitro, or multiplier x4 or higher |
| L4 | "Hero" variant | Final 10 s, or the final stage |

- **Gameplay hooks.** All are quantised to the beat or bar and limited to one stinger at a time, by priority.
  - **Speed:** a master low-pass filter opens with speed, from about 800 Hz to fully open.
  - **Overtake:** an accent hit on the next 1/8 note.
  - **Near miss:** key-locked arpeggio blips whose pitch climbs with the combo.
  - **Nitro:** a 1–2 bar riser that lands the lead layer on the downbeat.
  - **Crash:** a tape-stop over 300–500 ms, then the music re-enters on the next bar.
  - **Checkpoint:** a fill plus an "EXTENDED PLAY" stinger.
- **SFX:**
  - Synthesised engine, driven by RPM, with throttle and off-throttle layers and backfire pops on shifts.
  - Tyre squeal that follows slip.
  - Doppler pass-by for up to 4 nearby traffic voices.
  - A positioned near-miss whoosh.
  - A nitro ignition roar.
  - Crunchy collision impacts.
  - Synth blips for menus.
- **Mixer:** Master, Music and SFX volumes, plus a mute toggle (M). Audio starts only after an explicit "PRESS START" interaction, which is when the browser allows sound to play.

## 10. User interface

### 10.1 Flow

```
Boot (logo build-in, "PRESS START" = audio unlock)
 → Title / Attract (AI driving behind the logo; high-score table cycles)
 → Main menu: ARCADE RUN · DAILY RUN · FREE DRIVE · SETTINGS · HIGH SCORES
 → Car/colour select (cosmetic) → Countdown 3-2-1 synced to the music downbeat
 → Race ⇄ Pause (Resume · Restart · Settings · Quit)
 → Fork choice (steer left/right at the split; signage shows biome + difficulty)
 → Goal / Time Over → Results (score breakdown, grades) → Name entry (3 letters) → High scores
```

### 10.2 HUD (in game)

The HUD is rendered in-engine so it gets bloom, and its layout adapts to a safe area.

- **Top left:** score and multiplier pill (x1–x8, which pulses when it goes up).
- **Top centre:** a large countdown timer that turns red under 10 s; stage number and route map (5-step pyramid with the current position).
- **Top right:** distance travelled and the best-score delta.
- **Bottom right:** an arc speedometer with a digital readout in km/h or mph, a gear indicator, and an RPM bar.
- **Bottom left:** a 3-segment nitro meter, plus a drift charge ring when drifting.
- **Centre:** event callouts ("CLOSE!", "x4", "OVERTAKE", "EXTENDED PLAY +25"). At most 2 show at once, and they animate on the beat.

### 10.3 Settings

| Group | Options |
|---|---|
| Graphics | Quality preset (Low/Med/High/Ultra/Auto), render scale, CRT/VHS/8-BIT toggles, motion blur, screen shake, FOV kick, reduced flashing |
| Audio | Master, music and SFX volume; mute |
| Controls | Rebind keys, steering sensitivity, gamepad deadzone, touch layout (buttons or tilt), auto/manual gears |
| Units | km/h or mph |

Settings and high scores are saved in `localStorage`.

### 10.4 Controls

| Action | Keyboard | Gamepad (standard mapping) | Touch |
|---|---|---|---|
| Steer | ←/→ or A/D | Left stick / D-pad | Left and right halves of the screen, or optional tilt |
| Accelerate | ↑/W | RT (analog) | Auto-accelerate by default; brake button |
| Brake | ↓/S | LT (analog) | Brake button (bottom left) |
| Drift | Shift/Space | A / RB | Drift button (bottom right) |
| Nitro | N/Ctrl | X / LB | Nitro button |
| Pause | Esc/P | Start | Pause icon |

### 10.5 Accessibility

- A "reduced flashing" setting turns off beat strobes, VHS effects and datamosh.
- Screen shake has an intensity slider.
- Colour-blind-safe HUD state indicators use shape as well as colour.
- An optional assist keeps the car on the road (stops it leaving the road); scores earned with it are flagged.

## 11. Scope guardrails

**In v1:**
- one player car model with colour variants;
- 4 biomes;
- 5-stage Arcade Run with 16 routes, Daily Run and Free Drive;
- local high scores;
- desktop and mobile support.

**Out of v1:**
- multiplayer and online leaderboards;
- car upgrades or progression economy;
- a track editor;
- licensed anything.
