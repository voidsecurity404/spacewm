#version 300 es
// Depth from the sun, for the shadow map (drawn once per sun position).
layout(location = 0) in vec3 a_pos;
uniform mat4 u_sun_view_proj;
void main() {
    gl_Position = u_sun_view_proj * vec4(a_pos, 1.0);
}
