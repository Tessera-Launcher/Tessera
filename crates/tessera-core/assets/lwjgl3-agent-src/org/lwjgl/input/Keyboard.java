package org.lwjgl.input;

import org.lwjgl.LWJGLException;

import static org.lwjgl.glfw.GLFW.*;

/**
 * LWJGL 2's keyboard: the same constants, the same event queue, fed by GLFW.
 *
 * The game walks the queue with {@link #next()} and reads the event with
 * {@link #getEventKey()}, {@link #getEventKeyState()} and
 * {@link #getEventCharacter()}. Character events come through with key 0, which
 * is what the game's typed-character path expects.
 */
public final class Keyboard {
    public static final int KEYBOARD_SIZE = 256;
    public static final int CHAR_NONE = '\0';

    public static final int KEY_NONE = 0x00;
    public static final int KEY_ESCAPE = 0x01;
    public static final int KEY_1 = 0x02;
    public static final int KEY_2 = 0x03;
    public static final int KEY_3 = 0x04;
    public static final int KEY_4 = 0x05;
    public static final int KEY_5 = 0x06;
    public static final int KEY_6 = 0x07;
    public static final int KEY_7 = 0x08;
    public static final int KEY_8 = 0x09;
    public static final int KEY_9 = 0x0A;
    public static final int KEY_0 = 0x0B;
    public static final int KEY_MINUS = 0x0C;
    public static final int KEY_EQUALS = 0x0D;
    public static final int KEY_BACK = 0x0E;
    public static final int KEY_TAB = 0x0F;
    public static final int KEY_Q = 0x10;
    public static final int KEY_W = 0x11;
    public static final int KEY_E = 0x12;
    public static final int KEY_R = 0x13;
    public static final int KEY_T = 0x14;
    public static final int KEY_Y = 0x15;
    public static final int KEY_U = 0x16;
    public static final int KEY_I = 0x17;
    public static final int KEY_O = 0x18;
    public static final int KEY_P = 0x19;
    public static final int KEY_LBRACKET = 0x1A;
    public static final int KEY_RBRACKET = 0x1B;
    public static final int KEY_RETURN = 0x1C;
    public static final int KEY_LCONTROL = 0x1D;
    public static final int KEY_A = 0x1E;
    public static final int KEY_S = 0x1F;
    public static final int KEY_D = 0x20;
    public static final int KEY_F = 0x21;
    public static final int KEY_G = 0x22;
    public static final int KEY_H = 0x23;
    public static final int KEY_J = 0x24;
    public static final int KEY_K = 0x25;
    public static final int KEY_L = 0x26;
    public static final int KEY_SEMICOLON = 0x27;
    public static final int KEY_APOSTROPHE = 0x28;
    public static final int KEY_GRAVE = 0x29;
    public static final int KEY_LSHIFT = 0x2A;
    public static final int KEY_BACKSLASH = 0x2B;
    public static final int KEY_Z = 0x2C;
    public static final int KEY_X = 0x2D;
    public static final int KEY_C = 0x2E;
    public static final int KEY_V = 0x2F;
    public static final int KEY_B = 0x30;
    public static final int KEY_N = 0x31;
    public static final int KEY_M = 0x32;
    public static final int KEY_COMMA = 0x33;
    public static final int KEY_PERIOD = 0x34;
    public static final int KEY_SLASH = 0x35;
    public static final int KEY_RSHIFT = 0x36;
    public static final int KEY_MULTIPLY = 0x37;
    public static final int KEY_LMENU = 0x38;
    public static final int KEY_SPACE = 0x39;
    public static final int KEY_CAPITAL = 0x3A;
    public static final int KEY_F1 = 0x3B;
    public static final int KEY_F2 = 0x3C;
    public static final int KEY_F3 = 0x3D;
    public static final int KEY_F4 = 0x3E;
    public static final int KEY_F5 = 0x3F;
    public static final int KEY_F6 = 0x40;
    public static final int KEY_F7 = 0x41;
    public static final int KEY_F8 = 0x42;
    public static final int KEY_F9 = 0x43;
    public static final int KEY_F10 = 0x44;
    public static final int KEY_NUMLOCK = 0x45;
    public static final int KEY_SCROLL = 0x46;
    public static final int KEY_NUMPAD7 = 0x47;
    public static final int KEY_NUMPAD8 = 0x48;
    public static final int KEY_NUMPAD9 = 0x49;
    public static final int KEY_SUBTRACT = 0x4A;
    public static final int KEY_NUMPAD4 = 0x4B;
    public static final int KEY_NUMPAD5 = 0x4C;
    public static final int KEY_NUMPAD6 = 0x4D;
    public static final int KEY_ADD = 0x4E;
    public static final int KEY_NUMPAD1 = 0x4F;
    public static final int KEY_NUMPAD2 = 0x50;
    public static final int KEY_NUMPAD3 = 0x51;
    public static final int KEY_NUMPAD0 = 0x52;
    public static final int KEY_DECIMAL = 0x53;
    public static final int KEY_F11 = 0x57;
    public static final int KEY_F12 = 0x58;
    public static final int KEY_F13 = 0x64;
    public static final int KEY_F14 = 0x65;
    public static final int KEY_F15 = 0x66;
    public static final int KEY_F16 = 0x67;
    public static final int KEY_F17 = 0x68;
    public static final int KEY_F18 = 0x69;
    public static final int KEY_KANA = 0x70;
    public static final int KEY_F19 = 0x71;
    public static final int KEY_CONVERT = 0x79;
    public static final int KEY_NOCONVERT = 0x7B;
    public static final int KEY_YEN = 0x7D;
    public static final int KEY_NUMPADEQUALS = 0x8D;
    public static final int KEY_CIRCUMFLEX = 0x90;
    public static final int KEY_AT = 0x91;
    public static final int KEY_COLON = 0x92;
    public static final int KEY_UNDERLINE = 0x93;
    public static final int KEY_KANJI = 0x94;
    public static final int KEY_STOP = 0x95;
    public static final int KEY_AX = 0x96;
    public static final int KEY_UNLABELED = 0x97;
    public static final int KEY_NUMPADENTER = 0x9C;
    public static final int KEY_RCONTROL = 0x9D;
    public static final int KEY_SECTION = 0xA7;
    public static final int KEY_NUMPADCOMMA = 0xB3;
    public static final int KEY_DIVIDE = 0xB5;
    public static final int KEY_SYSRQ = 0xB7;
    public static final int KEY_RMENU = 0xB8;
    public static final int KEY_FUNCTION = 0xC4;
    public static final int KEY_PAUSE = 0xC5;
    public static final int KEY_HOME = 0xC7;
    public static final int KEY_UP = 0xC8;
    public static final int KEY_PRIOR = 0xC9;
    public static final int KEY_LEFT = 0xCB;
    public static final int KEY_RIGHT = 0xCD;
    public static final int KEY_END = 0xCF;
    public static final int KEY_DOWN = 0xD0;
    public static final int KEY_NEXT = 0xD1;
    public static final int KEY_INSERT = 0xD2;
    public static final int KEY_DELETE = 0xD3;
    public static final int KEY_CLEAR = 0xDA;
    public static final int KEY_LMETA = 0xDB;
    public static final int KEY_RMETA = 0xDC;
    public static final int KEY_APPS = 0xDD;
    public static final int KEY_POWER = 0xDE;
    public static final int KEY_SLEEP = 0xDF;

