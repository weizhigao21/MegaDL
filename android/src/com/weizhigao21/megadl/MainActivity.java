package com.weizhigao21.megadl;

import android.app.Activity;
import android.app.AlertDialog;
import android.content.ClipData;
import android.content.ClipboardManager;
import android.content.Context;
import android.graphics.Color;
import android.graphics.Typeface;
import android.graphics.drawable.GradientDrawable;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.text.InputType;
import android.text.TextUtils;
import android.util.TypedValue;
import android.view.Gravity;
import android.view.View;
import android.view.ViewGroup;
import android.view.WindowManager;
import android.widget.EditText;
import android.widget.LinearLayout;
import android.widget.ListView;
import android.widget.ProgressBar;
import android.widget.ScrollView;
import android.widget.TextView;
import android.widget.Toast;

import java.io.File;
import java.util.ArrayList;
import java.util.List;
import java.util.Locale;

public class MainActivity extends Activity implements TreeAdapter.Host {

    private static final int BG = Color.rgb(0xF2, 0xF4, 0xF2);
    private static final int CARD = Color.WHITE;
    private static final int BORDER = Color.rgb(0xE1, 0xE7, 0xE3);
    private static final int TEXT = Color.rgb(0x1B, 0x23, 0x20);
    private static final int MUTED = Color.rgb(0x8A, 0x96, 0x90);
    private static final int ACCENT = Color.rgb(0x1F, 0x6B, 0xEB);
    private static final int DANGER = Color.rgb(0xD1, 0x43, 0x43);
    private static final int SUCCESS = Color.rgb(0x1E, 0x9C, 0x6A);
    private static final int FIELD = Color.rgb(0xF7, 0xF8, 0xF7);

    private final Handler ui = new Handler(Looper.getMainLooper());
    private MegaApi api = new MegaApi(false);
    private Mega.Link link;
    private List<MegaApi.Node> roots = new ArrayList<>();
    private TreeAdapter adapter;

    private EditText input;
    private ListView list;
    private TextView fileSummary;
    private TextView countValue;
    private TextView sizeValue;
    private TextView speedValue;
    private TextView speedUnit;
    private TextView progressText;
    private ProgressBar progress;
    private TextView downloadButton;
    private TextView stopButton;
    private TextView statusDot;
    private TextView statusText;
    private TextView pathText;

