// The `LOD 100` pass (`LightMode = Vertex`) of the project's "Sprite", "Textured Polygon" and
// "Color Polygon" shaders, recovered from assets_raw/Assets/Shader/*.bytes. The project renders
// with the legacy Vertex Lit path (ProjectSettings m_RenderingPath: 0), so this is the pass that
// runs:
//
//   Lighting On; ColorMaterial AmbientAndDiffuse      -> done per vertex on the CPU
//                                                         (sticks_sim::draw::vertex_lit), arriving
//                                                         here as the vertex colour
//   SetTexture [_MainTex] { combine texture * primary double, texture alpha * primary alpha }
//   ("Color Polygon": ConstantColor [_Color] combine primary * constant double, ... * constant)
//   Blend SrcAlpha OneMinusSrcAlpha; ZTest Always; ZWrite Off; Cull Back
//
// Values are gamma-space (m_ActiveColorSpace: 0). Bevy blends in linear space, so the colour is
// converted to linear and the alpha bent to `1 - (1 - a)^2.2`, which reproduces the gamma-space
// result exactly for black sources (every shadow) under a 2.2 power curve.
// TODO(UNCERTAIN): partially transparent coloured texels blend slightly differently.

#import bevy_pbr::forward_io::VertexOutput

struct Tint {
    color: vec4<f32>,
};

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> tint: Tint;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var main_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var main_sampler: sampler;

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let low = c / 12.92;
    let high = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    return select(high, low, c <= vec3<f32>(0.04045));
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let texel = textureSample(main_texture, main_sampler, in.uv);
    var primary = vec4<f32>(1.0);
#ifdef VERTEX_COLORS
    primary = in.color;
#endif
    let rgb = clamp(texel.rgb * tint.color.rgb * primary.rgb * 2.0, vec3<f32>(0.0), vec3<f32>(1.0));
    let a = clamp(texel.a * tint.color.a * primary.a, 0.0, 1.0);
    return vec4<f32>(srgb_to_linear(rgb), 1.0 - pow(1.0 - a, 2.2));
}
