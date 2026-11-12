package tessera.legacy;

/** Entry point for clients that would otherwise boot through LaunchWrapper. */
public final class Main {
    private Main() {}

    public static void main(String[] args) throws Exception {
        Class<?> client = Class.forName("net.minecraft.client.Minecraft");
        client.getMethod("main", String[].class).invoke(null, (Object) args);
    }
}
