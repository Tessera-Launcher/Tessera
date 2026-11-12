package tessera.legacy;

import java.io.File;
import java.net.URL;
import java.net.URLClassLoader;

/**
 * System class loader for LaunchWrapper-era versions on a modern JVM.
 *
 * LaunchWrapper 1.5 and 1.12 open with
 * {@code (URLClassLoader) Launch.class.getClassLoader()}; since Java 9 the
 * system loader is no longer a URLClassLoader, so that line throws before the
 * game starts. Installing this loader with
 * {@code -Djava.system.class.loader=tessera.legacy.CastSafeLoader} both makes
 * the cast valid and lets the loader keep the whole application class path.
 *
 * Only the LaunchWrapper classes are loaded here (child-first) so the rest of
 * the game keeps the ordinary system loader; LaunchWrapper then builds its own
 * LaunchClassLoader with this one as the parent.
 */
public final class CastSafeLoader extends URLClassLoader {
    public CastSafeLoader(ClassLoader parent) throws Exception {
        super(classpath(), parent);
    }

    @Override
    protected Class<?> loadClass(String name, boolean resolve) throws ClassNotFoundException {
        if (!name.startsWith("net.minecraft.launchwrapper.")) {
            return super.loadClass(name, resolve);
        }
        synchronized (getClassLoadingLock(name)) {
            Class<?> found = findLoadedClass(name);
            if (found == null) {
                found = findClass(name);
            }
            if (resolve) {
                resolveClass(found);
            }
            return found;
        }
    }

    private static URL[] classpath() throws Exception {
        String[] parts = System.getProperty("java.class.path", "").split(File.pathSeparator);
        URL[] urls = new URL[parts.length];
        for (int index = 0; index < parts.length; index++) {
            urls[index] = new File(parts[index]).toURI().toURL();
        }
        return urls;
    }
}
