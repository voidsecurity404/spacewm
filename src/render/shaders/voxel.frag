#version 300 es
precision highp float;
precision highp sampler2DArray;

in vec3 v_uv;
in float v_light;
in vec3 v_world_pos;

uniform sampler2DArray u_blocks;
// Pixels with less alpha than this are discarded (0.5 for leaves in the opaque pass).
uniform float u_alpha_cut;
uniform vec3 u_camera_pos;
uniform vec3 u_fog_color;
uniform float u_fog_start;
uniform float u_fog_end;

out vec4 frag_color;

void main() {
    vec4 c = texture(u_blocks, v_uv); // premultiplied alpha
    if (c.a < u_alpha_cut) {
        discard;
    }
    if (u_alpha_cut > 0.4) {
        // Cutout (leaves): what survives the test is fully opaque. Undo the premultiply,
        // or mipmapped (partly transparent) texels would darken distant trees.
        c = vec4(c.rgb / max(c.a, 0.001), 1.0);
    }
    c.rgb *= v_light;
    float fog = smoothstep(u_fog_start, u_fog_end, distance(v_world_pos, u_camera_pos));
    frag_color = vec4(mix(c.rgb, u_fog_color * c.a, fog), c.a);
}
