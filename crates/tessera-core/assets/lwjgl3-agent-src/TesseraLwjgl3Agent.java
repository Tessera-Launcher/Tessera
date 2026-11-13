import java.lang.instrument.ClassFileTransformer;
import java.lang.instrument.Instrumentation;
import java.security.ProtectionDomain;
import java.util.HashMap;
import java.util.Map;
import org.objectweb.asm.*;

/** Rewrites the LWJGL 2 call sites that LWJGL 3 spells differently. */
public final class TesseraLwjgl3Agent {
    private static final String BRIDGE = "tessera/lwjgl3/Bridge";

    // Generated from the 1.8.9 class files: every LWJGL 2 name the game uses
        // that LWJGL 3 spells differently, checked against the 3.4.3 jars.
    static final String[][] RENAMES = new String[][] {
        {"org/lwjgl/openal/AL10", "alListener", "(ILjava/nio/FloatBuffer;)V", "alListenerfv"},
        {"org/lwjgl/openal/AL10", "alSource", "(IILjava/nio/FloatBuffer;)V", "alSourcefv"},
        {"org/lwjgl/openal/AL10", "alSourceStop", "(Ljava/nio/IntBuffer;)V", "alSourceStopv"},
        {"org/lwjgl/opengl/ARBShaderObjects", "glUniform1ARB", "(ILjava/nio/FloatBuffer;)V", "glUniform1fvARB"},
        {"org/lwjgl/opengl/ARBShaderObjects", "glUniform1ARB", "(ILjava/nio/IntBuffer;)V", "glUniform1ivARB"},
        {"org/lwjgl/opengl/ARBShaderObjects", "glUniform2ARB", "(ILjava/nio/FloatBuffer;)V", "glUniform2fvARB"},
        {"org/lwjgl/opengl/ARBShaderObjects", "glUniform2ARB", "(ILjava/nio/IntBuffer;)V", "glUniform2ivARB"},
        {"org/lwjgl/opengl/ARBShaderObjects", "glUniform3ARB", "(ILjava/nio/FloatBuffer;)V", "glUniform3fvARB"},
        {"org/lwjgl/opengl/ARBShaderObjects", "glUniform3ARB", "(ILjava/nio/IntBuffer;)V", "glUniform3ivARB"},
        {"org/lwjgl/opengl/ARBShaderObjects", "glUniform4ARB", "(ILjava/nio/FloatBuffer;)V", "glUniform4fvARB"},
        {"org/lwjgl/opengl/ARBShaderObjects", "glUniform4ARB", "(ILjava/nio/IntBuffer;)V", "glUniform4ivARB"},
        {"org/lwjgl/opengl/ARBShaderObjects", "glUniformMatrix2ARB", "(IZLjava/nio/FloatBuffer;)V", "glUniformMatrix2fvARB"},
        {"org/lwjgl/opengl/ARBShaderObjects", "glUniformMatrix3ARB", "(IZLjava/nio/FloatBuffer;)V", "glUniformMatrix3fvARB"},
        {"org/lwjgl/opengl/ARBShaderObjects", "glUniformMatrix4ARB", "(IZLjava/nio/FloatBuffer;)V", "glUniformMatrix4fvARB"},
        {"org/lwjgl/opengl/GL11", "glFog", "(ILjava/nio/FloatBuffer;)V", "glFogfv"},
        {"org/lwjgl/opengl/GL11", "glGetFloat", "(ILjava/nio/FloatBuffer;)V", "glGetFloatv"},
        {"org/lwjgl/opengl/GL11", "glGetInteger", "(ILjava/nio/IntBuffer;)V", "glGetIntegerv"},
        {"org/lwjgl/opengl/GL11", "glLight", "(IILjava/nio/FloatBuffer;)V", "glLightfv"},
        {"org/lwjgl/opengl/GL11", "glLightModel", "(ILjava/nio/FloatBuffer;)V", "glLightModelfv"},
        {"org/lwjgl/opengl/GL11", "glMultMatrix", "(Ljava/nio/FloatBuffer;)V", "glMultMatrixf"},
        {"org/lwjgl/opengl/GL11", "glTexEnv", "(IILjava/nio/FloatBuffer;)V", "glTexEnvfv"},
        {"org/lwjgl/opengl/GL11", "glTexGen", "(IILjava/nio/FloatBuffer;)V", "glTexGenfv"},
        {"org/lwjgl/opengl/GL20", "glUniform1", "(ILjava/nio/FloatBuffer;)V", "glUniform1fv"},
        {"org/lwjgl/opengl/GL20", "glUniform1", "(ILjava/nio/IntBuffer;)V", "glUniform1iv"},
        {"org/lwjgl/opengl/GL20", "glUniform2", "(ILjava/nio/FloatBuffer;)V", "glUniform2fv"},
        {"org/lwjgl/opengl/GL20", "glUniform2", "(ILjava/nio/IntBuffer;)V", "glUniform2iv"},
        {"org/lwjgl/opengl/GL20", "glUniform3", "(ILjava/nio/FloatBuffer;)V", "glUniform3fv"},
        {"org/lwjgl/opengl/GL20", "glUniform3", "(ILjava/nio/IntBuffer;)V", "glUniform3iv"},
        {"org/lwjgl/opengl/GL20", "glUniform4", "(ILjava/nio/FloatBuffer;)V", "glUniform4fv"},
        {"org/lwjgl/opengl/GL20", "glUniform4", "(ILjava/nio/IntBuffer;)V", "glUniform4iv"},
        {"org/lwjgl/opengl/GL20", "glUniformMatrix2", "(IZLjava/nio/FloatBuffer;)V", "glUniformMatrix2fv"},
        {"org/lwjgl/opengl/GL20", "glUniformMatrix3", "(IZLjava/nio/FloatBuffer;)V", "glUniformMatrix3fv"},
        {"org/lwjgl/opengl/GL20", "glUniformMatrix4", "(IZLjava/nio/FloatBuffer;)V", "glUniformMatrix4fv"},
    };

