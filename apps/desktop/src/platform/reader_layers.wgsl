struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}
@group(0) @binding(0) var pixels: texture_2d<f32>;
@group(0) @binding(1) var pixel_sampler: sampler;

@vertex
fn vs_main(@builtin(vertex_index) index: u32,
           @location(0) rect: vec4<f32>, @location(1) uv_rect: vec4<f32>) -> VertexOutput {
    let corners = array<vec2<f32>, 6>(vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0),
                                    vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0));
    let corner = corners[index];
    var result: VertexOutput;
    result.position = vec4(mix(rect.xy, rect.zw, corner), 0.0, 1.0);
    result.uv = mix(uv_rect.xy, uv_rect.zw, corner);
    return result;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    // Vello's fine shader writes separated RGB/alpha, although its internal
    // blend math is premultiplied. Premultiply before interpolating; filtering
    // separated colors first would add dark fringes at transparent edges.
    let size = vec2<i32>(textureDimensions(pixels));
    let point = input.uv * vec2<f32>(size) - vec2(0.5);
    let origin = vec2<i32>(floor(point));
    let weight = fract(point);
    let a = premultiplied_load(origin, size);
    let b = premultiplied_load(origin + vec2(1, 0), size);
    let c = premultiplied_load(origin + vec2(0, 1), size);
    let d = premultiplied_load(origin + vec2(1, 1), size);
    return mix(mix(a, b, weight.x), mix(c, d, weight.x), weight.y);
}

fn premultiplied_load(point: vec2<i32>, size: vec2<i32>) -> vec4<f32> {
    let color = textureLoad(pixels, clamp(point, vec2(0), size - vec2(1)), 0);
    return vec4(color.rgb * color.a, color.a);
}
