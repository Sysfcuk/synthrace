//! Deterministic trig for the simulation: polynomial approximations built only from IEEE-exact
//! operations (+ − × ÷), so native and wasm produce bit-identical results (libm's `sin`/`atan2`
//! are not guaranteed to agree across platforms).

use core::f32::consts::{FRAC_PI_2, PI, TAU};

/// Wraps an angle to (−π, π].
pub fn wrap_angle(a: f32) -> f32 {
    let mut a = a % TAU;
    if a > PI {
        a -= TAU;
    } else if a <= -PI {
        a += TAU;
    }
    a
}

/// sin(x), max error ≈ 2e-6 on any finite input.
pub fn sin(x: f32) -> f32 {
    let mut a = wrap_angle(x);
    // Reflect into [−π/2, π/2]: sin(π − a) = sin(a).
    if a > FRAC_PI_2 {
        a = PI - a;
    } else if a < -FRAC_PI_2 {
        a = -PI - a;
    }
    let a2 = a * a;
    // Taylor to x^11 (accurate to ~6e-8 on [−π/2, π/2]).
    a * (1.0
        + a2 * (-1.0 / 6.0 + a2 * (1.0 / 120.0 + a2 * (-1.0 / 5040.0 + a2 * (1.0 / 362_880.0 + a2 * (-1.0 / 39_916_800.0))))))
}

pub fn cos(x: f32) -> f32 {
    sin(x + FRAC_PI_2)
}

/// atan(x) for any x, max error ≈ 1e-5 rad.
pub fn atan(x: f32) -> f32 {
    let (ax, flip) = if x.abs() > 1.0 { (1.0 / x.abs(), true) } else { (x.abs(), false) };
    // Range-reduce further with atan(x) = π/6 + atan((x·√3 − 1)/(x + √3)) for x > tan(π/12).
    const T15: f32 = 0.267_949_2;
    const SQRT3: f32 = 1.732_050_8;
    let (y, offset) = if ax > T15 { ((ax * SQRT3 - 1.0) / (ax + SQRT3), PI / 6.0) } else { (ax, 0.0) };
    let y2 = y * y;
    let r = y * (1.0 + y2 * (-1.0 / 3.0 + y2 * (1.0 / 5.0 + y2 * (-1.0 / 7.0 + y2 * (1.0 / 9.0))))) + offset;
    let r = if flip { FRAC_PI_2 - r } else { r };
    if x < 0.0 { -r } else { r }
}

/// atan2(y, x) in (−π, π].
pub fn atan2(y: f32, x: f32) -> f32 {
    if x > 0.0 {
        atan(y / x)
    } else if x < 0.0 {
        if y >= 0.0 { atan(y / x) + PI } else { atan(y / x) - PI }
    } else if y > 0.0 {
        FRAC_PI_2
    } else if y < 0.0 {
        -FRAC_PI_2
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_std_closely() {
        let mut x = -20.0f32;
        while x < 20.0 {
            assert!((sin(x) - x.sin()).abs() < 1e-5, "sin({x})");
            assert!((cos(x) - x.cos()).abs() < 1e-5, "cos({x})");
            assert!((atan(x) - x.atan()).abs() < 2e-5, "atan({x})");
            x += 0.0137;
        }
        for (y, x) in [(1.0, 1.0), (1.0, -1.0), (-1.0, -1.0), (-0.3, 2.0), (0.0, -1.0), (2.0, 0.0)] {
            assert!((atan2(y, x) - f32::atan2(y, x)).abs() < 2e-5, "atan2({y},{x})");
        }
    }
}
