package org.lwjgl.opengl;

/** The pixel format request the game passes to {@link Display#create(PixelFormat)}. */
public class PixelFormat {
    int alphaBits = 8;
    int depthBits = 8;
    int stencilBits;
    int samples;
    boolean stereo;

    public PixelFormat() {}

    public PixelFormat withDepthBits(int depthBits) {
        this.depthBits = depthBits;
        return this;
    }

    public int getDepthBits() {
        return depthBits;
    }

    public int getAlphaBits() {
        return alphaBits;
    }

    public int getStencilBits() {
        return stencilBits;
    }

    public boolean isStereo() {
        return stereo;
    }

    public PixelFormat withCoverageSamples(int samples) {
        this.samples = samples;
        return this;
    }

    public PixelFormat withSamples(int samples) {
        this.samples = samples;
        return this;
    }
}
