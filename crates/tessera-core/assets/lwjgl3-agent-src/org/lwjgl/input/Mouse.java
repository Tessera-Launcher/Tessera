package org.lwjgl.input;

import java.util.ArrayDeque;

import org.lwjgl.LWJGLException;

import static org.lwjgl.glfw.GLFW.*;

/**
 * LWJGL 2's mouse, fed by GLFW.
 *
 * LWJGL 2 works in a bottom-left origin, reports integer deltas per frame, and
 * delivers button and wheel events through its own queue; the game relies on all
 * three, so the conversion happens here rather than in the game.
 */
public final class Mouse {
    private static final class Event {
        final int button;
        final boolean state;
        final int x;
        final int y;
        final int wheel;

        Event(int button, boolean state, int x, int y, int wheel) {
            this.button = button;
            this.state = state;
            this.x = x;
            this.y = y;
            this.wheel = wheel;
        }
    }

    private static final ArrayDeque<Event> EVENTS = new ArrayDeque<>();
    private static final boolean[] BUTTONS = new boolean[8];
    private static boolean created;
    private static boolean grabbed;
    private static long window;

    private static double lastX;
    private static double lastY;
    private static boolean haveLast;
    private static int dx;
    private static int dy;
    private static int dwheel;
    private static Event current;

    private Mouse() {}

    /** Called by {@link org.lwjgl.opengl.Display} once the window exists. */
    public static void install(long windowHandle) {
        window = windowHandle;
        created = true;
        synchronized (EVENTS) {
            EVENTS.clear();
        }
        resetPosition();

        glfwSetCursorPosCallback(window, (win, x, y) -> {
            synchronized (Mouse.class) {
                if (haveLast) {
                    dx += (int) Math.round(x - lastX);
                    // GLFW's y grows downwards, LWJGL 2's grows upwards.
                    dy -= (int) Math.round(y - lastY);
                }
                lastX = x;
                lastY = y;
                haveLast = true;
            }
        });

        glfwSetMouseButtonCallback(window, (win, button, action, mods) -> {
            int index = button == GLFW_MOUSE_BUTTON_LEFT ? 0
                    : button == GLFW_MOUSE_BUTTON_RIGHT ? 1
                    : button == GLFW_MOUSE_BUTTON_MIDDLE ? 2
                    : button;
            boolean pressed = action == GLFW_PRESS;
            if (index >= 0 && index < BUTTONS.length) {
                BUTTONS[index] = pressed;
            }
            int x = (int) Math.round(getCursorX());
            int y = (int) Math.round(getCursorY());
            synchronized (EVENTS) {
                EVENTS.add(new Event(index, pressed, x, y, 0));
            }
        });

        glfwSetScrollCallback(window, (win, xOffset, yOffset) -> {
            int wheel = (int) Math.round(yOffset) * 120;
            synchronized (Mouse.class) {
                dwheel += wheel;
            }
            synchronized (EVENTS) {
                EVENTS.add(new Event(-1, false, (int) Math.round(getCursorX()),
                        (int) Math.round(getCursorY()), wheel));
            }
        });
    }

    public static void uninstall() {
        created = false;
        window = 0L;
        grabbed = false;
        synchronized (EVENTS) {
            EVENTS.clear();
        }
    }

    private static void resetPosition() {
        if (window == 0L) {
            return;
        }
        double[] x = new double[1];
        double[] y = new double[1];
        glfwGetCursorPos(window, x, y);
        synchronized (Mouse.class) {
            lastX = x[0];
            lastY = y[0];
            haveLast = true;
            dx = 0;
            dy = 0;
        }
    }

    private static double getCursorX() {
        double[] x = new double[1];
        double[] y = new double[1];
        glfwGetCursorPos(window, x, y);
        return x[0];
    }

    private static double getCursorY() {
        double[] x = new double[1];
        double[] y = new double[1];
        glfwGetCursorPos(window, x, y);
        int[] h = new int[1];
        glfwGetFramebufferSize(window, null, h);
        return h[0] - y[0];
    }

    public static void create() throws LWJGLException {
        created = true;
    }

    public static void destroy() {
        uninstall();
    }

    public static boolean isCreated() {
        return created;
    }

    public static void poll() {
        glfwPollEvents();
    }

    public static boolean next() {
        synchronized (EVENTS) {
            current = EVENTS.poll();
        }
        return current != null;
    }

    public static int getEventButton() {
        return current == null ? -1 : current.button;
    }

    public static boolean getEventButtonState() {
        return current != null && current.state;
    }

    public static int getEventX() {
        return current == null ? 0 : current.x;
    }

    public static int getEventY() {
        return current == null ? 0 : current.y;
    }

    public static int getEventDWheel() {
        return current == null ? 0 : current.wheel;
    }

    public static long getEventNanoseconds() {
        return System.nanoTime();
    }

    public static int getX() {
        return (int) Math.round(getCursorX());
    }

    public static int getY() {
        return (int) Math.round(getCursorY());
    }

    /** Horizontal movement since the last read. */
    public static int getDX() {
        synchronized (Mouse.class) {
            int value = dx;
            dx = 0;
            return value;
        }
    }

    /** Vertical movement since the last read (up is positive). */
    public static int getDY() {
        synchronized (Mouse.class) {
            int value = dy;
            dy = 0;
            return value;
        }
    }

    public static int getDWheel() {
        synchronized (Mouse.class) {
            int value = dwheel;
            dwheel = 0;
            return value;
        }
    }

    public static boolean isButtonDown(int button) {
        return button >= 0 && button < BUTTONS.length && BUTTONS[button];
    }

    public static void setGrabbed(boolean grab) {
        if (window == 0L) {
            return;
        }
        grabbed = grab;
        glfwSetInputMode(window, GLFW_CURSOR, grab ? GLFW_CURSOR_DISABLED : GLFW_CURSOR_NORMAL);
        resetPosition();
    }

    public static boolean isGrabbed() {
        return grabbed;
    }

    public static void setCursorPosition(int x, int y) {
        if (window == 0L) {
            return;
        }
        int[] h = new int[1];
        glfwGetFramebufferSize(window, null, h);
        glfwSetCursorPos(window, x, h[0] - y);
    }

    public static boolean isInsideWindow() {
        if (window == 0L) {
            return false;
        }
        return glfwGetWindowAttrib(window, GLFW_HOVERED) == GLFW_TRUE;
    }
}
