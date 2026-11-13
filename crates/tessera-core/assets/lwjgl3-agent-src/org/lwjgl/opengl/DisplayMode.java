package org.lwjgl.opengl;

/** A display size, in LWJGL 2's shape. */
public class DisplayMode {
    private final int width;
    private final int height;
    private final int bpp;
    private final int freq;

    public DisplayMode(int width, int height) {
        this(width, height, 32, 60);
    }

    public DisplayMode(int width, int height, int bitsPerPixel, int frequency) {
        this.width = width;
        this.height = height;
        this.bpp = bitsPerPixel;
        this.freq = frequency;
    }

    public int getWidth() {
        return width;
    }

    public int getHeight() {
        return height;
    }

    public int getBitsPerPixel() {
        return bpp;
    }

    public int getFrequency() {
        return freq;
    }

    @Override
    public boolean equals(Object other) {
        if (!(other instanceof DisplayMode)) {
            return false;
        }
        DisplayMode mode = (DisplayMode) other;
        return width == mode.width && height == mode.height;
    }

    @Override
    public int hashCode() {
        return width * 31 + height;
    }

    @Override
    public String toString() {
        return width + "x" + height + "x" + bpp + "@" + freq;
    }
}
