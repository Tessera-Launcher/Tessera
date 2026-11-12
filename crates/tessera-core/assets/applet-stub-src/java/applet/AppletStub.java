package java.applet;

import java.net.URL;

/** Minimal AppletStub for versions whose client was distributed as an applet. */
public interface AppletStub {
    boolean isActive();
    URL getDocumentBase();
    URL getCodeBase();
    String getParameter(String name);
    AppletContext getAppletContext();
    void appletResize(int width, int height);
}
