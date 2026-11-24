//! Traffic: lane-following AI cars with telegraphed lane changes and a fairness-checked
//! row spawner (GAME_DESIGN §5). All positions in track space.

use crate::rng::Pcg32;
use crate::track::Track;

/// Lane centres (m): four same-direction lanes on the 18 m road.
pub const LANES: [f32; 4] = [-6.75, -2.25, 2.25, 6.75];
pub const MAX_CARS: usize = 48;
/// Rows spawn this far ahead of the player (m) — far outside any reaction window.
pub const SPAWN_AHEAD: f32 = 900.0;
const DESPAWN_BEHIND: f32 = -220.0;
const DESPAWN_AHEAD: f32 = 1400.0;
/// Seconds of blinking indicator before a lane change starts, and the change duration.
pub const INDICATOR_TIME: f32 = 1.0;
const CHANGE_TIME: f32 = 1.6;
/// A lane change is never started inside this window ahead of the player (m).
const NO_CUT_IN_AHEAD: (f32, f32) = (-15.0, 160.0);
/// Minimum time-to-impact (s) for any new spawn.
pub const MIN_SPAWN_TTI: f32 = 1.2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Archetype {
    Coupe,
    Van,
    Truck,
    Sports,
}

impl Archetype {
    pub const ALL: [Archetype; 4] = [Archetype::Coupe, Archetype::Van, Archetype::Truck, Archetype::Sports];

    /// (half width, half length) in metres.
    pub fn half_extents(self) -> (f32, f32) {
        match self {
            Archetype::Coupe => (0.92, 2.2),
            Archetype::Van => (1.02, 2.5),
            Archetype::Truck => (1.25, 4.6),
            Archetype::Sports => (0.95, 2.2),
        }
    }

    /// Cruise speed range (m/s).
    fn speed_range(self) -> (f32, f32) {
        match self {
            Archetype::Coupe => (26.0, 36.0),
            Archetype::Van => (22.0, 28.0),
            Archetype::Truck => (19.0, 25.0),
            Archetype::Sports => (36.0, 47.0),
        }
    }

    /// Lane changes started per second (when allowed).
    fn change_rate(self) -> f32 {
        match self {
            Archetype::Coupe => 0.05,
            Archetype::Van => 0.025,
            Archetype::Truck => 0.012,
            Archetype::Sports => 0.16,
        }
    }

    pub fn index(self) -> usize {
        self as usize
    }
}

#[derive(Clone, Debug)]
pub struct TrafficCar {
    pub id: u32,
    pub kind: Archetype,
    pub s: f32,
    pub x: f32,
    pub v: f32,
    pub cruise: f32,
    pub lane: u8,
    pub target: u8,
    /// < 0: no lane change; 0..INDICATOR_TIME: indicating; then moving until done.
    pub change_t: f32,
    /// Brake-light intensity 0..1.
    pub brake: f32,
    /// Signed distance ahead of the player at the previous step (for pass detection).
    pub prev_ds: f32,
    pub passed: bool,
    pub touched: bool,
}

impl TrafficCar {
    /// −1 / +1 while indicating or changing toward that side; 0 otherwise.
    pub fn indicator(&self) -> f32 {
        if self.change_t >= 0.0 { (self.target as f32 - self.lane as f32).signum() } else { 0.0 }
    }

    fn occupies(&self, lane: u8) -> bool {
        self.lane == lane || (self.change_t >= 0.0 && self.target == lane)
    }
}

/// Record of a row spawn, kept for fairness tests and debugging.
#[derive(Clone, Copy, Debug)]
pub struct RowSpawn {
    pub occupied: [bool; 4],
    pub distance_ahead: f32,
    pub min_tti: f32,
}

pub struct Traffic {
    pub cars: Vec<TrafficCar>,
    rng: Pcg32,
    next_id: u32,
    travel_since_row: f32,
    next_gap: f32,
    /// Mean player travel (m) between spawned rows; lower = denser.
    pub row_gap: f32,
    pub last_spawn: Option<RowSpawn>,
}

fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl Traffic {
    pub fn new(seed: u64) -> Self {
        Self {
            cars: Vec::with_capacity(MAX_CARS),
            rng: Pcg32::new(seed, 0x7af1c),
            next_id: 1,
            travel_since_row: 0.0,
            next_gap: 0.0,
            row_gap: 75.0,
            last_spawn: None,
        }
    }

    /// Fill the road ahead at the start of a run.
    pub fn populate(&mut self, track: &Track, player_s: f32, player_v: f32) {
        let mut d = 180.0;
        while d < SPAWN_AHEAD {
            self.spawn_row(track, player_s, player_v, d);
            d += self.row_gap * self.rng.range(0.7, 1.3);
        }
        self.next_gap = self.row_gap;
    }

    pub fn update(&mut self, track: &Track, player_s: f32, player_v: f32, dt: f32) {
        // Spawn rows as the player covers ground.
        self.travel_since_row += player_v * dt;
        if self.travel_since_row >= self.next_gap {
            self.travel_since_row = 0.0;
            self.next_gap = self.row_gap * self.rng.range(0.6, 1.4);
            self.spawn_row(track, player_s, player_v, SPAWN_AHEAD);
        }

        // Despawn far behind / far ahead.
        self.cars.retain(|c| {
            let d = track.delta(c.s, player_s);
            (DESPAWN_BEHIND..DESPAWN_AHEAD).contains(&d)
        });

        let n = self.cars.len();
        for i in 0..n {
            // Car-following: match the speed of a slower car close ahead in our lane(s).
            let me = self.cars[i].clone();
            let (_, my_hl) = me.kind.half_extents();
            let mut limit = me.cruise;
            // Anti-wall: never be the car that completes a four-abreast row.
            let mut beside = [false; 4];
            let mut beside_min_v = f32::MAX;
            for (j, other) in self.cars.iter().enumerate() {
                if i == j {
                    continue;
                }
                let gap = track.delta(other.s, me.s);
                let shares = other.occupies(me.lane) || (me.change_t >= 0.0 && other.occupies(me.target));
                if shares && gap > 0.0 && gap < 45.0 {
                    let want = other.v - (30.0 - gap).max(0.0) * 0.3;
                    limit = limit.min(want.max(0.0));
                }
                let (_, hl) = other.kind.half_extents();
                if gap > -(my_hl + hl + 1.0) && gap < my_hl + hl + 30.0 && !shares {
                    beside[other.lane as usize] = true;
                    if other.change_t >= 0.0 {
                        beside[other.target as usize] = true;
                    }
                    beside_min_v = beside_min_v.min(other.v);
                }
            }
            beside[me.lane as usize] = false;
            let walling = beside.iter().filter(|b| **b).count() >= 3;
            if walling {
                limit = limit.min(beside_min_v - 3.0);
            }
            let c = &mut self.cars[i];
            let target_v = limit;
            let dv = target_v - c.v;
            let rate = if dv < 0.0 { if walling { 12.0 } else { 6.0 } } else { 2.5 };
            c.v += dv.clamp(-rate * dt, rate * dt);
            c.brake = if dv < -0.5 { 1.0 } else { (c.brake - dt * 3.0).max(0.0) };
            c.s = track.wrap(c.s + c.v * dt);

            // Lane-change progress.
            if c.change_t >= 0.0 {
                c.change_t += dt;
                let t = ((c.change_t - INDICATOR_TIME) / CHANGE_TIME).clamp(0.0, 1.0);
                c.x = LANES[c.lane as usize] + (LANES[c.target as usize] - LANES[c.lane as usize]) * smooth(t);
                if c.change_t >= INDICATOR_TIME + CHANGE_TIME {
                    c.lane = c.target;
                    c.x = LANES[c.lane as usize];
                    c.change_t = -1.0;
                }
            }
        }

        // Start new lane changes (fairness: never right in front of the player).
        for i in 0..n {
            let (lane, change_t, kind, s) = {
                let c = &self.cars[i];
                (c.lane, c.change_t, c.kind, c.s)
            };
            if change_t >= 0.0 || self.rng.f32() >= kind.change_rate() * dt {
                continue;
            }
            let ahead_of_player = track.delta(s, player_s);
            if (NO_CUT_IN_AHEAD.0..NO_CUT_IN_AHEAD.1).contains(&ahead_of_player) {
                continue;
            }
            // Short-circuit keeps the RNG untouched for edge lanes (determinism).
            let dir: i32 = if lane == 0 {
                1
            } else if lane == 3 || self.rng.f32() < 0.5 {
                -1
            } else {
                1
            };
            let target = (lane as i32 + dir) as u8;
            let clear =
                self.cars.iter().enumerate().all(|(j, o)| j == i || !o.occupies(target) || track.delta(o.s, s).abs() > 35.0);
            if clear && self.keeps_a_free_lane(track, s, i, target) {
                let c = &mut self.cars[i];
                c.target = target;
                c.change_t = 0.0;
            }
        }
    }

    /// True if, after car `idx` also occupies `target`, some lane near `s` stays free.
    fn keeps_a_free_lane(&self, track: &Track, s: f32, idx: usize, target: u8) -> bool {
        let mut occ = [false; 4];
        for (j, o) in self.cars.iter().enumerate() {
            if track.delta(o.s, s).abs() < 30.0 {
                occ[o.lane as usize] = true;
                if o.change_t >= 0.0 {
                    occ[o.target as usize] = true;
                }
            }
            if j == idx {
                occ[target as usize] = true;
            }
        }
        occ.iter().any(|o| !o)
    }

    fn spawn_row(&mut self, track: &Track, player_s: f32, player_v: f32, ahead: f32) {
        let row_s = track.wrap(player_s + ahead);
        // Lanes already occupied near the row position.
        let mut occupied = [false; 4];
        for c in &self.cars {
            if track.delta(c.s, row_s).abs() < 30.0 {
                occupied[c.lane as usize] = true;
                occupied[c.target as usize] |= c.change_t >= 0.0;
            }
        }
        let free: Vec<u8> = (0..4u8).filter(|&l| !occupied[l as usize]).collect();
        if free.len() <= 1 || self.cars.len() >= MAX_CARS {
            return; // never close the last gap
        }
        let want = 1 + (self.rng.f32() * 2.2) as usize; // 1..3 cars
        let count = want.min(free.len() - 1);
        let mut lanes = free;
        let mut min_tti = f32::MAX;
        for _ in 0..count {
            let k = self.rng.int(0, lanes.len() as u32 - 1) as usize;
            let lane = lanes.swap_remove(k);
            let kind = match self.rng.int(0, 9) {
                0..=4 => Archetype::Coupe,
                5..=6 => Archetype::Van,
                7 => Archetype::Truck,
                _ => Archetype::Sports,
            };
            let (lo, hi) = kind.speed_range();
            let v = self.rng.range(lo, hi);
            let s = track.wrap(row_s + self.rng.range(-6.0, 6.0));
            let closing = (player_v - v).max(1e-3);
            min_tti = min_tti.min(track.delta(s, player_s) / closing);
            occupied[lane as usize] = true;
            self.cars.push(TrafficCar {
                id: self.next_id,
                kind,
                s,
                x: LANES[lane as usize],
                v,
                cruise: v,
                lane,
                target: lane,
                change_t: -1.0,
                brake: 0.0,
                prev_ds: track.delta(s, player_s),
                passed: false,
                touched: false,
            });
            self.next_id += 1;
        }
        self.last_spawn = Some(RowSpawn { occupied, distance_ahead: ahead, min_tti });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawner_is_fair_over_10k_seeds() {
        let track = Track::straight(4000);
        for seed in 0..10_000u64 {
            let mut t = Traffic::new(seed);
            let mut player_s = 0.0;
            let player_v = 20.0 + (seed % 7) as f32 * 10.0;
            t.populate(&track, player_s, player_v);
            for _ in 0..240 {
                let dt = 1.0 / 30.0;
                t.update(&track, player_s, player_v, dt);
                player_s = track.wrap(player_s + player_v * dt);
                if let Some(r) = t.last_spawn.take() {
                    assert!(r.occupied.iter().any(|o| !o), "seed {seed}: row closed every lane");
                    assert!(r.distance_ahead >= 180.0);
                    assert!(r.min_tti > MIN_SPAWN_TTI, "seed {seed}: spawn inside reaction window (tti {})", r.min_tti);
                }
            }
            assert!(t.cars.len() <= MAX_CARS);
        }
    }

    /// Player car width plus margin: the smallest gap that counts as passable.
    const PASSABLE_GAP: f32 = 2.5;

    #[test]
    fn no_wall_of_cars_ever_forms_ahead() {
        // Simulate long runs and check every 10 m slice ahead keeps a free lane.
        let track = Track::straight(6000);
        for seed in 0..200u64 {
            let mut t = Traffic::new(seed);
            t.row_gap = 50.0; // stress: very dense
            let mut player_s = 0.0;
            let v = 75.0;
            t.populate(&track, player_s, v);
            for step in 0..3000 {
                t.update(&track, player_s, v, 1.0 / 60.0);
                player_s = track.wrap(player_s + v / 60.0);
                if step % 30 == 0 {
                    for slice in 0..80 {
                        let s = track.wrap(player_s + slice as f32 * 10.0);
                        // Lateral intervals physically covered by cars overlapping this slice.
                        let mut blocks: Vec<(f32, f32)> = t
                            .cars
                            .iter()
                            .filter(|c| track.delta(c.s, s).abs() < c.kind.half_extents().1 + 1.0)
                            .map(|c| (c.x - c.kind.half_extents().0, c.x + c.kind.half_extents().0))
                            .collect();
                        blocks.sort_by(|a, b| a.0.total_cmp(&b.0));
                        let mut edge = -crate::ROAD_HALF_WIDTH;
                        let mut widest = 0.0f32;
                        for (lo, hi) in &blocks {
                            widest = widest.max(lo - edge);
                            edge = edge.max(*hi);
                        }
                        widest = widest.max(crate::ROAD_HALF_WIDTH - edge);
                        if widest < PASSABLE_GAP {
                            let near: Vec<String> = t
                                .cars
                                .iter()
                                .filter(|c| track.delta(c.s, s).abs() < 40.0)
                                .map(|c| {
                                    format!("{:?} x{:.2} t{:.2} d{:.1} v{:.1}", c.kind, c.x, c.change_t, track.delta(c.s, s), c.v)
                                })
                                .collect();
                            panic!(
                                "seed {seed}: no {PASSABLE_GAP} m gap {slice}0 m ahead (step {step}):
{}",
                                near.join(
                                    "
"
                                )
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn lane_changes_telegraph_for_a_second() {
        let track = Track::straight(4000);
        let mut t = Traffic::new(3);
        t.cars.push(TrafficCar {
            id: 1,
            kind: Archetype::Sports,
            s: 600.0,
            x: LANES[1],
            v: 30.0,
            cruise: 30.0,
            lane: 1,
            target: 2,
            change_t: 0.0,
            brake: 0.0,
            prev_ds: 600.0,
            passed: false,
            touched: false,
        });
        for _ in 0..(0.9 * 60.0) as usize {
            t.update(&track, 0.0, 0.0, 1.0 / 60.0);
        }
        assert_eq!(t.cars[0].x, LANES[1], "must not move while indicating");
        assert_eq!(t.cars[0].indicator(), 1.0);
        for _ in 0..(2.0 * 60.0) as usize {
            t.update(&track, 0.0, 0.0, 1.0 / 60.0);
        }
        assert_eq!(t.cars[0].lane, 2);
        assert_eq!(t.cars[0].indicator(), 0.0);
    }
}
