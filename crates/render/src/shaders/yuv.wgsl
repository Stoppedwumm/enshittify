// Planar YUV (8-bit, any subsampling) -> RGBA, full-screen triangle.

struct Params {
    // rgb[i] = dot(row_i.xyz, yuv) + row_i.w
    r: vec4<f32>,
    g: vec4<f32>,
    b: vec4<f32>,
};

@group(0) @binding(0) var<uniform> P: Params;
@group(0) @binding(1) var tex_y: texture_2d<f32>;
@group(0) @binding(2) var tex_u: texture_2d<f32>;
@group(0) @binding(3) var tex_v: texture_2d<f32>;
@group(0) @binding(4) var samp: sampler;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VsOut {
    let x = f32((i << 1u) & 2u);
    let y = f32(i & 2u);
    var o: VsOut;
    o.pos = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    o.uv = vec2<f32>(x, y);
    return o;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let yuv = vec3<f32>(
        textureSample(tex_y, samp, in.uv).r,
        textureSample(tex_u, samp, in.uv).r,
        textureSample(tex_v, samp, in.uv).r,
    );
    let rgb = vec3<f32>(
        dot(P.r.xyz, yuv) + P.r.w,
        dot(P.g.xyz, yuv) + P.g.w,
        dot(P.b.xyz, yuv) + P.b.w,
    );
    return vec4<f32>(clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
