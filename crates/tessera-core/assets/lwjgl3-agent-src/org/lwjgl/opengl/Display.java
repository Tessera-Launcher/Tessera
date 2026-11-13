package org.lwjgl.opengl;

import java.nio.ByteBuffer;
import java.nio.IntBuffer;
import java.util.ArrayList;
import java.util.List;

import org.lwjgl.LWJGLException;
import org.lwjgl.input.Keyboard;
import org.lwjgl.input.Mouse;

import org.lwjgl.glfw.GLFWErrorCallback;
import org.lwjgl.glfw.GLFWImage;
import org.lwjgl.glfw.GLFWVidMode;

import static org.lwjgl.glfw.GLFW.*;
import static org.lwjgl.system.MemoryUtil.NULL;

/**
 * LWJGL 2's {@code Display}, implemented over GLFW 3 and LWJGL 3.
 *
 * This is the only part of the shim that does real work: it owns the window and
 * the GL context, feeds {@link Keyboard} and {@link Mouse} from GLFW callbacks,
 * and reproduces the LWJGL 2 surface 1.8.9 calls (swap, resize notification,
 * close request, fullscreen, vsync and the fps limiter).
 *
 * Everything stays on OpenGL. There is no Vulkan here on purpose: 1.8.9 draws
 * with the GL fixed-function pipeline, so the only "Vulkan" that could be
 * plugged in is a GL-over-Vulkan translation, which is a different project.
 */
public final class Display {
    private static long window = NULL;
    private static boolean created;
    private static boolean closeRequested;
    private static volatile boolean resized;
    private static volatile int width;
    private static volatile int height;
    private static DisplayMode mode = new DisplayMode(854, 480);
    private static String title = "Minecraft";
    private static boolean fullscreen;
    private static boolean resizable = true;
    private static boolean vsync;
    private static long lastSync;
    private static GLFWErrorCallback errorCallback;
    /** The icon Minecraft sets before the window exists. */
    private static ByteBuffer[] pendingIcons;

    private Display() {}

    public static void create() throws LWJGLException {
        create(new PixelFormat());
    }

    public static void create(PixelFormat pixelFormat) throws LWJGLException {
        System.err.println("[tessera-lwjgl3] Display.create start");
        if (created) {
            throw new LWJGLException("Display already created");
        }
        try {
            if (!glfwInit()) {
                throw new LWJGLException("Failed to initialise GLFW");
            }
            errorCallback = GLFWErrorCallback.create((code, description) ->
                    System.err.println("[GLFW] " + description));
            glfwSetErrorCallback(errorCallback);

            glfwDefaultWindowHints();
            glfwWindowHint(GLFW_VISIBLE, GLFW_FALSE);
            glfwWindowHint(GLFW_RESIZABLE, resizable ? GLFW_TRUE : GLFW_FALSE);
            if (pixelFormat.getDepthBits() > 0) {
                glfwWindowHint(GLFW_DEPTH_BITS, pixelFormat.getDepthBits());
            }
            if (pixelFormat.getStencilBits() > 0) {
                glfwWindowHint(GLFW_STENCIL_BITS, pixelFormat.getStencilBits());
            }
            if (pixelFormat.samples > 1) {
                glfwWindowHint(GLFW_SAMPLES, pixelFormat.samples);
            }

            long monitor = fullscreen ? glfwGetPrimaryMonitor() : NULL;
            window = glfwCreateWindow(mode.getWidth(), mode.getHeight(), title, monitor, NULL);
            if (window == NULL) {
                glfwTerminate();
                throw new LWJGLException("Failed to create window");
            }

            glfwMakeContextCurrent(window);
            GL.createCapabilities();
            GLContext.setCapabilities(new ContextCapabilities(GL.getCapabilities()));
            glfwSwapInterval(vsync ? 1 : 0);

            int[] w = new int[1];
            int[] h = new int[1];
            glfwGetFramebufferSize(window, w, h);
            width = w[0];
            height = h[0];

            if (pendingIcons != null) {
                applyIcon(pendingIcons);
                pendingIcons = null;
            }
            installCallbacks();
            Keyboard.install(window);
            Mouse.install(window);

            glfwShowWindow(window);
            created = true;
            resized = false;
            closeRequested = false;
            lastSync = System.nanoTime();
            System.err.println("[tessera-lwjgl3] Display.create ok " + width + "x" + height);
        } catch (Throwable t) {
            System.err.println("[tessera-lwjgl3] Display.create failed: " + t);
            t.printStackTrace();
            throw t instanceof LWJGLException ? (LWJGLException) t
                    : new LWJGLException("Display.create failed", t);
        }
    }

    private static void installCallbacks() {
        glfwSetWindowCloseCallback(window, win -> closeRequested = true);
        glfwSetFramebufferSizeCallback(window, (win, w, h) -> {
            width = w;
            height = h;
            resized = true;
        });
    }

    public static void update() {
        if (!created) {
            return;
        }
        glfwSwapBuffers(window);
        glfwPollEvents();
    }

    public static void destroy() {
        if (!created) {
            return;
        }
        Keyboard.uninstall();
        Mouse.uninstall();
        GL.setCapabilities(null);
        glfwDestroyWindow(window);
        window = NULL;
        glfwTerminate();
        if (errorCallback != null) {
            errorCallback.free();
            errorCallback = null;
        }
        created = false;
    }

