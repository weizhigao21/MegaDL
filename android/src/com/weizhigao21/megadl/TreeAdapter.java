package com.weizhigao21.megadl;

import android.content.Context;
import android.graphics.Color;
import android.graphics.Typeface;
import android.text.TextUtils;
import android.view.Gravity;
import android.view.View;
import android.view.ViewGroup;
import android.widget.BaseAdapter;
import android.widget.LinearLayout;
import android.widget.TextView;

import java.util.ArrayList;
import java.util.List;

/** 文件树列表：按层级缩进、三态勾选、文件夹展开/收起。 */
public class TreeAdapter extends BaseAdapter {

    public interface Host {
        void onToggleFile(MegaApi.Node node);

        void onToggleFolder(MegaApi.Node node);

        void onExpand(MegaApi.Node node);

        void onSelectionChanged();
    }

    public static class Row {
        public final MegaApi.Node node;
        public final int depth;

        Row(MegaApi.Node node, int depth) {
            this.node = node;
            this.depth = depth;
        }
    }

    private final Context context;
    private final Host host;
    private final List<Row> rows = new ArrayList<>();

    private static final int TEXT = Color.rgb(0x1B, 0x23, 0x20);
    private static final int MUTED = Color.rgb(0x8A, 0x96, 0x90);
    private static final int ACCENT = Color.rgb(0x1F, 0x6B, 0xEB);
    private static final int DANGER = Color.rgb(0xD1, 0x43, 0x43);
    private static final int SUCCESS = Color.rgb(0x1E, 0x9C, 0x6A);

    public TreeAdapter(Context context, Host host) {
        this.context = context;
        this.host = host;
    }

    public void setRoots(List<MegaApi.Node> roots) {
        rebuild(roots);
    }

    public void rebuild(List<MegaApi.Node> roots) {
        rows.clear();
        for (MegaApi.Node n : roots) walk(n, 0);
        notifyDataSetChanged();
    }

    private void walk(MegaApi.Node n, int depth) {
        rows.add(new Row(n, depth));
        if (n.dir && n.expanded) {
            for (MegaApi.Node child : n.children) walk(child, depth + 1);
        }
    }

    public List<Row> rows() {
        return rows;
    }

    @Override
    public int getCount() {
        return rows.size();
    }

    @Override
    public Object getItem(int position) {
        return rows.get(position).node;
    }

    @Override
    public long getItemId(int position) {
        return position;
    }

    @Override
    public View getView(int position, View convertView, ViewGroup parent) {
        Holder holder;
        if (convertView == null) {
            convertView = buildRow();
            holder = (Holder) convertView.getTag();
        } else {
            holder = (Holder) convertView.getTag();
        }
        Row row = rows.get(position);
        MegaApi.Node node = row.node;

        holder.spacer.setLayoutParams(new LinearLayout.LayoutParams(dp(row.depth * 14), 1));
        holder.arrow.setText(node.dir ? (node.expanded ? "▾" : "▸") : "");
        holder.arrow.setVisibility(node.dir ? View.VISIBLE : View.INVISIBLE);
        holder.check.setText(checkGlyph(node));
        holder.check.setTextColor(checkColor(node));
        holder.icon.setText(iconFor(node));
        holder.name.setText(node.name);
        holder.name.setTypeface(Typeface.create("sans-serif", node.dir ? Typeface.BOLD : Typeface.NORMAL));
        holder.name.setTextColor(TEXT);

        if (node.dir) {
            int files = countFiles(node);
            holder.size.setText(files + " 个 · " + Mega.fmtSize(totalSize(node)));
            holder.state.setText("");
        } else {
            holder.size.setText(Mega.fmtSize(node.size));
            holder.state.setText(stateText(node));
            holder.state.setTextColor(stateColor(node));
        }
        holder.size.setTextColor(MUTED);
        holder.arrow.setTextColor(MUTED);

        holder.arrow.setOnClickListener(v -> {
            if (node.dir) host.onExpand(node);
        });
        holder.check.setOnClickListener(v -> {
            if (node.dir) host.onToggleFolder(node);
            else host.onToggleFile(node);
        });
        convertView.setOnClickListener(v -> {
            if (node.dir) host.onExpand(node);
            else host.onToggleFile(node);
        });
        return convertView;
    }

    private View buildRow() {
        LinearLayout row = new LinearLayout(context);
        row.setOrientation(LinearLayout.HORIZONTAL);
        row.setGravity(Gravity.CENTER_VERTICAL);
        row.setPadding(dp(8), 0, dp(10), 0);
        row.setLayoutParams(new ViewGroup.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, dp(44)));
        row.setBackgroundColor(Color.TRANSPARENT);
        row.setMinimumHeight(0);
        row.setTag(new Holder());

        Holder h = (Holder) row.getTag();

        h.spacer = new View(context);
        row.addView(h.spacer, new LinearLayout.LayoutParams(0, 1));

