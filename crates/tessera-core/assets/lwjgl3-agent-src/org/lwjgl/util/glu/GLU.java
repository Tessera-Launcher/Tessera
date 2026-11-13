package org.lwjgl.util.glu;

import java.nio.FloatBuffer;
import java.nio.IntBuffer;

/** The {@code GLU} entry points 1.8.9 calls. */
public final class GLU {
    public static final int GLU_INVALID_ENUM = 100900;
    public static final int GLU_INVALID_VALUE = 100901;
    public static final int GLU_OUT_OF_MEMORY = 100902;
    public static final int GLU_INCOMPATIBLE_GL_VERSION = 100903;

    private GLU() {}

    /** {@code gluErrorString}: the GL error code's symbolic name. */
    public static String gluErrorString(int errorCode) {
        switch (errorCode) {
            case 0: return "no error";
            case 1280: return "invalid enum";
            case 1281: return "invalid value";
            case 1282: return "invalid operation";
            case 1283: return "stack overflow";
            case 1284: return "stack underflow";
            case 1285: return "out of memory";
            case 1286: return "invalid framebuffer operation";
            default: return "unknown error";
        }
    }

    /** {@code gluUnProject}: window coordinates to object coordinates. */
    public static boolean gluUnProject(float winX, float winY, float winZ,
                                       FloatBuffer model, FloatBuffer proj, IntBuffer viewport,
                                       FloatBuffer obj) {
        float[] in = new float[4];
        float[] out = new float[4];

        // Map window coordinates into normalised device coordinates.
        in[0] = (winX - viewport.get(viewport.position())) / viewport.get(viewport.position() + 2);
        in[1] = (winY - viewport.get(viewport.position() + 1)) / viewport.get(viewport.position() + 3);
        in[2] = 2.0f * winZ - 1.0f;
        in[3] = 1.0f;

        float[] m = new float[16];
        float[] p = new float[16];
        copy(model, m);
        copy(proj, p);

        // (projection * model) inverted, applied to `in`.
        float[] a = multiply(p, m);
        float[] inv = invert(a);
        if (inv == null) {
            return false;
        }
        for (int i = 0; i < 4; i++) {
            out[i] = inv[i] * in[0] + inv[i + 4] * in[1] + inv[i + 8] * in[2] + inv[i + 12] * in[3];
        }
        if (out[3] == 0.0f) {
            return false;
        }
        float w = out[3];
        obj.put(obj.position(), out[0] / w);
        obj.put(obj.position() + 1, out[1] / w);
        obj.put(obj.position() + 2, out[2] / w);
        return true;
    }

    private static void copy(FloatBuffer source, float[] target) {
        for (int i = 0; i < 16; i++) {
            target[i] = source.get(source.position() + i);
        }
    }

    /** Column-major 4x4 multiply, matching OpenGL's layout. */
    private static float[] multiply(float[] a, float[] b) {
        float[] out = new float[16];
        for (int col = 0; col < 4; col++) {
            for (int row = 0; row < 4; row++) {
                float sum = 0.0f;
                for (int k = 0; k < 4; k++) {
                    sum += a[k * 4 + row] * b[col * 4 + k];
                }
                out[col * 4 + row] = sum;
            }
        }
        return out;
    }

    /** General 4x4 inverse; {@code null} when the matrix is singular. */
    private static float[] invert(float[] m) {
        float[] inv = new float[16];
        inv[0] = m[5] * m[10] * m[15] - m[5] * m[11] * m[14] - m[9] * m[6] * m[15]
                + m[9] * m[7] * m[14] + m[13] * m[6] * m[11] - m[13] * m[7] * m[10];
        inv[4] = -m[4] * m[10] * m[15] + m[4] * m[11] * m[14] + m[8] * m[6] * m[15]
                - m[8] * m[7] * m[14] - m[12] * m[6] * m[11] + m[12] * m[7] * m[10];
        inv[8] = m[4] * m[9] * m[15] - m[4] * m[11] * m[13] - m[8] * m[5] * m[15]
                + m[8] * m[7] * m[13] + m[12] * m[5] * m[11] - m[12] * m[7] * m[9];
        inv[12] = -m[4] * m[9] * m[14] + m[4] * m[10] * m[13] + m[8] * m[5] * m[14]
                - m[8] * m[6] * m[13] - m[12] * m[5] * m[10] + m[12] * m[6] * m[9];
        inv[1] = -m[1] * m[10] * m[15] + m[1] * m[11] * m[14] + m[9] * m[2] * m[15]
                - m[9] * m[3] * m[14] - m[13] * m[2] * m[11] + m[13] * m[3] * m[10];
        inv[5] = m[0] * m[10] * m[15] - m[0] * m[11] * m[14] - m[8] * m[2] * m[15]
                + m[8] * m[3] * m[14] + m[12] * m[2] * m[11] - m[12] * m[3] * m[10];
        inv[9] = -m[0] * m[9] * m[15] + m[0] * m[11] * m[13] + m[8] * m[1] * m[15]
                - m[8] * m[3] * m[13] - m[12] * m[1] * m[11] + m[12] * m[3] * m[9];
        inv[13] = m[0] * m[9] * m[14] - m[0] * m[10] * m[13] - m[8] * m[1] * m[14]
                + m[8] * m[2] * m[13] + m[12] * m[1] * m[10] - m[12] * m[2] * m[9];
        inv[2] = m[1] * m[6] * m[15] - m[1] * m[7] * m[14] - m[5] * m[2] * m[15]
                + m[5] * m[3] * m[14] + m[13] * m[2] * m[7] - m[13] * m[3] * m[6];
        inv[6] = -m[0] * m[6] * m[15] + m[0] * m[7] * m[14] + m[4] * m[2] * m[15]
                - m[4] * m[3] * m[14] - m[12] * m[2] * m[7] + m[12] * m[3] * m[6];
        inv[10] = m[0] * m[5] * m[15] - m[0] * m[7] * m[13] - m[4] * m[1] * m[15]
                + m[4] * m[3] * m[13] + m[12] * m[1] * m[7] - m[12] * m[3] * m[5];
        inv[14] = -m[0] * m[5] * m[14] + m[0] * m[6] * m[13] + m[4] * m[1] * m[14]
                - m[4] * m[2] * m[13] - m[12] * m[1] * m[6] + m[12] * m[2] * m[5];
        inv[3] = -m[1] * m[6] * m[11] + m[1] * m[7] * m[10] + m[5] * m[2] * m[11]
                - m[5] * m[3] * m[10] - m[9] * m[2] * m[7] + m[9] * m[3] * m[6];
        inv[7] = m[0] * m[6] * m[11] - m[0] * m[7] * m[10] - m[4] * m[2] * m[11]
                + m[4] * m[3] * m[10] + m[8] * m[2] * m[7] - m[8] * m[3] * m[6];
        inv[11] = -m[0] * m[5] * m[11] + m[0] * m[7] * m[9] + m[4] * m[1] * m[11]
                - m[4] * m[3] * m[9] - m[8] * m[1] * m[7] + m[8] * m[3] * m[5];
        inv[15] = m[0] * m[5] * m[10] - m[0] * m[6] * m[9] - m[4] * m[1] * m[10]
                + m[4] * m[2] * m[9] + m[8] * m[1] * m[6] - m[8] * m[2] * m[5];

        float det = m[0] * inv[0] + m[1] * inv[4] + m[2] * inv[8] + m[3] * inv[12];
        if (det == 0.0f) {
            return null;
        }
        float invDet = 1.0f / det;
        for (int i = 0; i < 16; i++) {
            inv[i] *= invDet;
        }
        return inv;
    }
}
