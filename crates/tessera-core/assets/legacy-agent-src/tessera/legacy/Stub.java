package tessera.legacy;

import java.net.URL;
import java.util.Map;

/** The applet parameters the classic clients read. */
final class Stub implements AppletStub {
    private final Map<String, String> parameters;

    Stub(Map<String, String> parameters) {
        this.parameters = parameters;
    }

    @Override
    public boolean isActive() {
        return true;
    }

    @Override
    public URL getDocumentBase() {
        try {
            return new URL("http://www.minecraft.net/game/");
        } catch (Exception error) {
            return null;
        }
    }

    @Override
    public URL getCodeBase() {
        return getDocumentBase();
    }

    @Override
    public String getParameter(String name) {
        return parameters.get(name);
    }

    @Override
    public AppletContext getAppletContext() {
        return null;
    }

    @Override
    public void appletResize(int width, int height) {}

    void showStatus(String message) {}
}
