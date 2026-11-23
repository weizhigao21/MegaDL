package com.weizhigao21.megadl;

import android.content.ContentResolver;
import android.content.ContentValues;
import android.content.Context;
import android.database.Cursor;
import android.net.Uri;
import android.os.Build;
import android.os.Environment;
import android.os.ParcelFileDescriptor;
import android.provider.MediaStore;
import android.webkit.MimeTypeMap;

import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.RandomAccessFile;
import java.nio.ByteBuffer;
import java.nio.channels.FileChannel;
import java.util.Locale;

/**
 * 下载落地位置。Android 10 起走 MediaStore.Downloads，直接写入公共
 * Download/MegaDL，不需要任何存储权限；旧系统退回公共 Download 目录 + 写权限。
 */
public final class Storage {

    public static final String PUBLIC_PATH = "/storage/emulated/0/Download/MegaDL";

    public interface Target {
        /** 指定绝对偏移写入，供多连接分块并发使用。 */
        void write(byte[] data, int length, long offset) throws IOException;

        /** 收尾：落盘并让文件在系统文件管理里可见。 */
        void commit() throws IOException;

        /** 失败或取消时丢弃半成品。 */
        void abort();

        String path();
    }

    public interface Store {
        Target create(String relPath, String name, long size) throws IOException;

        /** 界面展示用的保存位置。 */
        String label();
    }

    private Storage() {
    }

    public static Store create(Context context) {
        if (Build.VERSION.SDK_INT >= 29) return new MediaStoreStore(context);
        return new LegacyStore();
    }

    // ------------------------------------------------------------ MediaStore

    private static final class MediaStoreStore implements Store {
        private final ContentResolver resolver;

        MediaStoreStore(Context context) {
            resolver = context.getContentResolver();
        }

        @Override
        public String label() {
            return "Download/MegaDL";
        }

        @Override
        public Target create(String relPath, String name, long size) throws IOException {
            String rel = normalize(relPath);
            String relativePath = "Download/MegaDL/" + (rel.isEmpty() ? "" : rel + "/");
            String display = uniqueName(relativePath, name);

            ContentValues values = new ContentValues();
            values.put(MediaStore.Downloads.DISPLAY_NAME, display);
            values.put(MediaStore.Downloads.MIME_TYPE, mimeOf(display));
            values.put(MediaStore.Downloads.RELATIVE_PATH, relativePath);
            values.put(MediaStore.Downloads.IS_PENDING, 1);

            Uri uri = resolver.insert(MediaStore.Downloads.EXTERNAL_CONTENT_URI, values);
            if (uri == null) throw new IOException("无法在下载目录创建文件");
            ParcelFileDescriptor pfd = null;
            try {
                pfd = resolver.openFileDescriptor(uri, "rw");
                if (pfd == null) throw new IOException("无法打开下载文件");
                return new MediaTarget(resolver, uri, pfd,
                        "Download/MegaDL/" + (rel.isEmpty() ? "" : rel + "/") + display);
            } catch (IOException e) {
                resolver.delete(uri, null, null);
                throw e;
            }
        }

        private String uniqueName(String relativePath, String name) {
            String candidate = name;
            if (!exists(relativePath, candidate)) return candidate;
            String stem = name;
            String ext = "";
            int dot = name.lastIndexOf('.');
            if (dot > 0) {
                stem = name.substring(0, dot);
                ext = name.substring(dot);
            }
            for (int i = 1; i < 10000; i++) {
                candidate = stem + " (" + i + ")" + ext;
                if (!exists(relativePath, candidate)) return candidate;
            }
            return candidate;
        }

        private boolean exists(String relativePath, String display) {
            String[] projection = {MediaStore.Downloads._ID};
            String selection = MediaStore.Downloads.RELATIVE_PATH + "=? AND " + MediaStore.Downloads.DISPLAY_NAME + "=?";
            try (Cursor c = resolver.query(MediaStore.Downloads.EXTERNAL_CONTENT_URI, projection, selection,
                    new String[]{relativePath, display}, null)) {
                return c != null && c.getCount() > 0;
            }
        }

        private static final class MediaTarget implements Target {
            private final ContentResolver resolver;
            private final Uri uri;
            private final ParcelFileDescriptor pfd;
            private final FileOutputStream out;
            private final FileChannel channel;
            private final String path;
            private boolean closed;

