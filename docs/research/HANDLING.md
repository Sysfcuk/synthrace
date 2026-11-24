# Handling research: why SYNTHRACE glides, and the fix

Written 2026-11-24, in response to playtest feedback: "it glides and doesn't act like a car; other cars seem immovable."

## Diagnosis of the M1/M2 model

Up to M2, `sim::car` moved the car laterally *directly*: `x += steer × rate × grip`.

- **No heading.** The car "strafes" sideways like a hovercraft. Releasing the key stops lateral motion instantly, so steering has no momentum to carry.
- **No yaw inertia.** Turn-in and recovery happen instantly. Nothing has to "settle".
- **No grip limit.** Cornering costs no speed. The car doesn't understeer when pushed, and doesn't slide unless a drift is triggered by script.
- **Traffic were kinematic walls.** The player bounced off them, but they never moved.

## What other games and articles do

1. **Heading and velocity are separate states.** The car points one way, its velocity points (slightly) another way, and tyre grip pulls the velocity toward the heading. The angle between them is the slip (or "grip") angle, the central quantity in every handling model.
   - Brian Beckman, *The Physics of Racing, Part 10: Grip Angle* ([gamedev.net](https://gamedev.net/tutorials/programming/math-and-physics/the-physics-of-racing-part-10-grip-angle-r1619)).
2. **Treat longitudinal and lateral forces separately.** Steering generates *lateral* tyre force, which turns the car. This is the classic simplification for games.
   - Marco Monster, *Car Physics for Games* ([mirror](https://asawicki.info/Mirror/Car%20Physics%20for%20Games/Car%20Physics%20for%20Games.html)).
3. **The bicycle model.** Collapse each axle into a single wheel. The yaw rate comes from speed, steering angle and wheelbase: `r = v·tan(δ)/L`. Above the grip limit, the tyres saturate.
   - [MathWorks: Bicycle Model](https://it.mathworks.com/help/driving/ref/bicyclemodel.html)
   - [UCSD CSE291 vehicle dynamics](https://cseweb.ucsd.edu/classes/sp19/cse291-d/Files/CSE291_14_VehicleDynamics.pdf)
4. **Speed-sensitive steering.** "Almost every racing game improves car handling by limiting the front wheels' maximum steering angle based on how fast the car is travelling." Full stick at 300 km/h asks for the grip limit, not a hairpin.
   - Game Developer, *Implementing racing games* ([article](https://gamedeveloper.com/design/implementing-racing-games-an-intro-to-different-approaches-and-their-game-design-trade-offs)).
5. **Arcade means grounded, then simplified.** Start from physics and remove what doesn't serve the feel. For "grippy arcade", raise the lateral friction peak. For drift-happy cars, lower *rear* lateral grip.
   - [gamedev.net arcade car physics thread](https://gamedev.net/forums/topic/699625-arcade-car-physics/5394776/)
   - [Sergey Makeev, arcadecarphysics](https://github.com/sergeymakeev/arcadecarphysics)
6. **Drift is reduced rear grip.** The velocity lags behind a heading that has rotated into the corner. Countersteer controls the angle.
   - [gamedev.net arcade drift thread](https://ehe.gamedev.net/forums/topic/712162-how-do-i-implement-arcade-drift-using-a-physics-engine)
7. **Weight transfer sells mass.**
   - Body roll on passenger cars is roughly 3–7°/g ([Vehicle Dynamics for Racing Games, PDF](https://storage.googleapis.com/google-code-archive-downloads/v2/code.google.com/transporter-game/VehicleDynamicsForRacingGames.pdf)).
   - Braking dives the nose and acceleration squats the rear ([Beckman, Part 1: Weight Transfer](https://gamedev.net/tutorials/programming/math-and-physics/the-physics-of-racing-part-1-weight-transfer-r1610)).
   - The camera's rotation and response to movement "greatly influences the feel of the controls" ([itch.io devlog](https://itch.io/devlog/788894/improving-the-camera.amp)).
8. **Traffic as a weapon (Burnout).**
   - Burnout 3 rewards ramming rivals into traffic and walls ("takedowns"). Its designers said crashes "are not the worst thing in the world" and tuned the physics for spectacle.
   - Burnout Revenge added *traffic checking*: ramming small and medium same-way traffic launches it forward instead of crashing you, while heavy vehicles still stop you.
   - Sources: [Burnout series](https://Www.wikipedia.org/wiki/Burnout_(series)), [Burnout Revenge](https://Www.wikipedia.org/wiki/Burnout_Revenge), [GameSpot designer diary](https://www.gamespot.com/articles/burnout-revenge-designer-diary-2/1100-6132331/).

## Design adopted for SYNTHRACE (M2.5)

The new model is a velocity-alignment (bicycle-lite) model in track space.

| State | Meaning |
|---|---|
| `heading` ψ | Where the body points, relative to the road tangent |
| `travel` θ | Direction of the velocity, relative to the road tangent |
| `v` | Speed along `travel` |
| `yaw_rate` r | Turn rate of the body, world frame |

**Each step:**

1. **Steering demands a yaw rate,** capped by geometry at low speed and by grip at high speed:
   `r_cap(v) = min(v·tan(δmax)/L, μg/v)`, and `r_target = steer · r_cap`.
   Full steer at top speed therefore *means* "use all the grip".
2. **Yaw inertia:** `r` eases toward `r_target` at about 9/s, so turn-in and unwind take a moment.
3. **The road curves under you:** `ψ += (r − κ·v)·dt`, where κ is the road's curvature. Not steering on a bend means your heading relative to the road drifts outward. This replaces the old scripted "centrifugal push".
4. **Grip pulls the velocity toward the heading:** the world turn rate of the velocity is `clamp(align·(ψ − θ), ±μg/v)`.
   - Normal driving: `align` is high (planted).
   - Drift: low rear grip makes `align` small, so the car slides.
   - Off-road: μ drops.
   - Stunned: almost no grip.
5. **Motion:** `x += v·sin θ·dt`, `s += v·cos θ·dt`. Lateral motion now carries momentum.
6. **Scrub:** cornering load and sliding cost speed (`|lat accel|/μg` and `|sin(ψ−θ)|` terms).
7. **Assists** (arcade, tunable):
   - a small road-follow assist (15% of road curvature);
   - mild heading recentre when you're not steering and the slip is small, so digital keyboard steering stays drivable.

**Collisions become momentum exchanges.**
- Masses: player 1300 kg, with an arcade "effective" 2600 kg against light traffic; coupe 1300, sports 1250, van 2100, truck 9000 kg.
- A restitution impulse is applied along the axis of least penetration.
- **Light traffic can be checked:** a rear-end launches them forward, and a side slam sends them sliding off the road.
- **Trucks stop you.** Rear-ending one hard spins you out.
- **Wrecked traffic** loses its AI, slides with friction and spin, smashes into palms and other cars (chain reactions), and stays behind as debris.
- **Takedown:** a player-caused wreck that leaves the road, hits scenery or hits other traffic. Pays nitro (+15) and a TAKEDOWN callout.

**Visual weight** (render only):
- body roll of about 4°/g of lateral acceleration;
- pitch squat under acceleration and dive under braking;
- the body yaws to the real heading (so the drift angle is visible);
- the camera yaws partially with the car's heading and lags laterally.

**Determinism:** heading needs sin, cos and atan2. These come from `sim::dmath`, deterministic polynomial approximations using IEEE-exact operations only, so the golden hash still holds across native and wasm.
