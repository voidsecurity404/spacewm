#version 300 es
// Window content with premultiplied alpha, an optional border around the window
// geometry (focus/hover highlight), a dark back side and the same fog as the world.
precision highp float;

in vec2 v_uv;
in vec3 v_world_pos;

uniform sampler2D u_tex;
uniform float u_opacity;
uniform vec2 u_tex_size;     // texture size in px
uniform vec4 u_geo_rect;     // window geometry inside the texture: x0, y0, x1, y1 (px)
uniform float u_border_px;   // 0 = no border
uniform vec4 u_border_color; // straight (not premultiplied) RGBA
uniform vec3 u_camera_pos;
uniform vec3 u_fog_color;
uniform float u_fog_start;
uniform float u_fog_end;

out vec4 frag_color;

void main() {
    vec4 c = texture(u_tex, v_uv);
    vec2 px = v_uv * u_tex_size;
    vec4 g = u_geo_rect;
    if (u_border_px > 0.0 && px.x >= g.x && px.y >= g.y && px.x < g.z && px.y < g.w) {
        float d = min(min(px.x - g.x, g.z - px.x), min(px.y - g.y, g.w - px.y));
        if (d < u_border_px) {
            c = vec4(u_border_color.rgb * u_border_color.a, u_border_color.a);
        }
    }
    if (!gl_FrontFacing) {
        // The back of a panel: a dark silhouette, so front and back are easy to tell
        // apart (and text isn't shown mirrored).
        c = vec4(vec3(0.10) * c.a, c.a);
    }
    c *= u_opacity;
    // Fully transparent pixels (shadow edges, rounded corners) must not write depth,
    // or they would hide panels behind them.
    if (c.a < 0.004) {
        discard;
    }
    float fog = smoothstep(u_fog_start, u_fog_end, distance(v_world_pos, u_camera_pos));
    // Premultiplied: the fog color is scaled by alpha too.
    frag_color = vec4(mix(c.rgb, u_fog_color * c.a, fog), c.a);
}
