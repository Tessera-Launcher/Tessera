package org.lwjgl.opengl;

/**
 * The current context's capabilities, as LWJGL 2 exposed them.
 *
 * Filled in by {@link Display#create}; the game reads the fields on
 * {@link ContextCapabilities} (it is what tells it whether shaders, FBOs and
 * so on are available).
 */
public final class GLContext {
    private static volatile ContextCapabilities capabilities;

    private GLContext() {}

    public static ContextCapabilities getCapabilities() {
        ContextCapabilities current = capabilities;
        if (current == null) {
            throw new IllegalStateException("No OpenGL context found in the current thread.");
        }
        return current;
    }

    public static ContextCapabilities getCapabilitiesImpl() {
        return capabilities;
    }

    public static ContextCapabilities getThreadLocalCapabilities() {
        return capabilities;
    }

    public static void setCapabilities(ContextCapabilities contextCapabilities) {
        capabilities = contextCapabilities;
    }
}
