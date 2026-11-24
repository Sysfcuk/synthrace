// Additive HDR particles: round soft sprites, or streaks stretched along the track (speed lines,
// light trails). Positions are camera-relative track space and bent like everything else.
#include common.wgsl

struct ParticleIn {
    @builtin(vertex_index) vi: u32,
    @location(0) pos_size: vec4f,
    @location(1) color: vec4f, // rgb premultiplied by intensity, w = streak length along z
}

struct ParticleOut {
    @builtin(position) clip: vec4f,
    @location(0) uv: vec2f,
    @location(1) color: vec3f,
    @location(2) rel: vec3f,
    @location(3) streak: f32,
}

@vertex
fn vs_particle(in: ParticleIn) -> ParticleOut {
    var corners = array<vec2f, 6>(
        vec2f(-1.0, -1.0), vec2f(1.0, -1.0), vec2f(1.0, 1.0),
        vec2f(-1.0, -1.0), vec2f(1.0, 1.0), vec2f(-1.0, 1.0));
    let c = corners[in.vi];
    let size = in.pos_size.w;
    let stretch = in.color.w;
    let centre = in.pos_size.xyz;
    var p: vec3f;
    if (stretch > 0.0) {
        // Vertical ribbon along the road: bends with the track.
        p = bend(centre + vec3f(0.0, c.x * size, c.y * stretch * 0.5));
    } else {
        // Camera-facing sprite (the camera never yaws or rolls, so x/y offsets face it).
        p = bend(centre) + vec3f(c.x * size, c.y * size, 0.0);
    }
    var o: ParticleOut;
    o.clip = frame.view_proj * vec4f(p, 1.0);
    o.uv = c;
    o.color = in.color.rgb;
    o.rel = p - frame.cam.xyz;
    o.streak = select(0.0, 1.0, stretch > 0.0);
    return o;
}

@fragment
fn fs_particle(in: ParticleOut) -> @location(0) vec4f {
    var a: f32;
    if (in.streak > 0.5) {
        a = (1.0 - abs(in.uv.x)) * (1.0 - in.uv.y * in.uv.y);
    } else {
        let r = length(in.uv);
        a = 1.0 - smoothstep(0.0, 1.0, r);
        a = a * a;
    }
    // Additive light fades into the fog rather than tinting toward the sky colour.
    let dist = length(in.rel);
    let fade = exp(-max(dist - 30.0, 0.0) * 0.0021);
    return vec4f(in.color * a * fade, 0.0);
}
