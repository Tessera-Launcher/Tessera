package tessera.lwjgl3;

import java.nio.ByteBuffer;
import org.lwjgl.openal.AL;
import org.lwjgl.openal.ALC;
import org.lwjgl.openal.ALC10;
import org.lwjgl.openal.ALCCapabilities;

/** LWJGL 2 API that has no LWJGL 3 equivalent under the same name. */
public final class Bridge {
    private Bridge() {}

    /** LWJGL 2 took the shader source as a direct ByteBuffer. */
    public static void glShaderSourceBuffer(int shader, ByteBuffer source) {
        byte[] data = new byte[source.remaining()];
        source.duplicate().get(data);
        org.lwjgl.opengl.GL20.glShaderSource(shader, new String(data));
    }

    private static long device;
    private static long context;
    private static boolean ready;

    /** LWJGL 2's {@code AL.create()} opened the default device and context. */
    public static void alCreate() {
        if (ready) {
            return;
        }
        device = ALC10.alcOpenDevice((ByteBuffer) null);
        if (device == 0L) {
            return;
        }
        ALCCapabilities alcCapabilities = ALC.createCapabilities(device);
        context = ALC10.alcCreateContext(device, (java.nio.IntBuffer) null);
        if (context == 0L) {
            ALC10.alcCloseDevice(device);
            return;
        }
        ALC10.alcMakeContextCurrent(context);
        AL.createCapabilities(alcCapabilities);
        ready = true;
    }

    /** LWJGL 2's {@code AL.destroy()} closed them again. */
    public static void alDestroy() {
        if (!ready) {
            return;
        }
        ALC10.alcMakeContextCurrent(0L);
        ALC10.alcDestroyContext(context);
        ALC10.alcCloseDevice(device);
        ready = false;
    }

    /** LWJGL 2 had a process-wide "is there a context" flag. */
    public static boolean alIsCreated() {
        return ready;
    }
}
