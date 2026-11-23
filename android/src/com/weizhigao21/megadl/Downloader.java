package com.weizhigao21.megadl;

import java.io.IOException;
import java.io.InputStream;
import java.net.HttpURLConnection;
import java.net.URL;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicLong;

/**
 * 分块 Range 下载 + AES-CTR 边收边解密。多个文件并发，每个文件多条连接，
 * 与桌面版 download.rs 的策略一致：4MB 分块、短退避重试、G/EU 线路互换。
 */
public class Downloader {

    public interface Listener {
        void onLog(String line);

        void onFinished(int ok, int fail, int skip, boolean stopped);
    }

    public static final class Job {
        public final MegaApi.Node node;
        public final boolean fromFolder;
        public final String relPath;

        public Job(MegaApi.Node node, boolean fromFolder, String relPath) {
            this.node = node;
            this.fromFolder = fromFolder;
            this.relPath = relPath;
        }
    }

    private interface ChunkSink {
        void onChunk(long bytes);
    }

    private final MegaApi api;
    private final int connections;
    private final int parallel;

    private volatile boolean cancelled;
    private final AtomicLong totalDone = new AtomicLong();
    private final AtomicLong totalBytes = new AtomicLong();
    private final List<String> logs = new ArrayList<>();

    public Downloader(MegaApi api, int connections, int parallelFiles) {
        this.api = api;
        this.connections = Math.max(1, connections);
        this.parallel = Math.max(1, parallelFiles);
    }

    public long getTotalDone() {
        return totalDone.get();
    }

    public long getTotalBytes() {
        return totalBytes.get();
    }

    public void cancel() {
        cancelled = true;
    }

    public boolean isCancelled() {
        return cancelled;
    }

    public List<String> getLogs() {
        synchronized (logs) {
            return new ArrayList<>(logs);
        }
    }

    public void start(final List<Job> jobs, final Storage.Store store, final String folderId, final Listener listener) {
        totalDone.set(0);
        totalBytes.set(0);
        long sum = 0;
        for (Job j : jobs) sum += j.node.size;
        totalBytes.set(sum);
        final AtomicInteger okCount = new AtomicInteger();
        final AtomicInteger failCount = new AtomicInteger();
        final AtomicInteger skipCount = new AtomicInteger();
        final AtomicBoolean stopped = new AtomicBoolean();

        Thread orchestrator = new Thread(new Runnable() {
            @Override
            public void run() {
                final Object queueLock = new Object();
                final AtomicInteger cursor = new AtomicInteger();
                List<Thread> workers = new ArrayList<>();
                final CountDownLatch latch = new CountDownLatch(Math.min(parallel, Math.max(1, jobs.size())));
                for (int w = 0; w < Math.min(parallel, Math.max(1, jobs.size())); w++) {
                    Thread t = new Thread(new Runnable() {
                        @Override
                        public void run() {
                            try {
                                while (!cancelled) {
                                    int idx;
                                    synchronized (queueLock) {
                                        idx = cursor.getAndIncrement();
                                    }
                                    if (idx >= jobs.size()) break;
                                    Job job = jobs.get(idx);
                                    int result = downloadOne(job, store, folderId, listener);
                                    if (result == 1) okCount.incrementAndGet();
                                    else if (result == 2) skipCount.incrementAndGet();
                                    else if (result == 3) {
                                        stopped.set(true);
                                        cancelled = true;
                                        break;
                                    } else failCount.incrementAndGet();
                                }
                            } finally {
                                latch.countDown();
                            }
                        }
                    }, "file-worker");
                    workers.add(t);
                    t.start();
                }
                try {
                    latch.await();
                } catch (InterruptedException ignored) {
                    Thread.currentThread().interrupt();
                }
                if (listener != null) {
                    listener.onFinished(okCount.get(), failCount.get(), skipCount.get(), stopped.get() || cancelled);
                }
            }
        }, "batch");
        orchestrator.start();
    }

