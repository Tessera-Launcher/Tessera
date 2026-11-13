package org.lwjgl.openal;

/** LWJGL 2's OpenAL exception type; the game's sound library may catch it. */
public class OpenALException extends RuntimeException {
    private static final long serialVersionUID = 1L;

    public OpenALException() {
        super();
    }

    public OpenALException(String message) {
        super(message);
    }

    public OpenALException(int errorCode) {
        super("OpenAL error " + errorCode);
    }
}
