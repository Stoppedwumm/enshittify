// Layer compositor: draws one transformed quad per layer.
// Output is premultiplied alpha; blending is fixed-function per pipeline.

struct Layer {
    // Affine UV -> output pixels: x = m0.x*u + m0.y*v + m0.z ; y = m1.x*u + m1.y*v + m1.z
    m0: vec4<f32>,
    m1: vec4<f32>,
    // Straight RGBA for solid layers.
    color: vec4<f32>,
    // x: opacity, y: mode (0 = texture, 1 = solid), zw: output size in px
    params: vec4<f32>,
};

@group(0) @binding(0) var<uniform> L: Layer;
@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VsOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
    );
    let uv = corners[i];
    let px = vec2<f32>(
        L.m0.x * uv.x + L.m0.y * uv.y + L.m0.z,
        L.m1.x * uv.x + L.m1.y * uv.y + L.m1.z,
    );
    let ndc = vec2<f32>(px.x / L.params.z * 2.0 - 1.0, 1.0 - px.y / L.params.w * 2.0);
    var o: VsOut;
    o.pos = vec4<f32>(ndc, 0.0, 1.0);
    o.uv = uv;
    return o;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    var c: vec4<f32>;
    if (L.params.y < 0.5) {
        c = textureSample(tex, samp, in.uv);
    } else {
        c = L.color;
    }
    let a = c.a * L.params.x;
    return vec4<f32>(c.rgb * a, a);
}
