#version 300 es
// Window content with premultiplied alpha, an optional border around the window
// geometry (focus/hover highlight), a dark back side and the same fog as the world.
precision highp float;

in vec2 v_uv;
in vec3 v_world_pos;

uniform sampler2D u_tex;
uniform float u_opacity;
uniform float u_flip_y;      // 1: the texture's rows are stored bottom first
uniform float u_opaque_rgb;  // 1: XRGB texture, its alpha channel is padding
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
    // A client's own texture (shown directly, see panel_texture.rs) may be stored bottom
    // row first and may have no real alpha.
    vec2 uv = u_flip_y > 0.5 ? vec2(v_uv.x, 1.0 - v_uv.y) : v_uv;
    // The quad grows past the texture to make room for the border: nothing to sample there.
    bool in_texture = v_uv.x >= 0.0 && v_uv.x <= 1.0 && v_uv.y >= 0.0 && v_uv.y <= 1.0;
    vec4 c = in_texture ? texture(u_tex, uv) : vec4(0.0);
    if (in_texture && u_opaque_rgb > 0.5) {
        c.a = 1.0;
    }
    vec2 px = v_uv * u_tex_size;
    vec4 g = u_geo_rect;
    // The border is drawn just outside the window geometry, so it never hides the app's
    // own pixels (a terminal's first row, the buttons at a window's edge).
    float outside = max(max(g.x - px.x, px.x - g.z), max(g.y - px.y, px.y - g.w));
    if (u_border_px > 0.0 && outside > 0.0 && outside <= u_border_px) {
        c = vec4(u_border_color.rgb * u_border_color.a, u_border_color.a);
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
