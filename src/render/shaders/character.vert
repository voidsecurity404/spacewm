#version 300 es
// The player's body: each vertex is moved by up to 4 bones (skinning, see
// world/character.rs), then placed in the world.
layout(location = 0) in vec3 a_pos;
layout(location = 1) in vec3 a_normal;
layout(location = 2) in vec2 a_uv;
layout(location = 3) in vec4 a_joints;
layout(location = 4) in vec4 a_weights;

uniform mat4 u_joints[64];
uniform mat4 u_model;
uniform mat4 u_view;
uniform mat4 u_proj;

out vec3 v_normal;
out vec2 v_uv;
out vec3 v_world;

void main() {
    mat4 skin = u_joints[int(a_joints.x)] * a_weights.x
              + u_joints[int(a_joints.y)] * a_weights.y
              + u_joints[int(a_joints.z)] * a_weights.z
              + u_joints[int(a_joints.w)] * a_weights.w;
    vec4 world = u_model * skin * vec4(a_pos, 1.0);
    v_world = world.xyz;
    v_normal = normalize(mat3(u_model) * mat3(skin) * a_normal);
    v_uv = a_uv;
    gl_Position = u_proj * u_view * world;
}
