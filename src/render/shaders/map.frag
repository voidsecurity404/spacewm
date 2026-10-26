#version 300 es
precision highp float;
precision highp sampler2DArray;
precision highp sampler2DShadow;

in vec3 v_world;
in vec3 v_normal;
in vec3 v_tangent;
in vec3 v_uv;
in vec3 v_tint;
in float v_base_y;
in vec3 v_shadow;

uniform sampler2DArray u_albedo;
uniform sampler2DArray u_normals;
uniform sampler2DShadow u_shadow;
uniform bool u_shadows;
uniform float u_shadow_texel;
uniform vec3 u_sun_dir;      // toward the sun, normalized
uniform vec3 u_sun_color;
uniform vec3 u_sky_ambient;  // light from the sky (faces looking up)
uniform vec3 u_ground_ambient; // light bounced off the sand (faces looking down)
uniform vec3 u_camera_pos;
uniform vec3 u_fog_color;
uniform float u_fog_start;
uniform float u_fog_end;

out vec4 frag_color;

// Sun visibility: 0 in shadow, 1 lit. Four taps of the depth comparison (each one is
// already filtered 2x2 by the hardware) soften the shadow edge.
float sunlight() {
    if (!u_shadows) {
        return 1.0;
    }
    if (v_shadow.x < 0.0 || v_shadow.x > 1.0 || v_shadow.y < 0.0 || v_shadow.y > 1.0) {
        return 1.0;
    }
    float d = u_shadow_texel;
    float s = texture(u_shadow, v_shadow + vec3(-d, -d, 0.0))
            + texture(u_shadow, v_shadow + vec3(d, -d, 0.0))
            + texture(u_shadow, v_shadow + vec3(-d, d, 0.0))
            + texture(u_shadow, v_shadow + vec3(d, d, 0.0));
    return s * 0.25;
}

void main() {
    vec3 albedo = texture(u_albedo, v_uv).rgb * v_tint;
    // Normal map: tangent space -> world. The texture's "up" (green) is the face's up,
    // which is normal x tangent here (see world/map.rs).
    vec3 tn = texture(u_normals, v_uv).xyz * 2.0 - 1.0;
    vec3 n0 = normalize(v_normal);
    vec3 t = normalize(v_tangent);
    vec3 b = cross(n0, t);
    vec3 n = normalize(t * tn.x + b * tn.y + n0 * tn.z * 1.4);

    float ndl = max(dot(n, u_sun_dir), 0.0) * step(0.0, dot(n0, u_sun_dir));
    // Where a wall meets the floor it gets darker: light can't reach into the corner.
    float ao = mix(0.62, 1.0, clamp((v_world.y - v_base_y) / 1.3, 0.0, 1.0));
    vec3 ambient = mix(u_ground_ambient, u_sky_ambient, n.y * 0.5 + 0.5) * ao;
    vec3 color = albedo * (ambient + u_sun_color * ndl * sunlight());
    // Soft roll-off of the brightest parts (sunlit sand) instead of clipping to white.
    color = vec3(1.0) - exp(-color * 1.25);

    float fog = smoothstep(u_fog_start, u_fog_end, distance(v_world, u_camera_pos));
    frag_color = vec4(mix(color, u_fog_color, fog * 0.85), 1.0);
}
