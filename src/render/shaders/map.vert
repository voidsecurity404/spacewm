#version 300 es
// The map ("dust"): static boxes, world-space positions, one draw call.
layout(location = 0) in vec3 a_pos;
layout(location = 1) in vec3 a_normal;
layout(location = 2) in vec3 a_tangent;
layout(location = 3) in vec2 a_uv;
layout(location = 4) in float a_layer;
layout(location = 5) in vec3 a_tint;
layout(location = 6) in float a_base_y;

uniform mat4 u_view;
uniform mat4 u_proj;
// World -> shadow map texture coordinates (0..1) and depth.
uniform mat4 u_shadow_matrix;

out vec3 v_world;
out vec3 v_normal;
out vec3 v_tangent;
out vec3 v_uv;
out vec3 v_tint;
out float v_base_y;
out vec3 v_shadow;

void main() {
    v_world = a_pos;
    v_normal = a_normal;
    v_tangent = a_tangent;
    v_uv = vec3(a_uv, a_layer);
    v_tint = a_tint;
    v_base_y = a_base_y;
    // Look the shadow up a little off the surface ("normal offset"), so a face never
    // shadows itself (shadow acne).
    v_shadow = (u_shadow_matrix * vec4(a_pos + a_normal * 0.06, 1.0)).xyz;
    gl_Position = u_proj * u_view * vec4(a_pos, 1.0);
}