    /** 返回 1 成功、2 跳过、3 取消、0 失败。 */
    private int downloadOne(Job job, Storage.Store store, String folderId, Listener listener) {
        MegaApi.Node node = job.node;
        node.state = 1;
        node.downloaded = 0;
        Storage.Target target = null;
        try {
            MegaApi.FileInfo info = api.fileInfo(
                    job.fromFolder ? null : node.handle,
                    job.fromFolder ? node.handle : null,
                    job.fromFolder ? folderId : null);
            node.size = info.size;
            node.state = 2;
            String rel = job.relPath == null ? "" : job.relPath;
            target = store.create(rel, Mega.safeName(node.name), info.size);
            final Storage.Target out = target;

            if (info.size == 0) {
                out.commit();
                target = null;
                node.state = 3;
                node.downloaded = 0;
                return 1;
            }

            long partSize = effectivePartSize(info.size);
            long totalParts = (info.size + partSize - 1) / partSize;
            int threads = (int) Math.min(connections, totalParts);

            final AtomicInteger nextPart = new AtomicInteger();
            final AtomicBoolean failed = new AtomicBoolean();
            final AtomicBoolean localCancel = new AtomicBoolean();
            final Object failLock = new Object();
            final String[] errHolder = new String[]{null};
            final String url = info.url;
            final byte[] key = node.key;
            final long size = info.size;
            final ChunkSink sink = new ChunkSink() {
                @Override
                public void onChunk(long bytes) {
                    synchronized (node) {
                        node.downloaded += bytes;
                    }
                    totalDone.addAndGet(bytes);
                }
            };

            ExecutorService pool = Executors.newFixedThreadPool(threads);
            CountDownLatch latch = new CountDownLatch(threads);
            for (int t = 0; t < threads; t++) {
                pool.execute(new Runnable() {
                    @Override
                    public void run() {
                        try {
                            while (!cancelled && !localCancel.get()) {
                                int idx = nextPart.getAndIncrement();
                                if (idx >= totalParts) break;
                                long start = idx * partSize;
                                long end = Math.min(start + partSize, size) - 1;
                                try {
                                    fetchPart(url, key, start, end, out, localCancel, sink);
                                } catch (Mega.MegaError e) {
                                    synchronized (failLock) {
                                        if (errHolder[0] == null) errHolder[0] = e.getMessage();
                                    }
                                    localCancel.set(true);
                                    failed.set(true);
                                    break;
                                }
                            }
                        } finally {
                            latch.countDown();
                        }
                    }
                });
            }
            pool.shutdown();
            try {
                latch.await();
                pool.awaitTermination(30, TimeUnit.SECONDS);
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
            }
            pool.shutdownNow();

            if (cancelled) {
                out.abort();
                target = null;
                node.state = 5;
                return 3;
            }
            if (failed.get() || errHolder[0] != null) {
                out.abort();
                target = null;
                node.state = 4;
                node.error = errHolder[0] == null ? "分块下载失败" : errHolder[0];
                if (listener != null) listener.onLog("失败：" + node.name + " - " + node.error);
                return 0;
            }
            out.commit();
            target = null;
            node.state = 3;
            node.downloaded = size;
            if (listener != null) listener.onLog("完成：" + node.name + " → " + out.path());
            return 1;
        } catch (Mega.MegaError e) {
            if (target != null) target.abort();
            node.state = 4;
            node.error = e.getMessage();
            if (listener != null) listener.onLog("失败：" + node.name + " - " + e.getMessage());
            return 0;
        } catch (Exception e) {
            if (target != null) target.abort();
            node.state = 4;
            node.error = e.getClass().getSimpleName();
            if (listener != null) listener.onLog("失败：" + node.name + " - " + e.getClass().getSimpleName());
            return 0;
        }
    }

