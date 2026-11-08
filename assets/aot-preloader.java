package yaml.aot;

import java.io.File;
import java.lang.instrument.Instrumentation;
import java.util.Enumeration;
import java.util.jar.JarEntry;
import java.util.jar.JarFile;

/**
 * Training-only class preloader for the forced AOT cache.
 *
 * The JVM's AOT cache only contains classes that the training JVM actually
 * loads. A normal launch leaves large parts of the client and its libraries
 * untouched, so when {@code yaml.aot.preload=1} this agent walks the whole
 * classpath and loads every class (without initializing it) before Minecraft's
 * main method runs. Production launches carry the same agent but do not set the
 * property, so they only pay for loading this tiny class.
 */
public final class Preloader {
    private Preloader() {}

    public static void premain(String args, Instrumentation instrumentation) {
        if (!"1".equals(System.getProperty("yaml.aot.preload"))) {
            return;
        }
        String classpath = System.getProperty("java.class.path", "");
        String separator = File.pathSeparator;
        ClassLoader loader = ClassLoader.getSystemClassLoader();
        int loaded = 0;
        int failed = 0;
        for (String entry : classpath.split(java.util.regex.Pattern.quote(separator))) {
            if (!entry.endsWith(".jar")) {
                continue;
            }
            try (JarFile jar = new JarFile(entry)) {
                Enumeration<JarEntry> entries = jar.entries();
                while (entries.hasMoreElements()) {
                    String name = entries.nextElement().getName();
                    if (!name.endsWith(".class") || name.startsWith("META-INF/")) {
                        continue;
                    }
                    String binary = name.substring(0, name.length() - 6).replace('/', '.');
                    try {
                        Class.forName(binary, false, loader);
                        loaded++;
                    } catch (Throwable ignored) {
                        failed++;
                    }
                }
            } catch (Throwable ignored) {
                // Not every jar is readable or a class holder; the game itself
                // will report what it actually needs.
            }
        }
        System.out.println("[yaml-aot] preloaded=" + loaded + " failed=" + failed);
    }
}
