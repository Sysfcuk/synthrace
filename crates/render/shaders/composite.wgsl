// Final composite: bloom, speed radial blur, chromatic aberration, AgX tonemap
// with a "punchy" look, vignette and grain. Output is display-referred.
#include common.wgsl
#include fullscreen.wgsl

struct Post {
    a: vec4f, // x = bloom strength, y = exposure, z = bloom normalisation (1/levels), w = 1 if output needs manual sRGB encode
    b: vec4f, // x = vignette, y = grain, z = chromatic aberration, w = radial blur strength
    flash: vec4f, // rgb colour, w = intensity (impacts, checkpoints)
}

@group(1) @binding(0) var scene: texture_2d<f32>;
@group(1) @binding(1) var bloom: texture_2d<f32>;
@group(1) @binding(2) var samp: sampler;
@group(1) @binding(3) var<uniform> post: Post;

// AgX (Troy Sobotka), minimal fit by Benjamin Wrensch (MIT).
fn agx_contrast(x: vec3f) -> vec3f {
    let x2 = x * x;
    let x4 = x2 * x2;
    return 15.5 * x4 * x2 - 40.14 * x4 * x + 31.96 * x4 - 6.868 * x2 * x + 0.4298 * x2 + 0.1191 * x - 0.00232;
}

fn agx(v: vec3f) -> vec3f {
    let m = mat3x3f(
        vec3f(0.842479062253094, 0.0423282422610123, 0.0423756549057051),
        vec3f(0.0784335999999992, 0.878468636469772, 0.0784336),
        vec3f(0.0792237451477643, 0.0791661274605434, 0.879142973793104));
    let min_ev = -12.47393;
    let max_ev = 4.026069;
    var x = m * max(v, vec3f(1e-10));
    x = clamp(log2(x), vec3f(min_ev), vec3f(max_ev));
    x = (x - min_ev) / (max_ev - min_ev);
    return agx_contrast(x);
}

fn agx_punchy(v: vec3f) -> vec3f {
    let p = pow(max(v, vec3f(0.0)), vec3f(1.45));
    let luma = dot(p, vec3f(0.2126, 0.7152, 0.0722));
    return luma + 1.45 * (p - luma);
}

fn agx_eotf(v: vec3f) -> vec3f {
    let mi = mat3x3f(
        vec3f(1.19687900512017, -0.0528968517574562, -0.0529716355144438),
        vec3f(-0.0980208811401368, 1.15190312990417, -0.0980434501171241),
        vec3f(-0.0990297440797205, -0.0989611768448433, 1.15107367264116));
    // AgX output is display-encoded (~2.2); convert back to linear for the sRGB target.
    return pow(max(mi * v, vec3f(0.0)), vec3f(2.2));
}

fn sample_scene(uv: vec2f, ca: f32) -> vec3f {
    let dir = uv - vec2f(0.5, 0.45);
    let r = textureSampleLevel(scene, samp, uv - dir * ca, 0.0).r;
    let g = textureSampleLevel(scene, samp, uv, 0.0).g;
    let b = textureSampleLevel(scene, samp, uv + dir * ca, 0.0).b;
    return vec3f(r, g, b);
}

@fragment
fn fs_composite(in: FsOut) -> @location(0) vec4f {
    let uv = in.uv;
    let ca = post.b.z;
    var c = sample_scene(uv, ca);

    // Cheap radial speed blur about the vanishing point; centre stays sharp.
    let blur = post.b.w;
    if (blur > 0.001) {
        let centre = vec2f(0.5, 0.42);
        let dir = uv - centre;
        let mask = smoothstep(0.08, 0.5, length(dir));
        var acc = c;
        for (var i = 1; i < 8; i++) {
            acc += textureSampleLevel(scene, samp, uv - dir * blur * mask * f32(i) / 8.0, 0.0).rgb;
        }
        c = acc / 8.0;
    }

    let bl = textureSampleLevel(bloom, samp, uv, 0.0).rgb * post.a.z;
    c = mix(c, bl, post.a.x);
    c *= post.a.y;
    c += post.flash.rgb * post.flash.w;

    var d = agx_punchy(agx(c));

    // Vignette + grain (display space).
    let q = uv - 0.5;
    d *= 1.0 - post.b.x * dot(q, q) * 2.2;
    let n = hash12(in.pos.xy + vec2f(fract(frame.cam.w * 7.13) * 1000.0)) - 0.5;
    d += n * post.b.y;

    var lin = agx_eotf(clamp(d, vec3f(0.0), vec3f(1.0)));
    if (post.a.w > 0.5) {
        lin = linear_to_srgb(lin);
    }
    return vec4f(lin, 1.0);
}
