struct FsOut {
    @builtin(position) pos: vec4f,
    @location(0) uv: vec2f,
}

// One oversized triangle covering the viewport; uv (0,0) = top-left.
@vertex
fn vs_fullscreen(@builtin(vertex_index) vi: u32) -> FsOut {
    let x = f32((vi << 1u) & 2u);
    let y = f32(vi & 2u);
    var o: FsOut;
    o.pos = vec4f(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    o.uv = vec2f(x, y);
    return o;
}
