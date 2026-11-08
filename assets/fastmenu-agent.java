package yaml.fastmenu;

import java.lang.instrument.Instrumentation;
import java.lang.reflect.Field;
import java.lang.reflect.Method;

/**
 * Skips the 26.x menu fades without touching the client jar.
 *
 * At startup Minecraft creates a LoadingOverlay (Mojang logo plus a one-second
 * fade) and then a TitleScreen with its own two-second widget fade. Resources
 * are already loaded when the overlay starts fading, so both waits are pure
 * presentation. This agent polls the public Gui screen/overlay API and moves
 * the private fade timestamps into the past, or clears TitleScreen.fading.
 *
 * It is reflective on purpose: 26.x class names are stable, but the agent must
 * not depend on Minecraft being present at agent-build time. All failures are
 * swallowed so a changed field is a no-op rather than a launch failure.
 */
public final class Agent {
    private Agent() {}

    public static void premain(String args, Instrumentation instrumentation) {
        Thread thread = new Thread(Agent::watch, "yaml-fastmenu");
        thread.setDaemon(true);
        thread.start();
    }

    private static void watch() {
        Object minecraft = null;
        for (int attempt = 0; attempt < 1200 && minecraft == null; attempt++) {
            try {
                Class<?> type = Class.forName("net.minecraft.client.Minecraft", false, Agent.class.getClassLoader());
                minecraft = type.getMethod("getInstance").invoke(null);
            } catch (Throwable ignored) {
                sleep(50);
            }
        }
        if (minecraft == null) {
            return;
        }

        Field guiField;
        try {
            guiField = minecraft.getClass().getField("gui");
        } catch (Throwable ignored) {
            return;
        }

        long quietSince = 0L;
        for (int tick = 0; tick < 12000; tick++) { // about two minutes
            try {
                Object gui = guiField.get(minecraft);
                Method screenMethod = gui.getClass().getMethod("screen");
                Method overlayMethod = gui.getClass().getMethod("overlay");
                Object screen = screenMethod.invoke(gui);
                Object overlay = overlayMethod.invoke(gui);
                boolean settled = true;

                if (screen != null) {
                    if (setBoolean(screen, "fading", false)) {
                        // A TitleScreen created with fading=true; the first
                        // frames are already full opacity now.
                    }
                    settled &= !"AccessibilityOnboardingScreen".equals(screen.getClass().getSimpleName());
                }

                if (overlay != null) {
                    String name = overlay.getClass().getSimpleName();
                    if (name.endsWith("LoadingOverlay") || name.equals("LoadingOverlay")) {
                        settled = false;
                        long now = System.currentTimeMillis();
                        // The overlay's own onFinish callback runs from tick()
                        // only when fadeOutStart is set; do not set it before
                        // that, or the resource-load callback is skipped.
                        long fadeOut = getLong(overlay, "fadeOutStart", -1L);
                        if (fadeOut > 0L) {
                            setLong(overlay, "fadeOutStart", now - 3000L);
                        }
                        if (getLong(overlay, "fadeInStart", -1L) == -1L) {
                            setLong(overlay, "fadeInStart", now - 3000L);
                        }
                    }
                }

                if (settled && overlay == null) {
                    if (quietSince == 0L) {
                        quietSince = System.currentTimeMillis();
                    } else if (System.currentTimeMillis() - quietSince > 1000L) {
                        return;
                    }
                } else {
                    quietSince = 0L;
                }
            } catch (Throwable ignored) {
                // Minecraft is not up yet, or a reflective field moved.
            }
            sleep(10);
        }
    }

    private static boolean setBoolean(Object target, String name, boolean value) {
        try {
            Field field = target.getClass().getDeclaredField(name);
            field.setAccessible(true);
            if (field.getBoolean(target) == value) {
                return false;
            }
            field.setBoolean(target, value);
            return true;
        } catch (Throwable ignored) {
            return false;
        }
    }

    private static long getLong(Object target, String name, long fallback) {
        try {
            Field field = target.getClass().getDeclaredField(name);
            field.setAccessible(true);
            return field.getLong(target);
        } catch (Throwable ignored) {
            return fallback;
        }
    }

    private static void setLong(Object target, String name, long value) {
        try {
            Field field = target.getClass().getDeclaredField(name);
            field.setAccessible(true);
            field.setLong(target, value);
        } catch (Throwable ignored) {
        }
    }

    private static void sleep(long millis) {
        try {
            Thread.sleep(millis);
        } catch (InterruptedException interrupted) {
            Thread.currentThread().interrupt();
        }
    }
}