    private static final class Event {
        final int key;
        final boolean state;
        final char character;
        final boolean repeat;

        Event(int key, boolean state, char character, boolean repeat) {
            this.key = key;
            this.state = state;
            this.character = character;
            this.repeat = repeat;
        }
    }

    private static final java.util.ArrayDeque<Event> EVENTS = new java.util.ArrayDeque<>();
    private static final boolean[] DOWN = new boolean[KEYBOARD_SIZE];
    private static boolean created;
    private static boolean repeatEvents;
    private static Event current;
    private static long window;

    private Keyboard() {}

    /** Called by {@link org.lwjgl.opengl.Display} once the window exists. */
    public static void install(long windowHandle) {
        window = windowHandle;
        created = true;
        java.util.Arrays.fill(DOWN, false);
        synchronized (EVENTS) {
            EVENTS.clear();
        }

        glfwSetKeyCallback(window, (win, glfwKey, scancode, action, mods) -> {
            int key = translate(glfwKey);
            if (key == KEY_NONE) {
                return;
            }
            boolean pressed = action != GLFW_RELEASE;
            boolean repeat = action == GLFW_REPEAT;
            if (key < DOWN.length) {
                DOWN[key] = pressed;
            }
            if (repeat && !repeatEvents) {
                return;
            }
            push(new Event(key, pressed, (char) CHAR_NONE, repeat));
        });

        glfwSetCharCallback(window, (win, codepoint) -> {
            if (codepoint < 0x10000) {
                push(new Event(0, true, (char) codepoint, false));
            }
        });
    }

