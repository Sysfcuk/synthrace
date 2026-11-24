// Instanced, flat-shaded meshes (car, palms) placed in track space and bent with the world.
#include common.wgsl

struct MeshIn {
    @location(0) pos: vec3f,
    @location(1) normal: vec3f,
    @location(2) color: vec3f,
    @location(3) emissive: f32,
    @location(4) i_pos_yaw: vec4f,   // track-space x, y, z (camera-relative), yaw (rad)
    @location(5) i_scale_roll: vec4f, // uniform scale, roll (rad), brake, indicator
    @location(6) i_extra: vec4f,      // pitch (rad, + = nose up)
}

struct MeshOut {
    @builtin(position) clip: vec4f,
    @location(0) rel: vec3f,
    @location(1) normal: vec3f,
    @location(2) color: vec3f,
    @location(3) emissive: f32,
    @location(4) height: f32,
    @location(5) lights: vec2f, // brake 0..1, indicator signal −1/0/+1
}

fn rot_y(v: vec3f, a: f32) -> vec3f {
    let c = cos(a);
    let s = sin(a);
    return vec3f(v.x * c + v.z * s, v.y, -v.x * s + v.z * c);
}

fn rot_x(v: vec3f, a: f32) -> vec3f {
    let c = cos(a);
    let s = sin(a);
    return vec3f(v.x, v.y * c + v.z * s, -v.y * s + v.z * c);
}

fn rot_z(v: vec3f, a: f32) -> vec3f {
    let c = cos(a);
    let s = sin(a);
    return vec3f(v.x * c - v.y * s, v.x * s + v.y * c, v.z);
}

@vertex
fn vs_mesh(in: MeshIn) -> MeshOut {
    let yaw = in.i_pos_yaw.w;
    let roll = in.i_scale_roll.y;
    let pitch = in.i_extra.x;
    let local = rot_y(rot_z(rot_x(in.pos * in.i_scale_roll.x, pitch), roll), yaw);
    let bent = bend(local + in.i_pos_yaw.xyz);
    var o: MeshOut;
    o.clip = frame.view_proj * vec4f(bent, 1.0);
    o.rel = bent - frame.cam.xyz;
    o.normal = rot_y(rot_z(rot_x(in.normal, pitch), roll), yaw);
    o.color = in.color;
    o.emissive = in.emissive;
    o.height = local.y + in.i_pos_yaw.y;
    o.lights = in.i_scale_roll.zw;
    return o;
}

@fragment
fn fs_mesh(in: MeshOut) -> @location(0) vec4f {
    if (in.emissive < -0.5) {
        // Contact shadow: darken toward black, fogged like everything else.
        return vec4f(apply_fog(vec3f(0.002, 0.0, 0.006), in.rel, 0.0), 1.0);
    }
    var emissive = in.emissive;
    if (emissive > 49.5) {
        // Per-instance light markers (see procgen::marker).
        let code = round(emissive);
        let blink = step(0.5, fract(frame.cam.w * 1.6));
        if (code == 50.0) {
            emissive = 1.6 + 6.0 * in.lights.x;
        } else {
            let side = select(1.0, -1.0, code == 51.0);
            emissive = select(0.0, 9.0 * blink, in.lights.y * side > 0.5);
        }
    }
    let dist = length(in.rel);
    let v = -in.rel / max(dist, 1e-3);
    var n = normalize(in.normal);
    if (dot(n, v) < 0.0) {
        n = -n; // two-sided for fronds and thin parts
    }
    let sd = sun_dir();
    // Ambient: violet sky from above, magenta bounce from the glowing ground and horizon.
    let amb = mix(vec3f(0.30, 0.03, 0.22), vec3f(0.10, 0.05, 0.30), n.y * 0.5 + 0.5) * 0.4;
    // Low sunset key light from the horizon ahead.
    let key = max(dot(n, sd), 0.0) * vec3f(1.0, 0.42, 0.30) * 0.9;
    // Neon rim: cyan on top edges, magenta on lower edges; strongest against the sun.
    let ndv = max(dot(n, v), 0.0);
    // Rim mostly on silhouettes, not on decks/roofs seen at grazing angles from above.
    let rim = pow(1.0 - ndv, 3.0) * (1.0 - 0.8 * smoothstep(0.45, 0.85, n.y));
    let rim_col = mix(vec3f(1.0, 0.18, 0.55), vec3f(0.15, 0.75, 1.0), smoothstep(-0.2, 0.6, n.y));
    // Glossy sky reflection on upward faces (car body, roof).
    let refl = reflect(-v, n);
    let gloss = sky_color(refl) * 0.12 * smoothstep(0.0, 0.6, n.y);
    let lit = select(1.0, 0.0, emissive > 0.5);
    var c = (in.color * (amb + key) + rim_col * rim * 1.3 + gloss) * lit + in.color * emissive;
    c = apply_fog(c, in.rel, in.height);
    return vec4f(c, 1.0);
}