    private Downloader engine;
    private volatile boolean busy;
    private volatile boolean parsing;
    private long batchTotal;
    private long lastDone;
    private long lastSampleAt;
    private double smoothSpeed;
    private final List<String> logs = new ArrayList<>();

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        getWindow().setStatusBarColor(BG);
        getWindow().setNavigationBarColor(BG);
        buildUi();
        refreshSelectable();
    }

    // ------------------------------------------------------------ 界面搭建

    private void buildUi() {
        LinearLayout root = new LinearLayout(this);
        root.setOrientation(LinearLayout.VERTICAL);
        root.setBackgroundColor(BG);
        root.setPadding(dp(14), dp(10), dp(14), dp(10));
        setContentView(root);

        root.addView(buildHeader(), lp(-1, -2, 0, 4, 0, 8));

        LinearLayout linkCard = card();
        linkCard.addView(label("链接", 13, TEXT, true), lp(-1, -2, 0, 0, 0, 8));
        LinearLayout inputRow = new LinearLayout(this);
        inputRow.setOrientation(LinearLayout.HORIZONTAL);
        inputRow.setGravity(Gravity.CENTER_VERTICAL);
        input = new EditText(this);
        input.setSingleLine(true);
        input.setTextSize(13);
        input.setTextColor(TEXT);
        input.setHintTextColor(MUTED);
        input.setHint("https://mega.nz/folder/… 或 /file/…");
        input.setInputType(InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_VARIATION_URI);
        input.setBackground(round(FIELD, 10, 1, BORDER));
        input.setPadding(dp(12), dp(8), dp(12), dp(8));
        inputRow.addView(input, lp(0, dp(44), 1f, 0, 8, 0));
        TextView paste = pill("粘贴", false, false);
        paste.setTextSize(13);
        paste.setOnClickListener(v -> paste());
        inputRow.addView(paste, lp(dp(62), dp(44), 0f, 0, 8, 0));
        TextView parse = pill("解析", true, false);
        parse.setTextSize(14);
        parse.setOnClickListener(v -> doParse());
        inputRow.addView(parse, lp(dp(74), dp(44), 0f, 0, 0, 0));
        linkCard.addView(inputRow);
        root.addView(linkCard, lp(-1, -2, 0, 0, 0, 10));

        LinearLayout fileCard = card();
        fileSummary = label("文件", 13.5f, TEXT, true);
        fileCard.addView(fileSummary, lp(-1, dp(28), 0, 0, 0, 4));

        LinearLayout fileActions = new LinearLayout(this);
        fileActions.setOrientation(LinearLayout.HORIZONTAL);
        fileActions.addView(actionChip("展开", v -> expandAll(true)), lp(0, dp(32), 1f, 0, 5, 0));
        fileActions.addView(actionChip("收起", v -> expandAll(false)), lp(0, dp(32), 1f, 0, 5, 0));
        fileActions.addView(actionChip("全选", v -> selectAll(true)), lp(0, dp(32), 1f, 0, 5, 0));
        fileActions.addView(actionChip("全不选", v -> selectAll(false)), lp(0, dp(32), 1f, 0, 0, 0));
        fileCard.addView(fileActions, lp(-1, -2, 0, 0, 0, 6));

        list = new ListView(this);
        list.setDivider(null);
        list.setDividerHeight(0);
        list.setBackgroundColor(CARD);
        list.setCacheColorHint(Color.TRANSPARENT);
        adapter = new TreeAdapter(this, this);
        list.setAdapter(adapter);
        fileCard.addView(list, lp(-1, 0, 1f, 0, 0, 0));

        TextView emptyHint = label("粘贴 MEGA 链接后点「解析」\n文件树会显示在这里", 12, MUTED, false);
        emptyHint.setGravity(Gravity.CENTER);
        emptyHint.setLineSpacing(dp(4), 1f);
        fileCard.addView(emptyHint, lp(-1, 0, 1f, 0, 0, 0));
        list.setEmptyView(emptyHint);
        root.addView(fileCard, lp(-1, 0, 1f, 0, 0, 8));

        root.addView(buildStatsCard(), lp(-1, -2, 0, 0, 0, 6));

        LinearLayout statusRow = new LinearLayout(this);
        statusRow.setOrientation(LinearLayout.HORIZONTAL);
        statusRow.setGravity(Gravity.CENTER_VERTICAL);
        statusRow.setPadding(dp(4), 0, dp(4), 0);
        statusDot = label("●", 10, SUCCESS, false);
        statusRow.addView(statusDot);
        statusText = label("就绪", 12, MUTED, false);
        statusRow.addView(statusText, lp(0, -2, 1f, 6, 0, 0));
        root.addView(statusRow, lp(-1, dp(26), 0f, 0, 0, 0));
    }

    private View buildHeader() {
        LinearLayout header = new LinearLayout(this);
        header.setOrientation(LinearLayout.HORIZONTAL);
        header.setGravity(Gravity.CENTER_VERTICAL);
        android.widget.ImageView icon = new android.widget.ImageView(this);
        icon.setImageResource(R.mipmap.ic_launcher);
        header.addView(icon, lp(dp(40), dp(40), 0f, 0, 10, 0));
        LinearLayout titles = new LinearLayout(this);
        titles.setOrientation(LinearLayout.VERTICAL);
        titles.addView(label("MegaDL", 20, TEXT, true));
        titles.addView(label("MEGA 公共链接下载器 · v0.2.1", 11.5f, MUTED, false));
        header.addView(titles, lp(-2, -2, 1f, 0, 0, 0));
        TextView logBtn = action("日志", v -> showLogs());
        logBtn.setPadding(dp(10), dp(6), dp(10), dp(6));
        header.addView(logBtn, lp(-2, dp(34), 0f, 0, 0, 0));
        return header;
    }

    private View buildStatsCard() {
        LinearLayout card = card();

        LinearLayout metrics = new LinearLayout(this);
        metrics.setOrientation(LinearLayout.HORIZONTAL);
        metrics.setGravity(Gravity.CENTER_VERTICAL);
        metrics.setPadding(0, dp(4), 0, dp(8));

        LinearLayout c1 = metricColumn("0", null, "已选文件");
        countValue = (TextView) c1.getTag();
        metrics.addView(c1, lp(0, dp(52), 1f, 0, 0, 0));

        LinearLayout c2 = metricColumn("0B", null, "总大小");
        sizeValue = (TextView) c2.getTag();
        metrics.addView(c2, lp(0, dp(52), 1f, 0, 0, 0));

        LinearLayout c3 = metricColumnSpeed("0", "B/s", "实时速度");
        speedValue = (TextView) c3.getTag();
        speedUnit = (TextView) c3.getTag(R.id.speed_unit);
        metrics.addView(c3, lp(0, dp(52), 1f, 0, 0, 0));
        card.addView(metrics);

        progress = new ProgressBar(this, null, android.R.attr.progressBarStyleHorizontal);
        progress.setMax(1000);
        progress.setProgress(0);
        progress.setProgressTintList(android.content.res.ColorStateList.valueOf(ACCENT));
        card.addView(progress, lp(-1, dp(6), 0f, 0, 0, 4));

        LinearLayout progressRow = new LinearLayout(this);
        progressRow.setOrientation(LinearLayout.HORIZONTAL);
        progressRow.setGravity(Gravity.CENTER_VERTICAL);
        progressText = label("", 11.5f, MUTED, false);
        progressText.setText("未开始");
        progressRow.addView(progressText, lp(0, -2, 1f, 0, 8, 0));
        pathText = label("8 连接 × 3 文件", 10, MUTED, false);
        pathText.setSingleLine(true);
        pathText.setGravity(Gravity.END);
        progressRow.addView(pathText, lp(-2, -2, 0f, 0, 0, 0));
        card.addView(progressRow, lp(-1, dp(24), 0, 0, 0, 10));

        LinearLayout buttons = new LinearLayout(this);
        buttons.setOrientation(LinearLayout.HORIZONTAL);
        downloadButton = pill("下载选中", true, false);
        downloadButton.setTextSize(14.5f);
        downloadButton.setOnClickListener(v -> startDownload());
        buttons.addView(downloadButton, lp(0, dp(48), 1.45f, 0, 10, 0));
        stopButton = pill("停止", false, true);
        stopButton.setTextSize(14.5f);
        stopButton.setOnClickListener(v -> stopDownload());
        buttons.addView(stopButton, lp(0, dp(48), 1f, 0, 0, 0));
        card.addView(buttons);

        LinearLayout pathRow = new LinearLayout(this);
        pathRow.setOrientation(LinearLayout.HORIZONTAL);
        pathRow.setGravity(Gravity.CENTER_VERTICAL);
        pathRow.setPadding(0, dp(10), 0, 0);
        pathRow.addView(label("保存到", 10.5f, MUTED, false));
        TextView savePath = label(saveDir().getAbsolutePath(), 10, MUTED, false);
        savePath.setSingleLine(true);
        savePath.setEllipsize(TextUtils.TruncateAt.MIDDLE);
        savePath.setPadding(dp(8), 0, dp(8), 0);
        pathRow.addView(savePath, lp(0, -2, 1f, 0, 0, 0));
        TextView copyBtn = actionChip("复制", v -> {
            ClipboardManager cm = (ClipboardManager) getSystemService(Context.CLIPBOARD_SERVICE);
            if (cm != null) {
                cm.setPrimaryClip(ClipData.newPlainText("MegaDL 保存路径", saveDir().getAbsolutePath()));
                toast("路径已复制");
            }
        });
        pathRow.addView(copyBtn, lp(dp(56), dp(30), 0f, 0, 0, 0));
        card.addView(pathRow);
        return card;
    }

    private LinearLayout metricColumn(String value, String unit, String caption) {
        LinearLayout col = new LinearLayout(this);
        col.setOrientation(LinearLayout.VERTICAL);
        col.setGravity(Gravity.CENTER_HORIZONTAL);
        TextView v = label(value, 22, TEXT, true);
        v.setSingleLine(true);
        col.addView(v);
        col.addView(label(caption, 11, MUTED, false));
        col.setTag(v);
        return col;
    }

    private LinearLayout metricColumnSpeed(String value, String unit, String caption) {
        LinearLayout col = new LinearLayout(this);
        col.setOrientation(LinearLayout.VERTICAL);
        col.setGravity(Gravity.CENTER_HORIZONTAL);
        LinearLayout line = new LinearLayout(this);
        line.setOrientation(LinearLayout.HORIZONTAL);
        line.setGravity(Gravity.CENTER_VERTICAL);
        TextView v = label(value, 22, TEXT, true);
        v.setSingleLine(true);
        TextView u = label(unit, 10.5f, MUTED, false);
        line.addView(v);
        LinearLayout.LayoutParams ulp = new LinearLayout.LayoutParams(-2, -2);
        ulp.leftMargin = dp(3);
        line.addView(u, ulp);
        col.addView(line);
        col.addView(label(caption, 11, MUTED, false));
        col.setTag(v);
        col.setTag(R.id.speed_unit, u);
        return col;
    }

    // ------------------------------------------------------------ 解析

    private void paste() {
        ClipboardManager cm = (ClipboardManager) getSystemService(Context.CLIPBOARD_SERVICE);
        if (cm == null) return;
        ClipData data = cm.getPrimaryClip();
        if (data != null && data.getItemCount() > 0) {
            CharSequence text = data.getItemAt(0).coerceToText(this);
            if (text != null) {
                input.setText(text.toString().trim());
                input.setSelection(0);
            }
        }
    }

    private void doParse() {
        if (busy || parsing) return;
        String raw = input.getText().toString().trim();
        if (raw.isEmpty()) {
            toast("先粘贴 MEGA 链接");
            return;
        }
        parsing = true;
        setStatus("正在解析链接…", ACCENT);
        final Mega.Link parsed;
        try {
            parsed = Mega.parseLink(raw);
        } catch (Mega.MegaError e) {
            parsing = false;
            failStatus(e.getMessage());
            return;
        }
        new Thread(() -> {
            try {
                List<MegaApi.Node> newRoots;
                List<String> manyLogs = new ArrayList<>();
                if (parsed.kind == Mega.Kind.FOLDER) {
                    List<MegaApi.Node> flat = api.enrichFolder(parsed);
                    newRoots = MegaApi.buildTree(flat, parsed.id);
                    manyLogs.add("解析完成：" + flat.size() + " 个节点");
                } else {
                    byte[] key = Mega.linkKey(parsed);
                    MegaApi.FileInfo info = api.fileInfo(parsed.id, null, null);
                    String name = Mega.decryptAttrs(info.at, key).optString("n", parsed.id);
                    MegaApi.Node node = new MegaApi.Node();
                    node.name = name;
                    node.handle = parsed.id;
                    node.size = info.size;
                    node.key = key;
                    node.dir = false;
                    node.checked = true;
                    node.expanded = false;
                    newRoots = new ArrayList<>();
                    newRoots.add(node);
                    manyLogs.add("解析完成：" + name);
                }
                ui.post(() -> {
                    link = parsed;
                    roots = newRoots;
                    for (MegaApi.Node n : roots) {
                        if (n.dir) {
                            n.expanded = true;
                            setAll(n, true);
                        }
                    }
                    parsing = false;
                    adapter.setRoots(roots);
                    refreshSelectable();
                    setBusyUi(false);
                    setStatus("解析完成", SUCCESS);
                    appendLogs(manyLogs);
                });
            } catch (Exception e) {
                ui.post(() -> {
                    parsing = false;
                    failStatus(e.getMessage() == null ? "解析失败" : e.getMessage());
                    appendLogs(java.util.Collections.singletonList("解析失败：" + e.getMessage()));
                });
            }
        }, "parse").start();
    }

    private void appendLogs(List<String> lines) {
        synchronized (logs) {
            for (String l : lines) {
                if (l != null) logs.add(l);
            }
            while (logs.size() > 200) logs.remove(0);
        }
    }

    private void failStatus(String msg) {
        setStatus("解析失败：" + msg, DANGER);
        appendLogs(java.util.Collections.singletonList("解析失败：" + msg));
    }

    // ------------------------------------------------------------ 选择与树

    private void setAll(MegaApi.Node node, boolean checked) {
        if (!node.dir) {
            node.checked = checked;
        } else {
            for (MegaApi.Node c : node.children) setAll(c, checked);
        }
    }

    private void selectAll(boolean checked) {
        for (MegaApi.Node n : roots) setAll(n, checked);
        adapter.rebuild(roots);
        refreshSelectable();
    }

    private void expandAll(boolean expanded) {
        setExpand(roots, expanded);
        adapter.rebuild(roots);
    }

    private void setExpand(List<MegaApi.Node> list, boolean expanded) {
        for (MegaApi.Node n : list) {
            if (n.dir) {
                n.expanded = expanded;
                setExpand(n.children, expanded);
            }
        }
    }

    private List<MegaApi.Node> selectedFiles() {
        List<MegaApi.Node> out = new ArrayList<>();
        collectSelected(roots, out);
        return out;
    }

    private void collectSelected(List<MegaApi.Node> list, List<MegaApi.Node> out) {
        for (MegaApi.Node n : list) {
            if (n.dir) collectSelected(n.children, out);
            else if (n.checked) out.add(n);
        }
    }

    private void refreshSelectable() {
        List<MegaApi.Node> picked = selectedFiles();
        long size = 0;
        for (MegaApi.Node n : picked) size += n.size;
        countValue.setText(String.valueOf(picked.size()));
        sizeValue.setText(Mega.fmtSize(size));
        int files = 0;
        int dirs = 0;
        for (MegaApi.Node n : roots) {
            files += TreeAdapter.countFiles(n);
            dirs += TreeAdapter.countDirs(n);
        }
        fileSummary.setText(roots.isEmpty() ? "文件" : "文件  " + dirs + " 个文件夹 · " + files + " 个文件");
        downloadButton.setEnabled(!busy && !picked.isEmpty());
        downloadButton.setAlpha(busy || picked.isEmpty() ? 0.5f : 1f);
    }

    @Override
    public void onToggleFile(MegaApi.Node node) {
        node.checked = !node.checked;
        adapter.rebuild(roots);
        refreshSelectable();
    }

    @Override
    public void onToggleFolder(MegaApi.Node node) {
        int state = TreeAdapter.countSelected(node);
        boolean target = state != TreeAdapter.countFiles(node);
        setAll(node, target);
        adapter.rebuild(roots);
        refreshSelectable();
    }

    @Override
    public void onExpand(MegaApi.Node node) {
        node.expanded = !node.expanded;
        adapter.rebuild(roots);
    }

    @Override
    public void onSelectionChanged() {
        refreshSelectable();
    }

    // ------------------------------------------------------------ 下载

    private File saveDir() {
        File dir = getExternalFilesDir("MegaDL");
        if (dir == null) dir = new File(getFilesDir(), "MegaDL");
        if (!dir.exists()) dir.mkdirs();
        return dir;
    }

    private void startDownload() {
        if (busy || parsing) return;
        List<MegaApi.Node> picked = selectedFiles();
        if (picked.isEmpty()) {
            toast("先勾选要下载的文件");
            return;
        }
        List<Downloader.Job> jobs = new ArrayList<>();
        batchTotal = 0;
        for (MegaApi.Node n : picked) {
            n.state = 0;
            n.downloaded = 0;
            jobs.add(new Downloader.Job(n, link.kind == Mega.Kind.FOLDER, relPath(n)));
            batchTotal += n.size;
        }
        engine = new Downloader(api, 8, 3);
        busy = true;
        lastDone = 0;
        lastSampleAt = System.currentTimeMillis();
        smoothSpeed = 0;
        progress.setProgress(0);
        progressText.setText("0B / " + Mega.fmtSize(batchTotal));
        pathText.setText("8 连接 × 3 文件");
        setBusyUi(true);
        setStatus("开始下载 " + jobs.size() + " 个文件", ACCENT);
        appendLogs(java.util.Collections.singletonList("开始下载 " + jobs.size() + " 个文件（8 连接 × 3 文件，4MB 分块）"));
        getWindow().addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
        engine.start(jobs, saveDir(), link.kind == Mega.Kind.FOLDER ? link.id : null, new Downloader.Listener() {
            @Override
            public void onLog(String line) {
                appendLogs(java.util.Collections.singletonList(line));
            }

            @Override
            public void onFinished(int ok, int fail, int skip, boolean stopped) {
                ui.post(() -> {
                    busy = false;
                    getWindow().clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
                    smoothSpeed = 0;
                    setBusyUi(false);
                    refreshSelectable();
                    adapter.rebuild(roots);
                    long done = engine == null ? 0 : engine.getTotalDone();
                    if (stopped) {
                        int permille = batchTotal <= 0 ? 0 : (int) Math.min(1000, done * 1000 / batchTotal);
                        progress.setProgress(permille);
                        progressText.setText(Mega.fmtSize(done) + " / " + Mega.fmtSize(batchTotal));
                    } else {
                        progress.setProgress(1000);
                        progressText.setText(Mega.fmtSize(batchTotal) + " / " + Mega.fmtSize(batchTotal));
                    }
                    String text;
                    if (stopped) text = "已停止：完成 " + ok + "，失败 " + fail;
                    else text = "完成 " + ok + (fail > 0 ? "，失败 " + fail : "") + (skip > 0 ? "，跳过 " + skip : "");
                    setStatus(text, fail > 0 ? DANGER : SUCCESS);
                    appendLogs(java.util.Collections.singletonList(text));
                });
            }
        });
        startTicker();
    }

    private void stopDownload() {
        if (engine != null) {
            engine.cancel();
            setStatus("正在停止…", MUTED);
        }
    }

    private String relPath(MegaApi.Node file) {
        List<String> parts = new ArrayList<>();
        MegaApi.Node p = file.parentNode;
        while (p != null) {
            parts.add(0, Mega.safeName(p.name));
            p = p.parentNode;
        }
        StringBuilder sb = new StringBuilder();
        for (String s : parts) {
            if (sb.length() > 0) sb.append('/');
            sb.append(s);
        }
        return sb.toString();
    }

    private void startTicker() {
        ui.post(new Runnable() {
            @Override
            public void run() {
                if (!busy) return;
                long now = System.currentTimeMillis();
                long done = engine == null ? 0 : engine.getTotalDone();
                double dt = Math.max(0.001, (now - lastSampleAt) / 1000.0);
                double inst = Math.max(0, done - lastDone) / dt;
                double alpha = 1.0 - Math.exp(-dt / 1.6);
                smoothSpeed = smoothSpeed * (1 - alpha) + inst * alpha;
                lastDone = done;
                lastSampleAt = now;

                int permille = batchTotal <= 0 ? 0 : (int) Math.min(1000, done * 1000 / batchTotal);
                progress.setProgress(permille);
                progressText.setText(Mega.fmtSize(done) + " / " + Mega.fmtSize(batchTotal));
                setSpeedText(smoothSpeed);
                adapter.rebuild(roots);
                ui.postDelayed(this, 500);
            }
        });
    }

    private void setSpeedText(double speed) {
        if (speed < 1024) {
            speedValue.setText("0");
            speedUnit.setText("B/s");
        } else if (speed < 1024 * 1024) {
            speedValue.setText(String.format(Locale.US, "%.0f", speed / 1024));
            speedUnit.setText("KB/s");
        } else if (speed < 1024L * 1024 * 1024) {
            speedValue.setText(String.format(Locale.US, "%.1f", speed / (1024 * 1024)));
            speedUnit.setText("MB/s");
        } else {
            speedValue.setText(String.format(Locale.US, "%.2f", speed / (1024.0 * 1024 * 1024)));
            speedUnit.setText("GB/s");
        }
    }

    private void setBusyUi(boolean isBusy) {
        stopButton.setEnabled(isBusy);
        stopButton.setAlpha(isBusy ? 1f : 0.45f);
        downloadButton.setEnabled(!isBusy && countValue != null && !selectedFiles().isEmpty());
        downloadButton.setAlpha(isBusy || selectedFiles().isEmpty() ? 0.5f : 1f);
        if (!isBusy) {
            setSpeedText(0);
        }
    }

    private void setStatus(String text, int color) {
        statusText.setText(text);
        statusText.setTextColor(color == ACCENT ? MUTED : color);
        statusDot.setTextColor(color);
        if (color == DANGER) statusText.setTextColor(DANGER);
    }

    private void showLogs() {
        List<String> all;
        synchronized (logs) {
            all = new ArrayList<>(logs);
        }
        if (engine != null) {
            for (String l : engine.getLogs()) if (!all.contains(l)) all.add(l);
        }
        StringBuilder sb = new StringBuilder();
        for (String l : all) sb.append(l).append('\n');
        ScrollView sv = new ScrollView(this);
        TextView tv = label(sb.length() == 0 ? "暂无日志" : sb.toString(), 12, TEXT, false);
        tv.setPadding(dp(16), dp(12), dp(16), dp(12));
        tv.setTextIsSelectable(true);
        sv.addView(tv);
        new AlertDialog.Builder(this)
                .setTitle("日志")
                .setView(sv)
                .setPositiveButton("关闭", null)
                .show();
    }

    // ------------------------------------------------------------ 小组件

    private LinearLayout card() {
        LinearLayout box = new LinearLayout(this);
        box.setOrientation(LinearLayout.VERTICAL);
        box.setPadding(dp(14), dp(14), dp(14), dp(14));
        box.setBackground(round(CARD, 16, 1, BORDER));
        return box;
    }

    private TextView label(String text, float size, int color, boolean bold) {
        TextView tv = new TextView(this);
        tv.setText(text);
        tv.setTextSize(size);
        tv.setTextColor(color);
        if (bold) tv.setTypeface(Typeface.create("sans-serif", Typeface.BOLD));
        return tv;
    }

    private TextView action(String text, View.OnClickListener click) {
        TextView tv = label(text, 11.5f, ACCENT, false);
        tv.setGravity(Gravity.CENTER);
        tv.setPadding(dp(8), dp(4), dp(8), dp(4));
        tv.setOnClickListener(click);
        return tv;
    }

    private TextView actionChip(String text, View.OnClickListener click) {
        TextView tv = label(text, 12, ACCENT, false);
        tv.setGravity(Gravity.CENTER);
        tv.setSingleLine(true);
        tv.setPadding(dp(4), 0, dp(4), 0);
        tv.setBackground(round(FIELD, 8, 1, BORDER));
        tv.setClickable(true);
        tv.setOnClickListener(click);
        return tv;
    }

    private TextView pill(String text, boolean primary, boolean danger) {
        TextView tv = new TextView(this);
        tv.setText(text);
        tv.setTextSize(13.5f);
        tv.setGravity(Gravity.CENTER);
        tv.setTypeface(Typeface.create("sans-serif", Typeface.BOLD));
        if (primary) {
            tv.setTextColor(Color.WHITE);
            tv.setBackground(round(ACCENT, 10, 0, 0));
        } else if (danger) {
            tv.setTextColor(DANGER);
            tv.setBackground(round(Color.rgb(0xFB, 0xEC, 0xEC), 10, 1, Color.rgb(0xF0, 0xC8, 0xC8)));
        } else {
            tv.setTextColor(TEXT);
            tv.setBackground(round(FIELD, 10, 1, BORDER));
        }
        tv.setClickable(true);
        return tv;
    }

    private GradientDrawable round(int color, float radius, int strokeWidth, int strokeColor) {
        GradientDrawable d = new GradientDrawable();
        d.setColor(color);
        d.setCornerRadius(dp(radius));
        if (strokeWidth > 0) d.setStroke(dp(strokeWidth), strokeColor);
        return d;
    }

    private LinearLayout.LayoutParams lp(int w, int h, float weight, int left, int right, int bottom) {
        LinearLayout.LayoutParams p = new LinearLayout.LayoutParams(w, h);
        p.weight = weight;
        p.setMargins(dp(left), 0, dp(right), dp(bottom));
        return p;
    }

    private int dp(float v) {
        return (int) TypedValue.applyDimension(TypedValue.COMPLEX_UNIT_DIP, v, getResources().getDisplayMetrics());
    }

    private void toast(String s) {
        Toast.makeText(this, s, Toast.LENGTH_SHORT).show();
    }

    @Override
    public void onBackPressed() {
        if (busy) {
            new AlertDialog.Builder(this)
                    .setTitle("仍在下载")
                    .setMessage("退出会中断正在进行的下载，确定退出？")
                    .setPositiveButton("退出", (d, w) -> {
                        if (engine != null) engine.cancel();
                        finish();
                    })
                    .setNegativeButton("继续下载", null)
                    .show();
        } else {
            super.onBackPressed();
        }
    }
}