            MediaTarget(ContentResolver resolver, Uri uri, ParcelFileDescriptor pfd, String path) {
                this.resolver = resolver;
                this.uri = uri;
                this.pfd = pfd;
                this.out = new FileOutputStream(pfd.getFileDescriptor());
                this.channel = out.getChannel();
                this.path = path;
            }

            @Override
            public void write(byte[] data, int length, long offset) throws IOException {
                ByteBuffer buffer = ByteBuffer.wrap(data, 0, length);
                long pos = offset;
                while (buffer.hasRemaining()) {
                    int n = channel.write(buffer, pos);
                    if (n <= 0) throw new IOException("写入无进展");
                    pos += n;
                }
            }

            @Override
            public void commit() throws IOException {
                channel.force(true);
                closeQuietly();
                ContentValues done = new ContentValues();
                done.put(MediaStore.Downloads.IS_PENDING, 0);
                resolver.update(uri, done, null, null);
            }

            @Override
            public void abort() {
                closeQuietly();
                try {
                    resolver.delete(uri, null, null);
                } catch (Throwable ignored) {
                }
            }

            @Override
            public String path() {
                return path;
            }

            private void closeQuietly() {
                if (closed) return;
                closed = true;
                try {
                    channel.close();
                } catch (IOException ignored) {
                }
                try {
                    out.close();
                } catch (IOException ignored) {
                }
                try {
                    pfd.close();
                } catch (IOException ignored) {
                }
            }
        }
    }

    // ------------------------------------------------------------ 旧系统

    private static final class LegacyStore implements Store {
        private final File base = new File(Environment.getExternalStoragePublicDirectory(
                Environment.DIRECTORY_DOWNLOADS), "MegaDL");

        @Override
        public String label() {
            return "Download/MegaDL";
        }

        @Override
        public Target create(String relPath, String name, long size) throws IOException {
            File dir = base;
            String rel = normalize(relPath);
            if (!rel.isEmpty()) dir = new File(dir, rel);
            if (!dir.exists() && !dir.mkdirs()) throw new IOException("无法创建保存目录 " + dir);
            File dest = unique(dir, name);
            File part = new File(dest.getAbsolutePath() + ".part");
            RandomAccessFile raf = new RandomAccessFile(part, "rw");
            raf.setLength(size);
            return new LegacyTarget(raf, part, dest);
        }

        private File unique(File dir, String name) {
            File dest = new File(dir, name);
            if (!occupied(dest)) return dest;
            String stem = name;
            String ext = "";
            int dot = name.lastIndexOf('.');
            if (dot > 0) {
                stem = name.substring(0, dot);
                ext = name.substring(dot);
            }
            for (int i = 1; i < 10000; i++) {
                File candidate = new File(dir, stem + " (" + i + ")" + ext);
                if (!occupied(candidate)) return candidate;
            }
            return dest;
        }

        private boolean occupied(File f) {
            return f.exists() || new File(f.getAbsolutePath() + ".part").exists();
        }

        private static final class LegacyTarget implements Target {
            private final RandomAccessFile raf;
            private final File part;
            private final File dest;

            LegacyTarget(RandomAccessFile raf, File part, File dest) {
                this.raf = raf;
                this.part = part;
                this.dest = dest;
            }

            @Override
            public void write(byte[] data, int length, long offset) throws IOException {
                raf.seek(offset);
                raf.write(data, 0, length);
            }

            @Override
            public void commit() throws IOException {
                raf.close();
                if (dest.exists() && !dest.delete()) throw new IOException("无法覆盖同名文件");
                if (!part.renameTo(dest)) throw new IOException("重命名失败");
            }

            @Override
            public void abort() {
                try {
                    raf.close();
                } catch (IOException ignored) {
                }
                part.delete();
            }

            @Override
            public String path() {
                return dest.getAbsolutePath();
            }
        }
    }

    private static String normalize(String rel) {
        if (rel == null) return "";
        String s = rel.replace('\\', '/');
        while (s.startsWith("/")) s = s.substring(1);
        while (s.endsWith("/")) s = s.substring(0, s.length() - 1);
        return s;
    }

    private static String mimeOf(String name) {
        int dot = name.lastIndexOf('.');
        if (dot > 0 && dot + 1 < name.length()) {
            String ext = name.substring(dot + 1).toLowerCase(Locale.US);
            String mime = MimeTypeMap.getSingleton().getMimeTypeFromExtension(ext);
            if (mime != null) return mime;
        }
        return "application/octet-stream";
    }
}
