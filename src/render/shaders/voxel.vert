#version 300 es
// Block faces. Vertices are packed into 8 bytes (see world/mesh.rs) and unpacked here.
layout(location = 0) in uvec4 a_pos;  // x, y, z inside the chunk; face | corner << 3
layout(location = 1) in uint a_tile;  // texture array layer
layout(location = 2) in uint a_ao;    // ambient occlusion 0 (dark) .. 3 (open)

uniform mat4 u_view;
uniform mat4 u_proj;
uniform vec3 u_chunk_origin; // world position of the chunk's (0,0,0) corner
uniform vec3 u_sun_dir;      // normalized, pointing TOWARD the sun

out vec3 v_uv;       // (u, v, layer)
out float v_light;
out vec3 v_world_pos;

const vec3 NORMALS[6] = vec3[6](
    vec3(1.0, 0.0, 0.0), vec3(-1.0, 0.0, 0.0),
    vec3(0.0, 1.0, 0.0), vec3(0.0, -1.0, 0.0),
    vec3(0.0, 0.0, 1.0), vec3(0.0, 0.0, -1.0)
);
// Corner order is bottom-left, bottom-right, top-right, top-left; v = 0 is the TOP row
// of the tile, so side faces show their textures upright.
const vec2 UVS[4] = vec2[4](vec2(0.0, 1.0), vec2(1.0, 1.0), vec2(1.0, 0.0), vec2(0.0, 0.0));

void main() {
    uint face = a_pos.w & 7u;
    uint corner = (a_pos.w >> 3u) & 3u;
    vec3 world = u_chunk_origin + vec3(a_pos.xyz);
    v_world_pos = world;
    v_uv = vec3(UVS[corner], float(a_tile));
    // Simple lighting: constant ambient + directional sun, darkened in creases (AO).
    float sun = max(dot(NORMALS[face], u_sun_dir), 0.0);
    float ao = 0.55 + 0.15 * float(a_ao);
    v_light = (0.55 + 0.45 * sun) * ao;
    gl_Position = u_proj * u_view * vec4(world, 1.0);
}
