#include common.wgsl
#include fullscreen.wgsl

@fragment
fn fs_sky(in: FsOut) -> @location(0) vec4f {
    let ndc = vec2f(in.uv.x * 2.0 - 1.0, 1.0 - in.uv.y * 2.0);
    let wp = frame.inv_view_proj * vec4f(ndc, 1.0, 1.0);
    let rd = normalize(wp.xyz / wp.w - frame.cam.xyz);
    var c = sky_color(rd);
    let t = frame.cam.w;

    // Stars: hashed cells on the tangent plane, twinkling, fading toward the horizon.
    if (rd.y > 0.02) {
        let sp = vec2f(atan2(rd.x, rd.z), asin(clamp(rd.y, -1.0, 1.0))) * 180.0;
        let cell = floor(sp);
        let r = hash12(cell);
        if (r > 0.992) {
            let centre = cell + 0.5 + (vec2f(hash12(cell + 7.0), hash12(cell + 13.0)) - 0.5) * 0.6;
            let d = length(sp - centre);
            let tw = 0.6 + 0.4 * sin(t * (1.5 + r * 4.0) + r * 60.0);
            c += vec3f(0.7, 0.8, 1.0) * (1.0 - smoothstep(0.0, 0.9, d)) * tw * 1.6 * smoothstep(0.02, 0.25, rd.y);
        }
    }

    // Banded synthwave sun (analytic, HDR so it blooms).
    let sd = sun_dir();
    let local = rd.xy / rd.z - sd.xy / sd.z;
    let radius = 0.23;
    let d = length(local);
    let fw = max(fwidth(d), 1e-5);
    let disc = 1.0 - smoothstep(radius - fw, radius + fw, d);
    let y = local.y / radius; // -1 bottom .. +1 top
    let grad = mix(vec3f(1.0, 0.06, 0.42), vec3f(1.0, 0.78, 0.22), smoothstep(-0.9, 0.8, y));
    // Slits: start below y = 0.35 and widen toward the bottom; scroll slowly downward.
    let k = clamp((0.45 - y) / 1.45, 0.0, 1.0);
    let bands = y * 7.0 + t * 0.12;
    let bw = max(fwidth(bands), 1e-4);
    let f = fract(bands);
    let gap = 0.08 + k * 0.7;
    let slit = select(1.0, smoothstep(gap - bw, gap + bw, f), y < 0.45);
    let pulse = 1.0 + 0.25 * pow(1.0 - frame.world.w, 6.0);
    c = mix(c, grad * 2.4 * pulse, disc * slit);
    return vec4f(c, 1.0);
}
