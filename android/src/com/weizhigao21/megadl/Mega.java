package com.weizhigao21.megadl;

import android.util.Base64;

import org.json.JSONException;
import org.json.JSONObject;

import java.io.UnsupportedEncodingException;
import java.util.ArrayList;
import java.util.List;
import java.util.Locale;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

import javax.crypto.Cipher;
import javax.crypto.spec.IvParameterSpec;
import javax.crypto.spec.SecretKeySpec;

/**
 * MEGA 协议里的纯计算部分：链接解析、Base64、AES 解密。
 * 与桌面 Rust 版 mega.rs 的字段与算法保持一致。
 */
public final class Mega {

    public static final String API_G = "https://g.api.mega.co.nz/cs";
    public static final String API_EU = "https://eu.api.mega.co.nz/cs";

    private static final Pattern FOLDER_NEW =
            Pattern.compile("/folder/([A-Za-z0-9_-]{6,})#([A-Za-z0-9_\\-+,]{20,})");
    private static final Pattern FILE_NEW =
            Pattern.compile("/file/([A-Za-z0-9_-]{6,})#([A-Za-z0-9_\\-+,]{20,})");
    private static final Pattern OLD =
            Pattern.compile("#(!|F!)([A-Za-z0-9_-]{6,})!([A-Za-z0-9_\\-+,]{20,})");
    private static final Pattern BARE =
            Pattern.compile("(?:^|!)([A-Za-z0-9_-]{6,})!([A-Za-z0-9_\\-+,]{16,})");

    private Mega() {
    }

    public enum Kind {FILE, FOLDER}

    public static final class Link {
        public final Kind kind;
        public final String id;
        public final String key;

        Link(Kind kind, String id, String key) {
            this.kind = kind;
            this.id = id;
            this.key = key;
        }
    }

    public static final class MegaError extends Exception {
        public MegaError(String message) {
            super(message);
        }
    }

    public static Link parseLink(String raw) throws MegaError {
        String link = raw == null ? "" : raw.trim();
        Matcher m = FOLDER_NEW.matcher(link);
        if (m.find()) return new Link(Kind.FOLDER, m.group(1), m.group(2));
        m = FILE_NEW.matcher(link);
        if (m.find()) return new Link(Kind.FILE, m.group(1), m.group(2));
        m = OLD.matcher(link);
        if (m.find()) {
            Kind kind = m.group(1).startsWith("F") ? Kind.FOLDER : Kind.FILE;
            return new Link(kind, m.group(2), m.group(3));
        }
        m = BARE.matcher(link);
        if (m.find()) return new Link(Kind.FILE, m.group(1), m.group(2));
        throw new MegaError("无法识别的 MEGA 链接，请确认是 /file/ 或 /folder/ 分享链接");
    }

    /** 文件链接密钥 32 字节（43 字符），文件夹链接密钥 16 字节（22 字符）。 */
    public static byte[] linkKey(Link link) throws MegaError {
        int chars = link.kind == Kind.FILE ? 43 : 22;
        int bytes = link.kind == Kind.FILE ? 32 : 16;
        String cleaned = link.key.replace(",", "").replace("=", "");
        cleaned = cleaned.replaceAll("\\s+", "");
        if (cleaned.length() != chars) {
            throw new MegaError("分享链接的密钥不完整：当前 " + cleaned.length()
                    + " 个字符，" + (link.kind == Kind.FILE ? "文件" : "文件夹")
                    + "链接应为 " + chars + " 个。请重新复制完整链接");
        }
        byte[] key = b64d(cleaned);
        if (key == null || key.length != bytes) {
            throw new MegaError("分享链接的密钥无效，请重新复制完整链接");
        }
        return key;
    }

    public static byte[] b64d(String s) {
        if (s == null) return null;
        String v = s.replaceAll("\\s+", "");
        int rem = v.length() % 4;
        if (rem == 1) return null;
        if (rem == 2) v = v + "==";
        else if (rem == 3) v = v + "=";
        try {
            return Base64.decode(v, Base64.URL_SAFE | Base64.NO_WRAP);
        } catch (IllegalArgumentException e) {
            return null;
        }
    }

    public static String b64e(byte[] data) {
        return Base64.encodeToString(data, Base64.URL_SAFE | Base64.NO_WRAP | Base64.NO_PADDING);
    }

    private static SecretKeySpec aesKey(byte[] key) throws MegaError {
        if (key == null || (key.length != 16 && key.length < 32)) {
            throw new MegaError("密钥长度错误");
        }
        byte[] k = new byte[16];
        if (key.length == 16) {
            System.arraycopy(key, 0, k, 0, 16);
        } else {
            for (int i = 0; i < 16; i++) k[i] = (byte) (key[i] ^ key[i + 16]);
        }
        return new SecretKeySpec(k, "AES");
    }

