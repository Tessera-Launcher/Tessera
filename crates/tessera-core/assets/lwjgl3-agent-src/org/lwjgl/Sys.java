package org.lwjgl;

import java.awt.Desktop;
import java.net.URI;

/** The few {@code Sys} calls the game makes, answered without native code. */
public final class Sys {
    private static final long START = System.nanoTime();

    private Sys() {}

    /** Nanoseconds since an arbitrary point, monotonic. */
    public static long getTime() {
        return System.nanoTime();
    }

    /** Nanoseconds per timer tick. */
    public static long getTimerResolution() {
        return 1_000_000L;
    }

    public static String getVersion() {
        return "2.9.4 (LWJGL3 shim)";
    }

    public static boolean openURL(String url) {
        try {
            if (Desktop.isDesktopSupported()
                    && Desktop.getDesktop().isSupported(Desktop.Action.BROWSE)) {
                Desktop.getDesktop().browse(new URI(url));
                return true;
            }
        } catch (Throwable ignored) {
            // Fall through: the caller only wants to know whether to try again.
        }
        return false;
    }

    public static void initialize() {}
}
