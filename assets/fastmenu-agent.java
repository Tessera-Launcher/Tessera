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
        if (Boolean.getBoolean("yaml.fastmenu.debug")) {
            System.out.println("[yaml-fastmenu] premain");
        }
        Thread thread = new Thread(Agent::watch, "yaml-fastmenu");
        thread.setDaemon(true);
        thread.start();
    }

    private static void watch() {
        Object minecraft = null;
        for (int attempt = 0; attempt < 1200 && minecraft == null; attempt++) {
            try {
                Class<?> type = Class.forName("net.minecraft.client.Minecraft", false, ClassLoader.getSystemClassLoader());
                minecraft = type.getMethod("getInstance").invoke(null);
            } catch (Throwable failure) {
                if (Boolean.getBoolean("yaml.fastmenu.debug") && attempt % 20 == 0) {
                    System.out.println("[yaml-fastmenu] wait: " + failure);
                }
            }
            if (minecraft == null) {
                sleep(50);
            }
        }
        if (minecraft == null) {
            if (Boolean.getBoolean("yaml.fastmenu.debug")) {
                System.out.println("[yaml-fastmenu] Minecraft not found");
            }
            return;
        }

        Field guiField;
        try {
            guiField = minecraft.getClass().getField("gui");
        } catch (Throwable ignored) {
            return;
        }

        long quietSince = 0L;
        Object fadeInFixed = null;
        Object fadeOutFixed = null;
        for (int tick = 0; tick < 12000; tick++) { // about two minutes
            try {
                Object gui = guiField.get(minecraft);
                Method screenMethod = gui.getClass().getMethod("screen");
                Method overlayMethod = gui.getClass().getMethod("overlay");
                Object screen = screenMethod.invoke(gui);
                Object overlay = overlayMethod.invoke(gui);
                boolean settled = true;

                if (screen != null) {
                    if (setBoolean(screen, "fading", false) && Boolean.getBoolean("yaml.fastmenu.debug")) {
                        System.out.println("[yaml-fastmenu] cleared " + screen.getClass().getSimpleName() + ".fading");
                    }
                    settled &= !"AccessibilityOnboardingScreen".equals(screen.getClass().getSimpleName());
                }

                if (overlay != null) {
                    String name = overlay.getClass().getSimpleName();
                    if (name.endsWith("LoadingOverlay") || name.equals("LoadingOverlay")) {
                        settled = false;
                        long now = utilMillis();
                        // The overlay's own onFinish callback runs from tick()
                        // only when fadeOutStart is set; do not set it before
                        // that, or the resource-load callback is skipped.
                        long fadeOut = getLong(overlay, "fadeOutStart", -1L);
                        if (now >= 0L && fadeOut > 0L && overlay != fadeOutFixed) {
                            // LoadingOverlay compares against Util.getMillis(),
                            // which is SDL's tick clock, not System.nanoTime().
                            setLong(overlay, "fadeOutStart", now - 3000L);
                            fadeOutFixed = overlay;
                            if (Boolean.getBoolean("yaml.fastmenu.debug")) {
                                System.out.println("[yaml-fastmenu] cleared LoadingOverlay.fadeOutStart");
                            }
                        }
                        if (now >= 0L && getLong(overlay, "fadeInStart", -1L) == -1L && overlay != fadeInFixed) {
                            setLong(overlay, "fadeInStart", now - 3000L);
                            fadeInFixed = overlay;
                            if (Boolean.getBoolean("yaml.fastmenu.debug")) {
                                System.out.println("[yaml-fastmenu] cleared LoadingOverlay.fadeInStart");
                            }
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

    private static Method utilMillis;

    private static long utilMillis() {
        try {
            if (utilMillis == null) {
                utilMillis = Class.forName("net.minecraft.util.Util", false, ClassLoader.getSystemClassLoader())
                    .getMethod("getMillis");
            }
            return (Long) utilMillis.invoke(null);
        } catch (Throwable failure) {
            // Never mix System.nanoTime with Minecraft's SDL tick clock: a
            // future timestamp makes fadeOutAnim negative and freezes the
            // LoadingOverlay forever. If Util is unavailable, leave the
            // overlay alone instead.
            return -1L;
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
