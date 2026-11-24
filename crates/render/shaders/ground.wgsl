#include common.wgsl

const CELL_Z: f32 = 5.0;
const ROAD_HALF: f32 = 9.0;

struct GroundOut {
    @builtin(position) clip: vec4f,
    @location(0) world: vec2f, // (x, world z) for scrolling line patterns
    @location(1) rel: vec3f,   // bent position relative to camera (fog direction)
    @location(2) height: f32,
}

fn terrain_height(x: f32, zw: f32) -> f32 {
    let m = smoothstep(45.0, 170.0, abs(x));
    if (m <= 0.0) {
        return 0.0;
    }
    let n = ridged_fbm(vec2f(x * 0.0055, zw * 0.0055));
    return m * (n * 150.0 - 4.0);
}

@vertex
fn vs_ground(@location(0) p: vec2f) -> GroundOut {
    // Snap the lattice to world-fixed z so terrain doesn't swim while scrolling.
    let scroll = frame.world.y;
    let snap = scroll - floor(scroll / CELL_Z) * CELL_Z;
    let z = p.y - snap;
    let zw = z + scroll;
    let h = terrain_height(p.x, zw);
    let bent = bend(vec3f(p.x, h, z));
    var o: GroundOut;
    o.clip = frame.view_proj * vec4f(bent, 1.0);
    o.world = vec2f(p.x, zw);
    o.rel = bent - frame.cam.xyz;
    o.height = h;
    return o;
}

// Anti-aliased constant-pixel-width grid lines (fwidth based; no MSAA needed).
fn grid_lines(coord: vec2f, width_px: f32) -> f32 {
    let fw = max(fwidth(coord), vec2f(1e-5));
    let d = abs(fract(coord - 0.5) - 0.5) / fw;
    let l = 1.0 - smoothstep(0.0, width_px, min(d.x, d.y));
    // Fade out where cells shrink below ~2px to suppress moire.
    let fade = 1.0 - smoothstep(0.25, 0.6, max(fw.x, fw.y));
    return l * fade;
}

fn line_x(x: f32, centre: f32, half_w: f32) -> f32 {
    let fw = max(fwidth(x), 1e-4);
    return 1.0 - smoothstep(half_w - fw, half_w + fw, abs(x - centre));
}

@fragment
fn fs_ground(in: GroundOut) -> @location(0) vec4f {
    let x = in.world.x;
    let zw = in.world.y;
    let ax = abs(x);
    let beat = pow(1.0 - frame.world.w, 4.0);
    // Derivative-based terms are evaluated in uniform control flow (WGSL rule),
    // then road vs terrain is chosen with a select.

    // Road surface.
    var road = vec3f(0.006, 0.004, 0.016);
    road += vec3f(0.025, 0.006, 0.04) * value_noise(vec2f(x * 0.8, zw * 0.05)); // wet sheen streaks
    let dash = step(fract(zw / 14.0), 0.45);
    let lanes = (line_x(x, -4.5, 0.10) + line_x(x, 4.5, 0.10)) * dash;
    road += vec3f(0.9, 0.85, 1.0) * lanes * 2.0;
    road += vec3f(1.0, 0.08, 0.55) * (line_x(x, -0.25, 0.08) + line_x(x, 0.25, 0.08)) * 3.5;
    let edge = line_x(ax, ROAD_HALF - 0.25, 0.18);
    road += vec3f(0.05, 0.85, 1.0) * edge * (4.0 + 2.0 * beat);
    let alt = step(0.5, fract(zw / 8.0));
    let rumble = mix(vec3f(0.9, 0.05, 0.5), vec3f(0.05, 0.6, 0.9), alt) * 0.7;
    road = select(road, rumble, ax > ROAD_HALF);

    // Ground plane + wireframe mountains.
    let hn = clamp(in.height / 140.0, 0.0, 1.0);
    var ground = mix(vec3f(0.010, 0.000, 0.022), vec3f(0.035, 0.004, 0.060), hn);
    let l = grid_lines(vec2f(x, zw) / 10.0, 1.2);
    let line_col = mix(vec3f(1.0, 0.08, 0.62), vec3f(0.25, 0.35, 1.0), smoothstep(5.0, 90.0, in.height));
    ground += line_col * l * (2.2 + 1.5 * beat);
    ground += vec3f(1.0, 0.2, 0.6) * smoothstep(0.55, 0.95, hn) * 0.25; // hot rim near crests

    var c = select(ground, road, ax < ROAD_HALF + 1.2);

    // Sky-matched fog: distant geometry melts into the horizon glow.
    c = apply_fog(c, in.rel, in.height);
    return vec4f(c, 1.0);
}