    /** LWJGL 2's frame limiter: sleep until the next frame is due. */
    public static void sync(int fps) {
        if (fps <= 0 || !created) {
            return;
        }
        long frame = 1_000_000_000L / fps;
        long now = System.nanoTime();
        long wait = lastSync + frame - now;
        if (wait > 0) {
            try {
                Thread.sleep(wait / 1_000_000L, (int) (wait % 1_000_000L));
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
            }
        }
        lastSync = System.nanoTime();
    }

    public static boolean isCreated() {
        return created;
    }

    public static boolean isCloseRequested() {
        return closeRequested;
    }

    public static boolean isActive() {
        if (!created) {
            return false;
        }
        return glfwGetWindowAttrib(window, GLFW_ICONIFIED) == GLFW_FALSE;
    }

    public static boolean wasResized() {
        boolean was = resized;
        resized = false;
        return was;
    }

    public static int getWidth() {
        return width;
    }

    public static int getHeight() {
        return height;
    }

    public static long getHandle() {
        return window;
    }

    public static DisplayMode getDisplayMode() {
        return mode;
    }

    public static void setDisplayMode(DisplayMode displayMode) {
        mode = displayMode;
        if (created && !fullscreen) {
            glfwSetWindowSize(window, displayMode.getWidth(), displayMode.getHeight());
            resized = true;
        }
    }

    public static DisplayMode getDesktopDisplayMode() {
        if (!glfwInit()) {
            return new DisplayMode(1920, 1080);
        }
        GLFWVidMode video = glfwGetVideoMode(glfwGetPrimaryMonitor());
        if (video == null) {
            return new DisplayMode(mode.getWidth(), mode.getHeight());
        }
        return new DisplayMode(video.width(), video.height(), video.redBits()
                + video.greenBits() + video.blueBits(), video.refreshRate());
    }

    public static DisplayMode[] getAvailableDisplayModes() {
        if (!glfwInit()) {
            return new DisplayMode[] {getDesktopDisplayMode()};
        }
        GLFWVidMode.Buffer modes = glfwGetVideoModes(glfwGetPrimaryMonitor());
        if (modes == null) {
            return new DisplayMode[] {getDesktopDisplayMode()};
        }
        List<DisplayMode> out = new ArrayList<>();
        for (int i = 0; i < modes.limit(); i++) {
            modes.position(i);
            int bits = modes.redBits() + modes.greenBits() + modes.blueBits();
            out.add(new DisplayMode(modes.width(), modes.height(), bits, modes.refreshRate()));
        }
        return out.toArray(new DisplayMode[0]);
    }

    public static void setTitle(String newTitle) {
        title = newTitle;
        if (created) {
            glfwSetWindowTitle(window, newTitle);
        }
    }

    public static void setFullscreen(boolean wantFullscreen) {
        fullscreen = wantFullscreen;
        if (!created) {
            return;
        }
        long monitor = wantFullscreen ? glfwGetPrimaryMonitor() : NULL;
        glfwSetWindowMonitor(window, monitor, 0, 0, mode.getWidth(), mode.getHeight(),
                wantFullscreen ? GLFW_DONT_CARE : 0);
        resized = true;
    }

    public static void setResizable(boolean wantResizable) {
        resizable = wantResizable;
        if (created) {
            glfwSetWindowAttrib(window, GLFW_RESIZABLE,
                    wantResizable ? GLFW_TRUE : GLFW_FALSE);
        }
    }

    public static void setVSyncEnabled(boolean enabled) {
        vsync = enabled;
        if (created) {
            glfwMakeContextCurrent(window);
            glfwSwapInterval(enabled ? 1 : 0);
        }
    }

    public static void makeCurrent() {
        if (created) {
            glfwMakeContextCurrent(window);
        }
    }

    /**
     * The window icon, in LWJGL 2's format: square RGBA buffers.
     *
     * Minecraft passes 16x16 and 32x32; the size is derived from the buffer so a
     * caller that only has one icon still works.
     */
    public static int setIcon(ByteBuffer[] icons) {
        if (icons == null || icons.length == 0) {
            return 0;
        }
        if (!created) {
            // The game sets its icon before the window is created; LWJGL 2
            // accepted that, so the buffers are kept and applied on create.
            ByteBuffer[] copy = new ByteBuffer[icons.length];
            for (int i = 0; i < icons.length; i++) {
                ByteBuffer duplicate = icons[i].duplicate();
                ByteBuffer buffer = org.lwjgl.BufferUtils.createByteBuffer(duplicate.remaining());
                buffer.put(duplicate);
                buffer.flip();
                copy[i] = buffer;
            }
            pendingIcons = copy;
            return 1;
        }
        applyIcon(icons);
        return 1;
    }

    private static void applyIcon(ByteBuffer[] icons) {
        if (window == NULL) {
            return;
        }
        GLFWImage.Buffer images = GLFWImage.malloc(icons.length);
        for (int i = 0; i < icons.length; i++) {
            ByteBuffer icon = icons[i].duplicate();
            int pixels = icon.remaining() / 4;
            int side = (int) Math.round(Math.sqrt(pixels));
            if (side <= 0) {
                continue;
            }
            images.position(i);
            images.width(side);
            images.height(side);
            images.pixels(icon);
        }
        images.position(0);
        glfwSetWindowIcon(window, images);
        images.free();
    }

    /** The pixel scale of the monitor (HiDPI); 1.0 when the platform hides it. */
    public static float getPixelScaleFactor() {
        return 1.0f;
    }

    public static void setSwapInterval(int interval) {
        if (created) {
            glfwSwapInterval(interval);
        }
    }

    public static void processMessages() {
        glfwPollEvents();
    }
}
