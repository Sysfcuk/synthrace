// Tent-filter upsample. Writes up[i] = down[i] + tent(up[i+1]) without blending,
// so it works identically on WebGL2 and WebGPU.
#include fullscreen.wgsl

@group(0) @binding(0) var lower: texture_2d<f32>; // up[i+1] (smaller)
@group(0) @binding(1) var same: texture_2d<f32>;  // down[i]
@group(0) @binding(2) var samp: sampler;
@group(0) @binding(3) var<uniform> params: vec4f; // xy = lower texel size, z = radius scale

@fragment
fn fs_up(in: FsOut) -> @location(0) vec4f {
    let t = params.xy * params.z;
    let uv = in.uv;
    var s = textureSampleLevel(lower, samp, uv, 0.0).rgb * 4.0;
    s += textureSampleLevel(lower, samp, uv + vec2f(-t.x, 0.0), 0.0).rgb * 2.0;
    s += textureSampleLevel(lower, samp, uv + vec2f(t.x, 0.0), 0.0).rgb * 2.0;
    s += textureSampleLevel(lower, samp, uv + vec2f(0.0, -t.y), 0.0).rgb * 2.0;
    s += textureSampleLevel(lower, samp, uv + vec2f(0.0, t.y), 0.0).rgb * 2.0;
    s += textureSampleLevel(lower, samp, uv + vec2f(-t.x, -t.y), 0.0).rgb;
    s += textureSampleLevel(lower, samp, uv + vec2f(t.x, -t.y), 0.0).rgb;
    s += textureSampleLevel(lower, samp, uv + vec2f(-t.x, t.y), 0.0).rgb;
    s += textureSampleLevel(lower, samp, uv + vec2f(t.x, t.y), 0.0).rgb;
    let here = textureSampleLevel(same, samp, uv, 0.0).rgb;
    return vec4f(here + s / 16.0, 1.0);
}