    private long effectivePartSize(long size) {
        long target = size / ((long) connections * 4);
        long capped = Math.min(target, 4L * 1024 * 1024);
        long aligned = Math.max(16 * 1024, (capped / 16384) * 16384);
        return aligned;
    }

    private void fetchPart(String rawUrl, byte[] key, long start, long end, Storage.Target sinkTarget,
                           AtomicBoolean localCancel, ChunkSink sink) throws Mega.MegaError {
        List<String> candidates = new ArrayList<>();
        candidates.add(rawUrl);
        if (rawUrl.contains("g.api.mega.co.nz")) {
            candidates.add(rawUrl.replace("g.api.mega.co.nz", "eu.api.mega.co.nz"));
        } else if (rawUrl.contains("eu.api.mega.co.nz")) {
            candidates.add(rawUrl.replace("eu.api.mega.co.nz", "g.api.mega.co.nz"));
        }
        long want = end - start + 1;
        String last = "";
        for (int attempt = 0; attempt < 5; attempt++) {
            if (cancelled || localCancel.get()) throw new Mega.MegaError("已取消");
            String urlCandidate = candidates.get(attempt % candidates.size());
            HttpURLConnection conn = null;
            try {
                conn = (HttpURLConnection) new URL(urlCandidate).openConnection();
                conn.setRequestMethod("GET");
                conn.setRequestProperty("Range", "bytes=" + start + "-" + end);
                conn.setRequestProperty("User-Agent", "MegaDL-Android/0.2");
                conn.setConnectTimeout(15000);
                conn.setReadTimeout(60000);
                int status = conn.getResponseCode();
                if (status == 509) throw new Mega.MegaError("EOVERQUOTA：带宽/配额超限");
                if (status == 416) throw new Mega.MegaError("ERANGE：Range 不被接受");
                if (status == 200 && start != 0) throw new Mega.MegaError("服务器忽略了 Range");
                if (status != 200 && status != 206) {
                    last = "HTTP " + status;
                } else {
                    InputStream in = conn.getInputStream();
                    byte[] buf = new byte[64 * 1024];
                    long pos = 0;
                    boolean shortRead = false;
                    while (pos < want) {
                        if (cancelled || localCancel.get()) throw new Mega.MegaError("已取消");
                        int block = (int) Math.min(buf.length, want - pos);
                        int got = 0;
                        while (got < block) {
                            int n = in.read(buf, got, block - got);
                            if (n <= 0) break;
                            got += n;
                        }
                        if (got < block) {
                            shortRead = true;
                            break;
                        }
                        // 每个块都从 16 字节对齐的绝对偏移开始，CTR 才能正确接续
                        byte[] plain = Mega.ctrDecrypt(buf, key, start + pos, got);
                        sinkTarget.write(plain, got, start + pos);
                        sink.onChunk(got);
                        pos += got;
                    }
                    in.close();
                    if (shortRead || pos != want) {
                        last = "分块长度不符：需要 " + want + "，收到 " + pos;
                    } else {
                        return;
                    }
                }
            } catch (Mega.MegaError e) {
                if ("已取消".equals(e.getMessage())) throw e;
                last = e.getMessage();
                if (last != null && last.contains("EOVERQUOTA")) throw e;
            } catch (IOException e) {
                String msg = e.getMessage() == null ? "" : e.getMessage();
                last = "网络错误：" + e.getClass().getSimpleName() + (msg.isEmpty() ? "" : " " + msg);
            } catch (Exception e) {
                String msg = e.getMessage() == null ? "" : e.getMessage();
                last = "读取失败：" + e.getClass().getSimpleName() + (msg.isEmpty() ? "" : " " + msg);
            } finally {
                if (conn != null) conn.disconnect();
            }
            sleep(120L * (1L << Math.min(attempt, 4)) + (System.nanoTime() % 90));
        }
        throw new Mega.MegaError("分块下载失败：" + last);
    }

    private static void sleep(long ms) {
        try {
            Thread.sleep(ms);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
        }
    }
}