    /** Called by {@link org.lwjgl.opengl.Display} right before the window goes. */
    public static void uninstall() {
        created = false;
        window = 0L;
        java.util.Arrays.fill(DOWN, false);
        synchronized (EVENTS) {
            EVENTS.clear();
        }
    }

    private static void push(Event event) {
        synchronized (EVENTS) {
            EVENTS.add(event);
        }
    }

    public static void create() throws LWJGLException {
        // The Display already installed the callbacks; nothing else to do.
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

    public static int getEventKey() {
        return current == null ? KEY_NONE : current.key;
    }

    public static boolean getEventKeyState() {
        return current != null && current.state;
    }

    public static char getEventCharacter() {
        return current == null ? (char) CHAR_NONE : current.character;
    }

    public static boolean isRepeatEvent() {
        return current != null && current.repeat;
    }

    public static long getEventNanoseconds() {
        return System.nanoTime();
    }

    public static boolean isKeyDown(int key) {
        return key >= 0 && key < DOWN.length && DOWN[key];
    }

    public static void enableRepeatEvents(boolean enable) {
        repeatEvents = enable;
    }

    public static boolean areRepeatEventsEnabled() {
        return repeatEvents;
    }

    /** LWJGL 2's name for a key code (`A`, `LSHIFT`, …). */
    public static String getKeyName(int key) {
        if (key == KEY_NONE) {
            return "NONE";
        }
        for (java.lang.reflect.Field field : Keyboard.class.getFields()) {
            try {
                if (field.getType() == int.class
                        && field.getName().startsWith("KEY_")
                        && field.getInt(null) == key) {
                    return field.getName().substring(4);
                }
            } catch (IllegalAccessException ignored) {
                // Not reachable for public static finals.
            }
        }
        return "UNKNOWN";
    }

    /** Count of usable key codes, as LWJGL 2 reported it. */
    public static int getKeyCount() {
        return KEYBOARD_SIZE;
    }

    /** GLFW's key token to LWJGL 2's key code. */
    private static int translate(int glfwKey) {
        if (glfwKey >= GLFW_KEY_A && glfwKey <= GLFW_KEY_Z) {
            // LWJGL 2 uses the X11/XKB keycodes, which differ per letter.
            int index = glfwKey - GLFW_KEY_A;
            return LETTERS[index];
        }
        if (glfwKey >= GLFW_KEY_0 && glfwKey <= GLFW_KEY_9) {
            int digit = glfwKey - GLFW_KEY_0;
            return digit == 0 ? KEY_0 : KEY_1 + digit - 1;
        }
        if (glfwKey >= GLFW_KEY_F1 && glfwKey <= GLFW_KEY_F15) {
            return KEY_F1 + (glfwKey - GLFW_KEY_F1);
        }
        if (glfwKey >= GLFW_KEY_KP_0 && glfwKey <= GLFW_KEY_KP_9) {
            return KEY_NUMPAD0 + (glfwKey - GLFW_KEY_KP_0);
        }
        switch (glfwKey) {
            case GLFW_KEY_ESCAPE: return KEY_ESCAPE;
            case GLFW_KEY_ENTER: return KEY_RETURN;
            case GLFW_KEY_TAB: return KEY_TAB;
            case GLFW_KEY_BACKSPACE: return KEY_BACK;
            case GLFW_KEY_INSERT: return KEY_INSERT;
            case GLFW_KEY_DELETE: return KEY_DELETE;
            case GLFW_KEY_RIGHT: return KEY_RIGHT;
            case GLFW_KEY_LEFT: return KEY_LEFT;
            case GLFW_KEY_DOWN: return KEY_DOWN;
            case GLFW_KEY_UP: return KEY_UP;
            case GLFW_KEY_PAGE_UP: return KEY_PRIOR;
            case GLFW_KEY_PAGE_DOWN: return KEY_NEXT;
            case GLFW_KEY_HOME: return KEY_HOME;
            case GLFW_KEY_END: return KEY_END;
            case GLFW_KEY_CAPS_LOCK: return KEY_CAPITAL;
            case GLFW_KEY_SCROLL_LOCK: return KEY_SCROLL;
            case GLFW_KEY_NUM_LOCK: return KEY_NUMLOCK;
            case GLFW_KEY_PRINT_SCREEN: return KEY_SYSRQ;
            case GLFW_KEY_PAUSE: return KEY_PAUSE;
            case GLFW_KEY_SPACE: return KEY_SPACE;
            case GLFW_KEY_LEFT_SHIFT: return KEY_LSHIFT;
            case GLFW_KEY_RIGHT_SHIFT: return KEY_RSHIFT;
            case GLFW_KEY_LEFT_CONTROL: return KEY_LCONTROL;
            case GLFW_KEY_RIGHT_CONTROL: return KEY_RCONTROL;
            case GLFW_KEY_LEFT_ALT: return KEY_LMENU;
            case GLFW_KEY_RIGHT_ALT: return KEY_RMENU;
            case GLFW_KEY_LEFT_SUPER: return KEY_LMETA;
            case GLFW_KEY_RIGHT_SUPER: return KEY_RMETA;
            case GLFW_KEY_MINUS: return KEY_MINUS;
            case GLFW_KEY_EQUAL: return KEY_EQUALS;
            case GLFW_KEY_LEFT_BRACKET: return KEY_LBRACKET;
            case GLFW_KEY_RIGHT_BRACKET: return KEY_RBRACKET;
            case GLFW_KEY_BACKSLASH: return KEY_BACKSLASH;
            case GLFW_KEY_SEMICOLON: return KEY_SEMICOLON;
            case GLFW_KEY_APOSTROPHE: return KEY_APOSTROPHE;
            case GLFW_KEY_GRAVE_ACCENT: return KEY_GRAVE;
            case GLFW_KEY_COMMA: return KEY_COMMA;
            case GLFW_KEY_PERIOD: return KEY_PERIOD;
            case GLFW_KEY_SLASH: return KEY_SLASH;
            case GLFW_KEY_KP_ENTER: return KEY_NUMPADENTER;
            case GLFW_KEY_KP_DECIMAL: return KEY_DECIMAL;
            case GLFW_KEY_KP_DIVIDE: return KEY_DIVIDE;
            case GLFW_KEY_KP_MULTIPLY: return KEY_MULTIPLY;
            case GLFW_KEY_KP_SUBTRACT: return KEY_SUBTRACT;
            case GLFW_KEY_KP_ADD: return KEY_ADD;
            default: return KEY_NONE;
        }
    }

    /** LWJGL 2's key code for A..Z, in GLFW's A..Z order. */
    private static final int[] LETTERS = {
        KEY_A, KEY_B, KEY_C, KEY_D, KEY_E, KEY_F, KEY_G, KEY_H, KEY_I, KEY_J, KEY_K, KEY_L,
        KEY_M, KEY_N, KEY_O, KEY_P, KEY_Q, KEY_R, KEY_S, KEY_T, KEY_U, KEY_V, KEY_W, KEY_X,
        KEY_Y, KEY_Z,
    };
}
