package java.applet;

import java.awt.Image;
import java.awt.Panel;
import java.net.URL;

/**
 * The Applet base class, for JDKs that no longer ship java.applet.
 *
 * Minecraft 1.5.2 and older ship a `MinecraftApplet extends Applet`, and even
 * their standalone `main` builds an `Applet` subclass as the stub. The API
 * surface they use is small: construction, `setStub`, `getParameter`,
 * `getDocumentBase`, `getCodeBase`, and the lifecycle methods.
 */
public class Applet extends Panel {
    private AppletStub stub;

    public Applet() {
        super();
    }

    public final void setStub(AppletStub stub) {
        this.stub = stub;
    }

    public boolean isActive() {
        return stub != null && stub.isActive();
    }

    public URL getDocumentBase() {
        return stub == null ? null : stub.getDocumentBase();
    }

    public URL getCodeBase() {
        return stub == null ? null : stub.getCodeBase();
    }

    public String getParameter(String name) {
        return stub == null ? null : stub.getParameter(name);
    }

    public AppletContext getAppletContext() {
        return stub == null ? null : stub.getAppletContext();
    }

    public void showStatus(String message) {
        AppletContext context = getAppletContext();
        if (context != null) {
            context.showStatus(message);
        }
    }

    public void resize(int width, int height) {
        setSize(width, height);
    }

    public void init() {}

    public void start() {}

    public void stop() {}

    public void destroy() {}

    public String getAppletInfo() {
        return null;
    }

    public String[][] getParameterInfo() {
        return null;
    }

    public AudioClip getAudioClip(URL url) {
        return null;
    }

    public AudioClip getAudioClip(URL url, String name) {
        return null;
    }

    public Image getImage(URL url) {
        return null;
    }

    public Image getImage(URL url, String name) {
        return null;
    }

    public void play(URL url) {}

    public void play(URL url, String name) {}

    public static AudioClip newAudioClip(URL url) {
        return null;
    }
}
