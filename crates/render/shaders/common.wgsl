// Shared frame uniforms, track-space bend, sky model and noise.
// Track space: x lateral (m, +right), y up (m), z ahead of the camera (m).

struct Frame {
    view_proj: mat4x4f,
    inv_view_proj: mat4x4f,
    cam: vec4f,      // xyz camera position (track space), w = time (s)
    world: vec4f,    // x = horizon curvature k, y = scroll distance (m), z = speed 0..1, w = beat phase 0..1
    screen: vec4f,   // width, height, 1/width, 1/height
    profile: array<vec4f, 64>, // xy = (C(z), H(z)) sampled every PROFILE_STEP metres ahead
}

@group(0) @binding(0) var<uniform> frame: Frame;

const PROFILE_STEP: f32 = 20.0;
const PROFILE_LAST: f32 = 62.999;

fn profile_at(z: f32) -> vec2f {
    let f = clamp(z / PROFILE_STEP, 0.0, PROFILE_LAST);
    let i = u32(f);
    return mix(frame.profile[i].xy, frame.profile[i + 1u].xy, fract(f));
}

// The "Neon Bent-World" transform: straight track-space geometry is bent by the
// road's accumulated curve/hill profile plus a global horizon curvature.
fn bend(p: vec3f) -> vec3f {
    let t = profile_at(p.z);
    let zc = max(p.z, 0.0);
    return vec3f(p.x + t.x, p.y + t.y - frame.world.x * zc * zc, p.z);
}

fn sun_dir() -> vec3f {
    return normalize(vec3f(0.0, 0.11, 1.0));
}

// Linear HDR sky radiance along view direction rd. Also used as the fog colour,
// so distant geometry dissolves into the horizon glow.
fn sky_color(rd: vec3f) -> vec3f {
    let h = rd.y;
    let zenith = vec3f(0.006, 0.002, 0.030);
    let upper = vec3f(0.055, 0.006, 0.150);
    let horizon = vec3f(0.95, 0.10, 0.38);
    let haze = vec3f(0.05, 0.005, 0.08);
    var c = mix(upper, zenith, smoothstep(0.05, 0.55, h));
    c = mix(horizon, c, smoothstep(-0.01, 0.20, h));
    c = mix(haze, c, smoothstep(-0.12, -0.005, h));
    let g = max(dot(rd, sun_dir()), 0.0);
    c += vec3f(1.0, 0.30, 0.35) * pow(g, 10.0) * 0.55;
    c += vec3f(1.0, 0.55, 0.25) * pow(g, 120.0) * 1.2;
    return c;
}

fn hash12(p: vec2f) -> f32 {
    var q = vec2u(vec2i(floor(p)) + vec2i(32768));
    var h = (q.x * 0x27d4eb2du) ^ (q.y * 0x165667b1u); // WGSL requires parens when mixing * and ^
    h = (h ^ (h >> 15u)) * 0x2c1b3c6du;
    h = h ^ (h >> 12u);
    return f32(h & 0xffffffu) / 16777215.0;
}

fn value_noise(p: vec2f) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let a = hash12(i);
    let b = hash12(i + vec2f(1.0, 0.0));
    let c = hash12(i + vec2f(0.0, 1.0));
    let d = hash12(i + vec2f(1.0, 1.0));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

// Ridged fBm: sharp crests for synthwave mountains (cf. Inigo Quilez, "fbm").
fn ridged_fbm(p0: vec2f) -> f32 {
    var p = p0;
    var sum = 0.0;
    var amp = 0.55;
    var prev = 1.0;
    for (var i = 0; i < 5; i++) {
        var n = 1.0 - abs(value_noise(p) * 2.0 - 1.0);
        n = n * n;
        sum += n * amp * prev;
        prev = n;
        p = p * 2.03 + vec2f(17.1, 9.7);
        amp *= 0.5;
    }
    return sum;
}

// sRGB OETF for outputs that are not an *Srgb surface format.
fn linear_to_srgb(c: vec3f) -> vec3f {
    let lo = c * 12.92;
    let hi = 1.055 * pow(max(c, vec3f(0.0)), vec3f(1.0 / 2.4)) - 0.055;
    return select(hi, lo, c <= vec3f(0.0031308));
}

// Sky-matched distance fog shared by every surface. `rel` = position relative to the camera.
fn apply_fog(c: vec3f, rel: vec3f, height: f32) -> vec3f {
    let dist = length(rel);
    let rd = rel / max(dist, 1e-3);
    var fog = 1.0 - exp(-max(dist - 30.0, 0.0) * 0.0021);
    fog *= 1.0 - 0.35 * smoothstep(20.0, 140.0, height);
    return mix(c, sky_color(rd), clamp(fog, 0.0, 1.0));
}
