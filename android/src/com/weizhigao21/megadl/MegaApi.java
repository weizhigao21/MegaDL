package com.weizhigao21.megadl;

import android.util.Log;

import org.json.JSONArray;
import org.json.JSONObject;
import org.json.JSONTokener;

import java.io.BufferedInputStream;
import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.io.OutputStream;
import java.net.HttpURLConnection;
import java.net.URL;
import java.net.URLEncoder;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.Collections;
import java.util.Comparator;
import java.util.List;

/** MEGA 匿名 API 客户端：g / f 请求，解密属性与节点密钥。 */
public class MegaApi {

    private static final String TAG = "MegaApi";

    private final List<String> endpoints = new ArrayList<>();
    private long seq = System.currentTimeMillis();

    public MegaApi(boolean euFirst) {
        if (euFirst) {
            endpoints.add(Mega.API_EU);
            endpoints.add(Mega.API_G);
        } else {
            endpoints.add(Mega.API_G);
            endpoints.add(Mega.API_EU);
        }
    }

    public String preferred() {
        return endpoints.get(0).contains("eu.api") ? "EU" : "G";
    }

    public static final class FileInfo {
        public long size;
        public String at;
        public String url;
    }

    public static final class Node {
        public String name = "";
        public String handle = "";
        public String parent;
        public long size;
        public boolean dir;
        public byte[] key;
        public boolean checked;
        public boolean expanded;
        public final List<Node> children = new ArrayList<>();
        public Node parentNode;
        public int depth;

        // 下载状态：0 等待 1 获取直链 2 下载中 3 完成 4 失败 5 已停止 6 跳过
        public volatile int state;
        public String error;
        public volatile long downloaded;
    }

    public FileInfo fileInfo(String fileId, String node, String folder) throws Mega.MegaError {
        JSONObject item = new JSONObject();
        try {
            item.put("a", "g");
            item.put("g", 1);
            item.put("ssl", 0);
            if (node != null) item.put("n", node);
            else item.put("p", fileId == null ? "" : fileId);
        } catch (Exception e) {
            throw new Mega.MegaError("构造请求失败");
        }
        JSONArray payload = new JSONArray();
        payload.put(item);
        Object data = request(payload, folder != null ? folder : node);
        JSONObject info = firstObject(data);
        int code = info.optInt("e", 0);
        if (code != 0) throw new Mega.MegaError(Mega.apiMessage(code) + "（获取文件信息）");
        FileInfo fi = new FileInfo();
        fi.size = info.optLong("s", -1);
        fi.at = info.optString("at", null);
        fi.url = info.optString("g", null);
        if (fi.size < 0 || fi.at == null || fi.url == null) {
            throw new Mega.MegaError("MEGA 返回的文件信息不完整");
        }
        return fi;
    }

    public List<JSONObject> folderNodes(String folderId) throws Mega.MegaError {
        JSONObject item = new JSONObject();
        try {
            item.put("a", "f");
            item.put("c", 1);
            item.put("r", 1);
        } catch (Exception e) {
            throw new Mega.MegaError("构造请求失败");
        }
        JSONArray payload = new JSONArray();
        payload.put(item);
        JSONObject resp = firstObject(request(payload, folderId));
        int code = resp.optInt("e", 0);
        if (code != 0) throw new Mega.MegaError(Mega.apiMessage(code) + "（获取文件夹列表）");
        JSONArray arr = resp.optJSONArray("f");
        List<JSONObject> out = new ArrayList<>();
        if (arr != null) {
            for (int i = 0; i < arr.length(); i++) {
                JSONObject o = arr.optJSONObject(i);
                if (o != null) out.add(o);
            }
        }
        return out;
    }

    public List<Node> enrichFolder(Mega.Link link) throws Mega.MegaError {
        byte[] folderKey = Mega.linkKey(link);
        List<JSONObject> raw = folderNodes(link.id);
        List<Node> nodes = new ArrayList<>();
        for (JSONObject n : raw) {
            int type = n.optInt("t", 0);
            if (type != 0 && type != 1) continue;
            String k = n.optString("k", null);
            String a = n.optString("a", null);
            if (k == null || a == null) continue;
            byte[] nodeKey;
            JSONObject attrs;
            try {
                nodeKey = Mega.decryptNodeKey(k, folderKey);
                attrs = Mega.decryptAttrs(a, nodeKey);
            } catch (Mega.MegaError e) {
                continue;
            }
            Node node = new Node();
            node.handle = n.optString("h", "");
            if (node.handle.isEmpty()) continue;
            node.name = attrs.optString("n", node.handle);
            node.size = type == 0 ? n.optLong("s", 0) : 0;
            node.dir = type == 1;
            node.key = nodeKey;
            node.parent = n.isNull("p") ? null : n.optString("p", null);
            nodes.add(node);
        }
        return nodes;
    }

