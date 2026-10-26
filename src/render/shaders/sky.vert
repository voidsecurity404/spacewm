#version 300 es
// The sky: one triangle covering the screen, made from the vertex number (no buffer).
out vec2 v_ndc;
void main() {
    vec2 p = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2)) * 2.0 - 1.0;
    v_ndc = p;
    gl_Position = vec4(p, 0.999, 1.0);
}