    private static final Map<String, String> RENAME = new HashMap<>();
    private static final Map<String, String> BRIDGE_CALLS = new HashMap<>();

    static {
        for (String[] rule : RENAMES) {
            RENAME.put(rule[0] + "|" + rule[1] + "|" + rule[2], rule[3]);
        }
        // LWJGL 2 API with no same-name, same-descriptor equivalent at all.
        BRIDGE_CALLS.put("org/lwjgl/opengl/GL20|glShaderSource|(ILjava/nio/ByteBuffer;)V",
                "glShaderSourceBuffer");
        BRIDGE_CALLS.put("org/lwjgl/opengl/ARBShaderObjects|glShaderSourceARB|(ILjava/nio/ByteBuffer;)V",
                "glShaderSourceBuffer");
        BRIDGE_CALLS.put("org/lwjgl/openal/AL|create|()V", "alCreate");
        BRIDGE_CALLS.put("org/lwjgl/openal/AL|destroy|()V", "alDestroy");
        BRIDGE_CALLS.put("org/lwjgl/openal/AL|isCreated|()Z", "alIsCreated");
    }

    public static void premain(String args, Instrumentation inst) {
        // The shim classes live in this same jar, which the agent loader can see
        // but the game's class loader cannot. Appending the jar to the system
        // search path is what makes `org.lwjgl.*` resolvable to the game.
        try {
            java.security.CodeSource source =
                    TesseraLwjgl3Agent.class.getProtectionDomain().getCodeSource();
            if (source != null && source.getLocation() != null) {
                java.io.File file = new java.io.File(source.getLocation().toURI());
                inst.appendToSystemClassLoaderSearch(new java.util.jar.JarFile(file));
            }
        } catch (Throwable t) {
            System.err.println("[tessera-lwjgl3] cannot expose the shim classes: " + t);
        }
        // Debug aid: give the JVM a current GL context before the game starts, so
        // the crash-report path can call glGetString instead of aborting the VM,
        // and the real exception becomes visible.
        if (System.getenv("TESSERA_LWJGL3_PROBE") != null) {
            try {
                org.lwjgl.glfw.GLFW.glfwInit();
                org.lwjgl.glfw.GLFW.glfwWindowHint(
                        org.lwjgl.glfw.GLFW.GLFW_VISIBLE, org.lwjgl.glfw.GLFW.GLFW_FALSE);
                long probe = org.lwjgl.glfw.GLFW.glfwCreateWindow(64, 64, "probe", 0L, 0L);
                org.lwjgl.glfw.GLFW.glfwMakeContextCurrent(probe);
                org.lwjgl.opengl.GL.createCapabilities();
                System.err.println("[tessera-lwjgl3] probe context ready");
            } catch (Throwable t) {
                System.err.println("[tessera-lwjgl3] probe context failed: " + t);
            }
        }
        inst.addTransformer(new ClassFileTransformer() {
            @Override
            public byte[] transform(ClassLoader loader, String name, Class<?> redefined,
                                    ProtectionDomain domain, byte[] bytes) {
                if (bytes == null || name == null || name.startsWith("java/")
                        || name.startsWith("jdk/") || name.startsWith("sun/")
                        // Never rewrite LWJGL itself: its own internals call the very
                        // methods we redirect (ALC.destroy -> AL.destroy).
                        || name.startsWith("org/lwjgl/") || name.startsWith("tessera/")) {
                    return null;
                }
                try {
                    ClassReader reader = new ClassReader(bytes);
                    ClassWriter writer = new ClassWriter(0);
                    final boolean[] changed = {false};
                    reader.accept(new ClassVisitor(Opcodes.ASM9, writer) {
                        @Override
                        public MethodVisitor visitMethod(int access, String mName, String mDesc,
                                                         String sig, String[] ex) {
                            MethodVisitor next = super.visitMethod(access, mName, mDesc, sig, ex);
                            return new MethodVisitor(Opcodes.ASM9, next) {
                                @Override
                                public void visitMethodInsn(int opcode, String owner, String iName,
                                                            String iDesc, boolean itf) {
                                    String key = owner + "|" + iName + "|" + iDesc;
                                    String bridge = BRIDGE_CALLS.get(key);
                                    if (bridge != null) {
                                        changed[0] = true;
                                        super.visitMethodInsn(Opcodes.INVOKESTATIC, BRIDGE,
                                                bridge, iDesc, false);
                                        return;
                                    }
                                    String renamed = RENAME.get(key);
                                    if (renamed != null) {
                                        changed[0] = true;
                                        super.visitMethodInsn(opcode, owner, renamed, iDesc, itf);
                                        return;
                                    }
                                    super.visitMethodInsn(opcode, owner, iName, iDesc, itf);
                                }
                            };
                        }
                    }, 0);
                    return changed[0] ? writer.toByteArray() : null;
                } catch (Throwable t) {
                    return null;
                }
            }
        });
    }
}