        h.arrow = textView(14, MUTED, false);
        h.arrow.setGravity(Gravity.CENTER);
        row.addView(h.arrow, new LinearLayout.LayoutParams(dp(22), dp(44)));

        h.check = textView(20, ACCENT, false);
        h.check.setGravity(Gravity.CENTER);
        h.check.setPadding(dp(2), 0, dp(2), 0);
        row.addView(h.check, new LinearLayout.LayoutParams(dp(36), dp(44)));

        h.icon = textView(16, MUTED, false);
        h.icon.setGravity(Gravity.CENTER);
        row.addView(h.icon, new LinearLayout.LayoutParams(dp(26), dp(44)));

        h.name = textView(14, TEXT, false);
        h.name.setSingleLine(true);
        h.name.setEllipsize(TextUtils.TruncateAt.END);
        LinearLayout.LayoutParams nameLp = new LinearLayout.LayoutParams(0, dp(44), 1f);
        row.addView(h.name, nameLp);

        h.size = textView(10.5f, MUTED, false);
        h.size.setGravity(Gravity.RIGHT | Gravity.CENTER_VERTICAL);
        row.addView(h.size, new LinearLayout.LayoutParams(dp(92), dp(44)));

        h.state = textView(11, MUTED, false);
        h.state.setGravity(Gravity.RIGHT | Gravity.CENTER_VERTICAL);
        h.state.setSingleLine(true);
        h.state.setEllipsize(TextUtils.TruncateAt.END);
        row.addView(h.state, new LinearLayout.LayoutParams(dp(86), dp(44)));

        return row;
    }

    private TextView textView(float size, int color, boolean bold) {
        TextView tv = new TextView(context);
        tv.setTextSize(size);
        tv.setTextColor(color);
        if (bold) tv.setTypeface(Typeface.DEFAULT_BOLD);
        return tv;
    }

    private String checkGlyph(MegaApi.Node node) {
        int state = checkState(node);
        if (state == 1) return "☑";
        if (state == 2) return "▣";
        return "☐";
    }

    private int checkColor(MegaApi.Node node) {
        return checkState(node) == 0 ? MUTED : ACCENT;
    }

    /** 0 未选、1 全选、2 部分选。 */
    private int checkState(MegaApi.Node node) {
        if (!node.dir) return node.checked ? 1 : 0;
        int total = countFiles(node);
        int selected = countSelected(node);
        if (selected == 0) return 0;
        if (selected == total) return 1;
        return 2;
    }

    public static int countFiles(MegaApi.Node node) {
        if (!node.dir) return 1;
        int n = 0;
        for (MegaApi.Node c : node.children) n += countFiles(c);
        return n;
    }

    public static int countSelected(MegaApi.Node node) {
        if (!node.dir) return node.checked ? 1 : 0;
        int n = 0;
        for (MegaApi.Node c : node.children) n += countSelected(c);
        return n;
    }

    public static int countDirs(MegaApi.Node node) {
        if (!node.dir) return 0;
        int n = 1;
        for (MegaApi.Node c : node.children) n += countDirs(c);
        return n;
    }

    public static long totalSize(MegaApi.Node node) {
        if (!node.dir) return node.size;
        long s = 0;
        for (MegaApi.Node c : node.children) s += totalSize(c);
        return s;
    }

    private String stateText(MegaApi.Node node) {
        switch (node.state) {
            case 1: return "直链…";
            case 2: {
                double pct = node.size <= 0 ? 0 : (node.downloaded * 100.0 / node.size);
                return String.format(java.util.Locale.US, "下载中 %.1f%%", pct);
            }
            case 3: return "完成";
            case 4: return "失败";
            case 5: return "已停止";
            case 6: return "已跳过";
            default: return "等待";
        }
    }

    private int stateColor(MegaApi.Node node) {
        switch (node.state) {
            case 2: return ACCENT;
            case 3: return SUCCESS;
            case 4: return DANGER;
            default: return MUTED;
        }
    }

    private String iconFor(MegaApi.Node node) {
        if (node.dir) return "📁";
        String lower = node.name.toLowerCase(java.util.Locale.US);
        if (lower.endsWith(".mp3") || lower.endsWith(".flac") || lower.endsWith(".ape") || lower.endsWith(".wav")) return "🎵";
        if (lower.endsWith(".mp4") || lower.endsWith(".mkv") || lower.endsWith(".avi")) return "🎬";
        if (lower.endsWith(".zip") || lower.endsWith(".rar") || lower.endsWith(".7z")) return "🗜";
        if (lower.endsWith(".jpg") || lower.endsWith(".png") || lower.endsWith(".webp")) return "🖼";
        if (lower.endsWith(".txt") || lower.endsWith(".log") || lower.endsWith(".cue")) return "📄";
        return "📄";
    }

    private int dp(float v) {
        return (int) (v * context.getResources().getDisplayMetrics().density + 0.5f);
    }

    static class Holder {
        View spacer;
        TextView arrow;
        TextView check;
        TextView icon;
        TextView name;
        TextView size;
        TextView state;
    }
}
