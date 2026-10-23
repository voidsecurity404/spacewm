#version 300 es
// Runs once per pixel. Distance fog blends far geometry into the sky color, which hides
// the edge of the world and gives a sense of depth.
precision highp float;

in vec3 v_color;
in vec3 v_world_pos;

uniform vec3 u_camera_pos;
uniform vec3 u_fog_color;
uniform float u_fog_start;
uniform float u_fog_end;

out vec4 frag_color;

void main() {
    float fog = smoothstep(u_fog_start, u_fog_end, distance(v_world_pos, u_camera_pos));
    frag_color = vec4(mix(v_color, u_fog_color, fog), 1.0);
}
