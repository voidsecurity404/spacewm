#version 300 es
// Plain colored geometry (floor, axes). Runs once per vertex.
layout(location = 0) in vec3 a_pos;
layout(location = 1) in vec3 a_color;

uniform mat4 u_model; // object -> world
uniform mat4 u_view;  // world -> camera
uniform mat4 u_proj;  // camera -> clip space (perspective)

out vec3 v_color;
out vec3 v_world_pos;

void main() {
    vec4 world = u_model * vec4(a_pos, 1.0);
    v_world_pos = world.xyz;
    v_color = a_color;
    gl_Position = u_proj * u_view * world;
}