    /** 解密文件属性：AES-CBC，IV 全 0，尾部补 0 到 16 字节对齐，明文以 "MEGA" 开头。 */
    public static JSONObject decryptAttrs(String atB64, byte[] key) throws MegaError {
        byte[] data = b64d(atB64);
        if (data == null || data.length == 0) throw new MegaError("文件属性为空");
        int pad = (16 - data.length % 16) % 16;
        byte[] padded = new byte[data.length + pad];
        System.arraycopy(data, 0, padded, 0, data.length);
        try {
            Cipher cipher = Cipher.getInstance("AES/CBC/NoPadding");
            cipher.init(Cipher.DECRYPT_MODE, aesKey(key), new IvParameterSpec(new byte[16]));
            byte[] plain = cipher.doFinal(padded);
            String text = new String(plain, "UTF-8").replace("\u0000", "");
            if (!text.startsWith("MEGA")) throw new MegaError("文件属性解密失败");
            String body = text.substring(4);
            try {
                return new JSONObject(body);
            } catch (JSONException ignored) {
                JSONObject fallback = new JSONObject();
                Matcher m = Pattern.compile("\"n\"\\s*:\\s*\"(.*?)\"").matcher(body);
                if (m.find()) {
                    try {
                        fallback.put("n", m.group(1));
                    } catch (JSONException e) {
                        // ignore
                    }
                }
                return fallback;
            }
        } catch (MegaError e) {
            throw e;
        } catch (Exception e) {
            throw new MegaError("文件属性解密失败");
        }
    }

    /** 用文件夹密钥 AES-128-ECB 解密节点密钥，k 字段形如 "handle:key"。 */
    public static byte[] decryptNodeKey(String kField, byte[] folderKey) throws MegaError {
        String part = kField.contains(":") ? kField.substring(kField.indexOf(':') + 1) : kField;
        byte[] blob = b64d(part);
        if (blob == null || blob.length == 0 || blob.length % 16 != 0) {
            throw new MegaError("节点密钥长度错误");
        }
        try {
            Cipher cipher = Cipher.getInstance("AES/ECB/NoPadding");
            cipher.init(Cipher.DECRYPT_MODE, aesKey(folderKey));
            return cipher.doFinal(blob);
        } catch (Exception e) {
            throw new MegaError("节点密钥解密失败");
        }
    }

    /**
     * 按文件绝对偏移解密一段数据。AES-CTR：IV = key[16..24] + 大端块序号，
     * offset 必须是 16 的倍数（分块下载按 16 字节对齐切块）。
     */
    public static byte[] ctrDecrypt(byte[] data, byte[] key, long offset) throws MegaError {
        return ctrDecrypt(data, key, offset, data.length);
    }

    public static byte[] ctrDecrypt(byte[] data, byte[] key, long offset, int len) throws MegaError {
        if (offset % 16 != 0) throw new MegaError("CTR 偏移未对齐");
        try {
            byte[] iv = new byte[16];
            if (key.length >= 24) System.arraycopy(key, 16, iv, 0, 8);
            long block = offset / 16;
            for (int i = 0; i < 8; i++) iv[15 - i] = (byte) (block >>> (8 * i));
            Cipher cipher = Cipher.getInstance("AES/CTR/NoPadding");
            cipher.init(Cipher.DECRYPT_MODE, aesKey(key), new IvParameterSpec(iv));
            return cipher.doFinal(data, 0, len);
        } catch (MegaError e) {
            throw e;
        } catch (Exception e) {
            throw new MegaError("数据解密失败");
        }
    }

    public static String apiMessage(int code) {
        switch (code) {
            case -1: return "EINTERNAL：内部错误";
            case -2: return "EARGS：参数错误";
            case -3: return "EAGAIN：请求过于频繁，请稍后再试";
            case -4: return "ERATELIMIT：请求频率超限";
            case -5: return "EFAILED：请求失败";
            case -6: return "ETOOMANY：连接数过多";
            case -7: return "ERANGE：Range 越界";
            case -8: return "EEXPIRED：下载链接已过期";
            case -9: return "ENOENT：节点不存在或链接已失效";
            case -10: return "ECIRCULAR：循环链接";
            case -11: return "EACCESS：无访问权限";
            case -12: return "EEXIST：已存在";
            case -14: return "EKEY：密钥错误";
            case -15: return "ESID：会话过期";
            case -16: return "EBLOCKED：资源已被封禁";
            case -17: return "EOVERQUOTA：带宽/配额超限";
            case -18: return "ETEMPUNAVAIL：暂时不可用";
            case -19: return "ETOOMANYCONNECTIONS：连接过多";
            case -20: return "EWRITE：写入失败";
            case -21: return "EREAD：读取失败";
            default: return "MEGA 错误 " + code;
        }
    }

    public static String fmtSize(long n) {
        if (n < 1024) return n + "B";
        double v = n;
        String[] units = {"B", "KB", "MB", "GB", "TB"};
        for (int i = 0; i < units.length; i++) {
            if (v < 1024.0 || i == units.length - 1) {
                return String.format(Locale.US, "%.1f%s", v, units[i]);
            }
            v /= 1024.0;
        }
        return n + "B";
    }

    public static String safeName(String name) {
        if (name == null || name.isEmpty()) return "unnamed";
        StringBuilder sb = new StringBuilder();
        for (char c : name.toCharArray()) {
            if (c < 0x20 || "<>:\"/\\|?*".indexOf(c) >= 0) sb.append('_');
            else sb.append(c);
        }
        String trimmed = sb.toString().replaceAll("[ .]+$", "").trim();
        return trimmed.isEmpty() ? "unnamed" : trimmed;
    }
}
