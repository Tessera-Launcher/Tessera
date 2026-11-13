/*
 * Copyright (c) 2002-2008 LWJGL Project
 * All rights reserved. BSD-3-Clause; see the vector sources for the full text.
 */
package org.lwjgl;

/** The LWJGL 2 checked exception, kept so `catch (LWJGLException)` still works. */
public class LWJGLException extends Exception {
    private static final long serialVersionUID = 1L;

    public LWJGLException() {
        super();
    }

    public LWJGLException(String message) {
        super(message);
    }

    public LWJGLException(String message, Throwable cause) {
        super(message, cause);
    }

    public LWJGLException(Throwable cause) {
        super(cause);
    }
}
