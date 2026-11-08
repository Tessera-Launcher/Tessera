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
 * presentation. This agent polls the public Gui screen/overlay API, skips the
 * logo fade-in, removes the overlay once tick() has run onFinish, and clears
 * TitleScreen.fading.
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
                if (gui == null) {
                    // On some 26.3+ builds getInstance() becomes non-null
                    // before the final gui field is assigned. Do not treat
                    // that as "menu settled" and exit the watcher.
                    sleep(10);
                    continue;
                }
                // 26.2+ keeps screen/overlay in Gui and exposes screen()/overlay().
                // 26.1 keeps them in Minecraft itself (screen is public, overlay
                // is private) and Gui has neither method. Read both shapes.
                Object screen = readScreen(gui, minecraft);
                Object overlay = readOverlay(gui, minecraft);
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
                        // The overlay's own tick() calls onFinish() and only
                        // then sets fadeOutStart. Once that has happened the
                        // screen is initialized; remove the overlay directly
                        // instead of rewriting its timestamp. Writing
                        // fadeOutStart is what broke 26.2 when an AOT cache
                        // compiled the overlay: the cached code kept seeing a
                        // stale timestamp and never reached the >=2s removal.
                        long fadeOut = getLong(overlay, "fadeOutStart", -1L);
                        if (fadeOut > 0L && overlay != fadeOutFixed) {
                            if (removeOverlay(gui, minecraft, overlay)) {
                                fadeOutFixed = overlay;
                                if (Boolean.getBoolean("yaml.fastmenu.debug")) {
                                    System.out.println("[yaml-fastmenu] removed LoadingOverlay");
                                }
                            }
                        }
                        // Skip the 500ms logo fade-in as well. This timestamp
                        // is only read by the overlay's own render code.
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

    /// 26.2+ has Gui.setOverlay; 26.1 has Minecraft.setOverlay.
    private static boolean removeOverlay(Object gui, Object minecraft, Object overlay) {
        for (Object target : new Object[] { gui, minecraft }) {
            Method setter = setOverlayMethod(target.getClass(), overlay);
            if (setter == null) {
                continue;
            }
            try {
                setter.invoke(target, new Object[] { null });
                return true;
            } catch (Throwable ignored) {
                // Try the other owner.
            }
        }
        return false;
    }

    private static Method setOverlayMethod(Class<?> type, Object overlay) {
        for (Method method : type.getMethods()) {
            if (!method.getName().equals("setOverlay")) {
                continue;
            }
            Class<?>[] parameters = method.getParameterTypes();
            if (parameters.length == 1 && parameters[0].isInstance(overlay)) {
                return method;
            }
        }
        return null;
    }

    /// 26.2+ exposes Gui.screen()/overlay(); 26.1 stores both on Minecraft.
    private static Object readScreen(Object gui, Object minecraft) {
        return callOrField(gui, minecraft, "screen");
    }

    private static Object readOverlay(Object gui, Object minecraft) {
        return callOrField(gui, minecraft, "overlay");
    }

    private static Object callOrField(Object gui, Object minecraft, String name) {
        try {
            Method method = gui.getClass().getMethod(name);
            return method.invoke(gui);
        } catch (NoSuchMethodException ignored) {
            // Older 26.x: the value lives directly on Minecraft.
        } catch (Throwable ignored) {
            return null;
        }
        return readField(minecraft, name);
    }

    private static Object readField(Object target, String name) {
        try {
            Field field = findField(target.getClass(), name);
            if (field == null) {
                return null;
            }
            field.setAccessible(true);
            return field.get(target);
        } catch (Throwable ignored) {
            return null;
        }
    }

    private static Field findField(Class<?> type, String name) {
        for (Class<?> current = type; current != null; current = current.getSuperclass()) {
            try {
                return current.getDeclaredField(name);
            } catch (NoSuchFieldException ignored) {
                // Keep walking up.
            }
        }
        return null;
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
            Field field = findField(target.getClass(), name);
            if (field == null) {
                return false;
            }
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
            Field field = findField(target.getClass(), name);
            if (field == null) {
                return fallback;
            }
            field.setAccessible(true);
            return field.getLong(target);
        } catch (Throwable ignored) {
            return fallback;
        }
    }

    private static void setLong(Object target, String name, long value) {
        try {
            Field field = findField(target.getClass(), name);
            if (field == null) {
                return;
            }
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
