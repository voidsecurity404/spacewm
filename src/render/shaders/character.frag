#version 300 es
precision highp float;
in vec3 v_normal;
in vec2 v_uv;
in vec3 v_world;
uniform sampler2D u_texture;
uniform vec3 u_sun_dir;
uniform vec3 u_sun_color;
uniform vec3 u_sky_ambient;
uniform vec3 u_ground_ambient;
uniform vec3 u_camera_pos;
uniform vec3 u_fog_color;
uniform float u_fog_start;
uniform float u_fog_end;
out vec4 frag_color;
void main() {
    vec3 albedo = texture(u_texture, v_uv).rgb;
    vec3 n = normalize(v_normal);
    // Soft "wrap" lighting: the toon-like characters look best without hard terminators.
    float ndl = clamp((dot(n, u_sun_dir) + 0.35) / 1.35, 0.0, 1.0);
    vec3 ambient = mix(u_ground_ambient, u_sky_ambient, n.y * 0.5 + 0.5);
    vec3 color = albedo * (ambient + u_sun_color * ndl * 0.85);
    color = vec3(1.0) - exp(-color * 1.25);
    float fog = smoothstep(u_fog_start, u_fog_end, distance(v_world, u_camera_pos));
    frag_color = vec4(mix(color, u_fog_color, fog * 0.85), 1.0);
}
