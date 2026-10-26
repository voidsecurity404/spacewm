#version 300 es
precision highp float;
in vec2 v_ndc;
// Screen -> world, to know which way each pixel looks.
uniform mat4 u_inv_view_proj;
uniform vec3 u_camera_pos;
uniform vec3 u_horizon;
uniform vec3 u_zenith;
uniform vec3 u_sun_dir;
uniform vec3 u_sun_color;
out vec4 frag_color;
void main() {
    vec4 far = u_inv_view_proj * vec4(v_ndc, 1.0, 1.0);
    vec3 dir = normalize(far.xyz / far.w - u_camera_pos);
    float up = max(dir.y, 0.0);
    vec3 color = mix(u_horizon, u_zenith, pow(up, 0.55));
    // Below the horizon: the hazy desert floor far away.
    color = mix(color, u_horizon * 0.92, smoothstep(0.0, -0.15, dir.y));
    // The sun: a bright disc and a wide soft glow around it.
    float s = max(dot(dir, u_sun_dir), 0.0);
    color += u_sun_color * (pow(s, 900.0) * 1.6 + pow(s, 12.0) * 0.18);
    frag_color = vec4(color, 1.0);
}
