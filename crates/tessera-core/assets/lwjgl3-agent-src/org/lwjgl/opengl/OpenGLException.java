package org.lwjgl.opengl;

/** LWJGL 2's GL error exception. */
public class OpenGLException extends RuntimeException {
    private static final long serialVersionUID = 1L;

    public OpenGLException() {
        super();
    }

    public OpenGLException(String message) {
        super(message);
    }

    public OpenGLException(int glError) {
        this(createErrorMessage(glError));
    }

    private static String createErrorMessage(int glError) {
        return "OpenGL error " + glError;
    }
}
