//! M0 spike only: an analytic curve/hill function standing in for the real
//! segment track that `sim` will own from M1.

use crate::PROFILE_SAMPLES;
pub use crate::PROFILE_STEP;

/// Road curvature (1/m) at track distance `s`.
pub fn curve(s: f32) -> f32 {
    0.0009 * (s / 420.0).sin() + 0.0006 * (s / 173.0 + 1.3).sin()
}

/// Road height (m) at track distance `s`.
pub fn hill(s: f32) -> f32 {
    18.0 * (s / 390.0).sin() + 7.0 * (s / 141.0 + 0.7).sin()
}

/// Accumulated lateral offset C(z) and relative height H(z) ahead of `s`,
/// integrated exactly like classic pseudo-3D (dx += curve; x += dx).
pub fn profile(s: f32) -> [[f32; 2]; PROFILE_SAMPLES] {
    let mut out = [[0.0; 2]; PROFILE_SAMPLES];
    let h0 = hill(s);
    let (mut x, mut dx) = (0.0f32, 0.0f32);
    for (i, p) in out.iter_mut().enumerate() {
        let z = i as f32 * PROFILE_STEP;
        *p = [x, hill(s + z) - h0];
        dx += curve(s + z) * PROFILE_STEP;
        x += dx * PROFILE_STEP;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_starts_at_camera_and_is_finite() {
        for s in [0.0, 1234.5, 98_765.0] {
            let p = profile(s);
            assert_eq!(p[0], [0.0, 0.0]);
            assert!(p.iter().all(|v| v[0].is_finite() && v[1].is_finite()));
        }
    }
}
