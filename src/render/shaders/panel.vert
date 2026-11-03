#version 300 es
// A window panel: a unit quad (-0.5..0.5) placed by u_model over the window's bbox.
layout(location = 0) in vec2 a_pos;

uniform mat4 u_model;
uniform mat4 u_view;
uniform mat4 u_proj;
uniform vec2 u_grow; // extra quad on each side for a border outside the window, as a part of it

out vec2 v_uv;
out vec3 v_world_pos;

void main() {
    // GL textures start at the bottom-left, but smithay renders window textures top row
    // first (like Wayland buffers). So the TOP of the quad (y = +0.5) samples v = 0.
    vec2 pos = a_pos * (1.0 + 2.0 * u_grow);
    v_uv = vec2(pos.x + 0.5, 0.5 - pos.y);
    vec4 world = u_model * vec4(pos, 0.0, 1.0);
    v_world_pos = world.xyz;
    gl_Position = u_proj * u_view * world;
}
