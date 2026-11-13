package org.lwjgl.util.glu;

import java.nio.FloatBuffer;

import org.lwjgl.opengl.GL11;
import org.lwjgl.system.MemoryStack;

/** The two {@code gluProject} helpers 1.8.9 uses, with LWJGL 2's signatures. */
public final class Project {
    private Project() {}

    /** {@code gluPerspective}: multiplies the current matrix, like LWJGL 2 did. */
    public static void gluPerspective(float fovy, float aspect, float zNear, float zFar) {
        float radians = (float) (fovy * Math.PI / 180.0 / 2.0);
        float deltaZ = zFar - zNear;
        float sine = (float) Math.sin(radians);
        float cotangent = (float) Math.cos(radians) / sine;

        try (MemoryStack stack = MemoryStack.stackPush()) {
            FloatBuffer m = stack.mallocFloat(16);
            m.put(0, cotangent / aspect).put(5, cotangent)
                    .put(10, -(zFar + zNear) / deltaZ).put(11, -1.0f)
                    .put(14, -2.0f * zNear * zFar / deltaZ).put(15, 0.0f);
            GL11.glMultMatrixf(m);
        }
    }
}
