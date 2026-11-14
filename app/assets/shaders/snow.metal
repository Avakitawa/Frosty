#include <metal_stdlib>
using namespace metal;

struct Particle {
    float x;
    float y;
    float diameter;
    float opacity;
    float phase;
    float rotation;
    float depth;
    float window_draft;
};

struct FrameUniforms {
    float elapsed_time;
    uint blocked_region_count;
    float drawable_width;
    float drawable_height;
    uint snow_suppressed;
    uint particles_in_windows;
};

struct BlockedRegion {
    float x;
    float y;
    float width;
    float height;
};

struct ParticleVertex {
    float4 position [[position]];
    float point_size [[point_size]];
    float opacity;
    float2 snow_shape;
    float accumulation;
    float window_draft;
};

struct SnowMeshOutput {
    float4 position [[position]];
};

vertex SnowMeshOutput snow_mesh_vertex(
    uint vertex_id [[vertex_id]],
    const device float2* vertices [[buffer(0)]]
) {
    float2 point = clamp(vertices[vertex_id], float2(0.0), float2(1.0));
    SnowMeshOutput output;
    output.position = float4(point.x * 2.0 - 1.0, 1.0 - point.y * 2.0, 0.0, 1.0);
    return output;
}

vertex ParticleVertex snow_vertex(
    uint vertex_id [[vertex_id]],
    const device Particle* particles [[buffer(0)]],
    constant FrameUniforms& frame [[buffer(1)]]
) {
    Particle particle = particles[vertex_id];
    float x = clamp(particle.x, 0.0, 1.0);
    float y = clamp(particle.y, 0.0, 1.0);

    ParticleVertex output;
    output.position = float4(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    output.point_size = particle.diameter * (1.0 + particle.depth * 0.35);
    output.opacity = particle.opacity * (particle.window_draft > 0.5
        ? 1.0
        : 0.96 + 0.04 * sin(frame.elapsed_time * 1.1 + particle.phase));
    output.snow_shape = float2(particle.phase, particle.rotation);
    output.accumulation = 0.0;
    output.window_draft = particle.window_draft;
    return output;
}

fragment float4 snow_fragment(
    ParticleVertex input [[stage_in]],
    float2 point_coord [[point_coord]],
    constant FrameUniforms& frame [[buffer(1)]],
    const device BlockedRegion* blocked_regions [[buffer(2)]]
) {
    if (frame.snow_suppressed != 0) {
        discard_fragment();
    }
    if (frame.drawable_width <= 0.0 || frame.drawable_height <= 0.0) {
        discard_fragment();
    }
    float2 fragment_position = input.position.xy
        / float2(frame.drawable_width, frame.drawable_height);
    for (uint index = 0; index < frame.blocked_region_count; index++) {
        BlockedRegion region = blocked_regions[index];
        bool inside_x = fragment_position.x >= region.x
            && fragment_position.x < region.x + region.width;
        bool inside_y = fragment_position.y >= region.y
            && fragment_position.y < region.y + region.height;
        bool always_blocked = input.accumulation > 0.5;
        if (inside_x && inside_y
            && (always_blocked
                || (frame.particles_in_windows == 0 && input.window_draft < 0.5))) {
            discard_fragment();
        }
    }
    if (input.accumulation > 0.5) {
        float2 local = (point_coord - 0.5) / input.snow_shape + 0.5;
        if (any(local < 0.0) || any(local > 1.0)) {
            discard_fragment();
        }
        float x = local.x * 2.0 - 1.0;
        float ridge_top = 0.06 + 0.86 * pow(abs(x), 2.2);
        float edge = max(fwidth(local.y - ridge_top), 0.012);
        float coverage = smoothstep(-edge, edge, local.y - ridge_top);
        coverage *= smoothstep(0.0, 0.045, local.x)
            * smoothstep(0.0, 0.045, 1.0 - local.x);
        float shade = 0.94 + 0.06 * (1.0 - local.y);
        float alpha = coverage * input.opacity;
        return float4(float3(0.93, 0.97, 1.0) * shade * alpha, alpha);
    }
    float distance_from_center = distance(point_coord, float2(0.5));
    float edge = 1.0 - smoothstep(0.22, 0.5, distance_from_center);
    float alpha = edge * input.opacity;
    return float4(float3(0.88, 0.95, 1.0) * alpha, alpha);
}

fragment float4 snow_mesh_fragment(
    SnowMeshOutput input [[stage_in]],
    constant FrameUniforms& frame [[buffer(1)]]
) {
    if (frame.snow_suppressed != 0) {
        discard_fragment();
    }
    if (frame.drawable_width <= 0.0 || frame.drawable_height <= 0.0) {
        discard_fragment();
    }
    float y = clamp(input.position.y / frame.drawable_height, 0.0, 1.0);
    float shade = 0.95 + 0.05 * (1.0 - y);
    float alpha = 0.96;
    return float4(float3(0.93, 0.97, 1.0) * shade * alpha, alpha);
}

struct ShapeVertex {
    float2 position;
    float2 padding;
    float4 color;
};

struct ShapeOutput {
    float4 position [[position]];
    float4 color;
};

vertex ShapeOutput shape_vertex(
    uint vertex_id [[vertex_id]],
    const device ShapeVertex* vertices [[buffer(0)]]
) {
    ShapeVertex item = vertices[vertex_id];
    ShapeOutput output;
    output.position = float4(
        item.position.x * 2.0 - 1.0,
        1.0 - item.position.y * 2.0,
        0.0,
        1.0
    );
    output.color = item.color;
    return output;
}

fragment float4 shape_fragment(ShapeOutput input [[stage_in]]) {
    float alpha = input.color.a;
    return float4(input.color.rgb * alpha, alpha);
}
