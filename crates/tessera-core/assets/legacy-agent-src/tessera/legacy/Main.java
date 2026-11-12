package tessera.legacy;

import java.awt.BorderLayout;
import java.awt.Frame;
import java.lang.reflect.Method;
import java.util.HashMap;
import java.util.Map;

/**
 * Entry point for clients that would otherwise boot through LaunchWrapper.
 *
 * Most of them have a plain `Minecraft.main`; the classic (c0.x) releases only
 * ship an applet, so the launcher hosts it in a frame the way the browser used
 * to. Both paths run on the ordinary application class loader.
 */
public final class Main {
    private Main() {}

    public static void main(String[] args) throws Exception {
        Method main = findMain();
        if (main != null) {
            main.invoke(null, (Object) args);
            return;
        }
        hostApplet(args);
    }

    private static Method findMain() {
        for (String name : new String[] {
                "net.minecraft.client.Minecraft",
                "com.mojang.minecraft.Minecraft",
                "com.mojang.rubydung.RubyDung"}) {
            try {
                return Class.forName(name).getMethod("main", String[].class);
            } catch (ClassNotFoundException | NoSuchMethodException ignored) {
                // try the next name
            }
        }
        return null;
    }

    private static void hostApplet(String[] args) throws Exception {
        Class<?> appletClass = null;
        for (String name : new String[] {
                "net.minecraft.client.MinecraftApplet",
                "com.mojang.minecraft.MinecraftApplet"}) {
            try {
                appletClass = Class.forName(name);
                break;
            } catch (ClassNotFoundException ignored) {
                // try the next name
            }
        }
        if (appletClass == null) {
            throw new IllegalStateException("no client entry point found");
        }
        Applet applet = (Applet) appletClass.getConstructor().newInstance();
        applet.setStub(new Stub(parameters(args)));

        Frame frame = new Frame("Minecraft");
        frame.setLayout(new BorderLayout());
        frame.add(applet, BorderLayout.CENTER);
        frame.setSize(854, 480);
        frame.validate();
        frame.setVisible(true);
        applet.init();
        applet.start();
    }

    /** The applet parameters, from the legacy positional arguments. */
    private static Map<String, String> parameters(String[] args) {
        Map<String, String> map = new HashMap<>();
        String username = null;
        String session = null;
        for (String arg : args) {
            if (arg.startsWith("--")) {
                continue;
            }
            if (username == null) {
                username = arg;
            } else if (session == null) {
                session = arg;
            }
        }
        if (username != null) {
            map.put("username", username);
        }
        if (session != null) {
            String id = session.startsWith("token:") ? session.substring("token:".length()) : session;
            map.put("sessionid", id);
        }
        return map;
    }
}
