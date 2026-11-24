// 13-tap downsample (Jimenez, "Next Generation Post Processing in Call of Duty: AW").
#include fullscreen.wgsl

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
@group(0) @binding(2) var<uniform> params: vec4f; // xy = source texel size, z = 1 on first pass (Karis average)

fn tap(uv: vec2f, o: vec2f) -> vec3f {
    let c = textureSampleLevel(src, samp, uv + o * params.xy, 0.0).rgb;
    return clamp(c, vec3f(0.0), vec3f(6.0e4)); // guard NaN/Inf from the scene
}

fn karis(c: vec3f) -> f32 {
    return 1.0 / (1.0 + dot(c, vec3f(0.2126, 0.7152, 0.0722)));
}

@fragment
fn fs_down(in: FsOut) -> @location(0) vec4f {
    let uv = in.uv;
    let a = tap(uv, vec2f(-2.0, -2.0));
    let b = tap(uv, vec2f(0.0, -2.0));
    let c = tap(uv, vec2f(2.0, -2.0));
    let d = tap(uv, vec2f(-2.0, 0.0));
    let e = tap(uv, vec2f(0.0, 0.0));
    let f = tap(uv, vec2f(2.0, 0.0));
    let g = tap(uv, vec2f(-2.0, 2.0));
    let h = tap(uv, vec2f(0.0, 2.0));
    let i = tap(uv, vec2f(2.0, 2.0));
    let j = tap(uv, vec2f(-1.0, -1.0));
    let k = tap(uv, vec2f(1.0, -1.0));
    let l = tap(uv, vec2f(-1.0, 1.0));
    let m = tap(uv, vec2f(1.0, 1.0));

    if (params.z > 0.5) {
        // Karis average per 2x2 group to suppress fireflies on the first mip.
        let g0 = (a + b + d + e) * 0.25;
        let g1 = (b + c + e + f) * 0.25;
        let g2 = (d + e + g + h) * 0.25;
        let g3 = (e + f + h + i) * 0.25;
        let g4 = (j + k + l + m) * 0.25;
        let w0 = karis(g0);
        let w1 = karis(g1);
        let w2 = karis(g2);
        let w3 = karis(g3);
        let w4 = karis(g4);
        let sum = g0 * w0 * 0.125 + g1 * w1 * 0.125 + g2 * w2 * 0.125 + g3 * w3 * 0.125 + g4 * w4 * 0.5;
        let wsum = w0 * 0.125 + w1 * 0.125 + w2 * 0.125 + w3 * 0.125 + w4 * 0.5;
        return vec4f(sum / max(wsum, 1e-4), 1.0);
    }
    let r = e * 0.125 + (a + c + g + i) * 0.03125 + (b + d + f + h) * 0.0625 + (j + k + l + m) * 0.125;
    return vec4f(r, 1.0);
}