    /** 把扁平节点挂成树：父句柄缺失或等于分享 id 视为根。 */
    public static List<Node> buildTree(List<Node> flat, String rootId) {
        List<Node> roots = new ArrayList<>();
        java.util.Map<String, Node> byHandle = new java.util.HashMap<>();
        for (Node n : flat) byHandle.put(n.handle, n);
        for (Node n : flat) {
            Node parent = n.parent == null ? null : byHandle.get(n.parent);
            if (parent == null || n.parent == null || n.parent.equals(rootId) || n.parent.equals(n.handle)) {
                roots.add(n);
            } else {
                n.parentNode = parent;
                parent.children.add(n);
            }
        }
        sortRecursive(roots, 0);
        return roots;
    }

    private static void sortRecursive(List<Node> list, int depth) {
        Collections.sort(list, new Comparator<Node>() {
            @Override
            public int compare(Node a, Node b) {
                if (a.dir != b.dir) return a.dir ? -1 : 1;
                return a.name.compareToIgnoreCase(b.name);
            }
        });
        for (Node n : list) {
            n.depth = depth;
            sortRecursive(n.children, depth + 1);
        }
    }

    private JSONObject firstObject(Object data) throws Mega.MegaError {
        if (data instanceof JSONArray) {
            JSONArray arr = (JSONArray) data;
            if (arr.length() == 0) throw new Mega.MegaError("MEGA 返回为空");
            Object first = arr.opt(0);
            if (first instanceof Number) {
                throw new Mega.MegaError(Mega.apiMessage(((Number) first).intValue()));
            }
            if (first instanceof JSONObject) return (JSONObject) first;
            throw new Mega.MegaError("MEGA 返回格式异常");
        }
        if (data instanceof JSONObject) return (JSONObject) data;
        throw new Mega.MegaError("MEGA 返回格式异常");
    }

    /** 发一个 cs 请求；网络错误会在 G / EU 之间切换并短暂重试。 */
    private Object request(JSONArray payload, String node) throws Mega.MegaError {
        String last = "";
        for (int attempt = 0; attempt < 6; attempt++) {
            long id;
            synchronized (this) {
                seq += 1;
                id = seq;
            }
            String base = endpoints.get(0);
            String url = base + "?id=" + id;
            if (node != null) {
                try {
                    url += "&n=" + URLEncoder.encode(node, "UTF-8");
                } catch (Exception ignored) {
                    // ignore
                }
            }
            HttpURLConnection conn = null;
            try {
                conn = (HttpURLConnection) new URL(url).openConnection();
                conn.setRequestMethod("POST");
                conn.setConnectTimeout(15000);
                conn.setReadTimeout(60000);
                conn.setDoOutput(true);
                conn.setRequestProperty("Content-Type", "application/json");
                conn.setRequestProperty("User-Agent", "MegaDL-Android/0.2");
                byte[] body = payload.toString().getBytes(StandardCharsets.UTF_8);
                try (OutputStream os = conn.getOutputStream()) {
                    os.write(body);
                }
                int status = conn.getResponseCode();
                if (status == 509) throw new Mega.MegaError("EOVERQUOTA：带宽/配额超限");
                String text;
                try (BufferedInputStream in = new BufferedInputStream(
                        status >= 400 ? conn.getErrorStream() : conn.getInputStream())) {
                    ByteArrayOutputStream bos = new ByteArrayOutputStream();
                    byte[] buf = new byte[8192];
                    int n;
                    while ((n = in.read(buf)) > 0) bos.write(buf, 0, n);
                    text = new String(bos.toByteArray(), StandardCharsets.UTF_8);
                }
                if (status >= 400) {
                    last = "HTTP " + status;
                } else {
                    if (text == null || text.trim().isEmpty()) throw new Mega.MegaError("MEGA 返回为空");
                    return new JSONTokener(text.trim()).nextValue();
                }
            } catch (Mega.MegaError e) {
                throw e;
            } catch (IOException e) {
                last = e.getClass().getSimpleName();
                rotate();
            } catch (Exception e) {
                last = "网络请求失败";
            }
            sleep(300L * (attempt + 1));
        }
        throw new Mega.MegaError("网络请求失败：" + last + "，请检查网络或代理");
    }

    private void rotate() {
        synchronized (this) {
            if (endpoints.size() > 1) {
                String first = endpoints.remove(0);
                endpoints.add(first);
            }
        }
    }

    private static void sleep(long ms) {
        try {
            Thread.sleep(ms);
        } catch (InterruptedException ignored) {
            Thread.currentThread().interrupt();
        }
    }
}
