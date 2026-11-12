package tessera.legacy;

import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;
import java.io.DataInputStream;
import java.io.DataOutputStream;
import java.io.IOException;

/**
 * Rewrites `java/applet/` references in a class file to this package.
 *
 * JDK 28 removed `java.applet`, and the applet-era clients reference it from
 * their superclass and interface lists. Renaming those constant-pool strings is
 * enough to redirect the whole hierarchy at `tessera.legacy.Applet`, and doing
 * it in a transformer keeps the game on the ordinary application class loader —
 * which is what lets the AOT cache archive it.
 *
 * Only the constant pool is parsed; the rest of the class file is copied
 * verbatim, so this cannot change anything but names.
 */
final class AppletRenamer {
    private static final String FROM = "java/applet/";
    private static final String TO = "tessera/legacy/";

    private AppletRenamer() {}

    static byte[] rewrite(byte[] bytes) {
        try {
            DataInputStream in = new DataInputStream(new ByteArrayInputStream(bytes));
            ByteArrayOutputStream buffer = new ByteArrayOutputStream(bytes.length + 64);
            DataOutputStream out = new DataOutputStream(buffer);

            int magic = in.readInt();
            if (magic != 0xCAFEBABE) {
                return null;
            }
            out.writeInt(magic);
            out.writeShort(in.readUnsignedShort());
            out.writeShort(in.readUnsignedShort());
            int count = in.readUnsignedShort();
            if (count == 0) {
                return null;
            }
            out.writeShort(count);

            boolean changed = false;
            for (int index = 1; index < count; index++) {
                int tag = in.readUnsignedByte();
                out.writeByte(tag);
                switch (tag) {
                    case 1: {
                        String text = in.readUTF();
                        String renamed = text.replace(FROM, TO);
                        changed |= !renamed.equals(text);
                        out.writeUTF(renamed);
                        break;
                    }
                    case 3:
                    case 4:
                        out.writeInt(in.readInt());
                        break;
                    case 5:
                    case 6:
                        out.writeLong(in.readLong());
                        index++;
                        break;
                    case 7:
                    case 8:
                    case 16:
                    case 19:
                    case 20:
                        out.writeShort(in.readUnsignedShort());
                        break;
                    case 9:
                    case 10:
                    case 11:
                    case 12:
                    case 17:
                    case 18:
                        out.writeInt(in.readInt());
                        break;
                    case 15:
                        out.writeByte(in.readUnsignedByte());
                        out.writeShort(in.readUnsignedShort());
                        break;
                    default:
                        return null;
                }
            }
            byte[] rest = new byte[in.available()];
            in.readFully(rest);
            out.write(rest);
            out.flush();
            return changed ? buffer.toByteArray() : null;
        } catch (IOException error) {
            return null;
        }
    }
}
