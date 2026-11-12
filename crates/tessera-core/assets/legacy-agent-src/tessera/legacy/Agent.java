package tessera.legacy;

import java.security.ProtectionDomain;
import java.lang.instrument.ClassFileTransformer;
import java.lang.instrument.Instrumentation;

/**
 * Agent for clients that reference `java.applet`, which JDK 28 removed.
 *
 * Only the class files that mention the package are rewritten; everything else
 * is left untouched, and no class loader is replaced, so the AOT cache can
 * still archive the game.
 */
public final class Agent {
    public static void premain(String arguments, Instrumentation instrumentation) {
        instrumentation.addTransformer(new ClassFileTransformer() {
            @Override
            public byte[] transform(
                    ClassLoader loader,
                    String className,
                    Class<?> redefined,
                    ProtectionDomain domain,
                    byte[] bytes) {
                if (loader == null || bytes == null) {
                    return null;
                }
                return AppletRenamer.rewrite(bytes);
            }
        });
    }
}
