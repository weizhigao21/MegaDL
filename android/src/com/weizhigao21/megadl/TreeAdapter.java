package com.weizhigao21.megadl;

import android.content.Context;
import android.graphics.Canvas;
import android.graphics.Color;
import android.graphics.Paint;
import android.graphics.Path;
import android.graphics.RectF;
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
        holder.icon.setKind(iconKind(node));
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

        h.icon = new TypeIcon(context);
        row.addView(h.icon, new LinearLayout.LayoutParams(dp(28), dp(44)));

        h.name = textView(14, TEXT, false);
        h.name.setSingleLine(true);
        h.name.setEllipsize(TextUtils.TruncateAt.END);
        h.name.setGravity(Gravity.CENTER_VERTICAL);
        LinearLayout.LayoutParams nameLp = new LinearLayout.LayoutParams(0, dp(44), 1f);
        nameLp.leftMargin = dp(4);
        row.addView(h.name, nameLp);

        h.size = textView(10.5f, MUTED, false);
        h.size.setGravity(Gravity.RIGHT | Gravity.CENTER_VERTICAL);
        row.addView(h.size, new LinearLayout.LayoutParams(dp(84), dp(44)));

        h.state = textView(11, MUTED, false);
        h.state.setGravity(Gravity.RIGHT | Gravity.CENTER_VERTICAL);
        h.state.setSingleLine(true);
        h.state.setEllipsize(TextUtils.TruncateAt.END);
        row.addView(h.state, new LinearLayout.LayoutParams(dp(76), dp(44)));

        return row;
    }

    private TextView textView(float size, int color, boolean bold) {
        TextView tv = new TextView(context);
        tv.setTextSize(size);
        tv.setTextColor(color);
        tv.setIncludeFontPadding(false);
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

    private String iconKind(MegaApi.Node node) {
        if (node.dir) return TypeIcon.FOLDER;
        String lower = node.name.toLowerCase(java.util.Locale.US);
        if (lower.endsWith(".mp3") || lower.endsWith(".flac") || lower.endsWith(".ape") || lower.endsWith(".wav")) return TypeIcon.AUDIO;
        if (lower.endsWith(".mp4") || lower.endsWith(".mkv") || lower.endsWith(".avi")) return TypeIcon.VIDEO;
        if (lower.endsWith(".zip") || lower.endsWith(".rar") || lower.endsWith(".7z")) return TypeIcon.ARCHIVE;
        if (lower.endsWith(".jpg") || lower.endsWith(".png") || lower.endsWith(".webp")) return TypeIcon.IMAGE;
        return TypeIcon.DOC;
    }

    private int dp(float v) {
        return (int) (v * context.getResources().getDisplayMetrics().density + 0.5f);
    }

    static class Holder {
        View spacer;
        TextView arrow;
        TextView check;
        TypeIcon icon;
        TextView name;
        TextView size;
        TextView state;
    }

    /** 自绘类型图标，避免依赖系统 emoji 字体（部分机型压缩包等字形缺失）。 */
    static class TypeIcon extends View {
        static final String FOLDER = "folder";
        static final String AUDIO = "audio";
        static final String VIDEO = "video";
        static final String ARCHIVE = "archive";
        static final String IMAGE = "image";
        static final String DOC = "doc";

        private static final int C_FOLDER = Color.rgb(0xE8, 0xA2, 0x2E);
        private static final int C_FOLDER_DARK = Color.rgb(0xD3, 0x8C, 0x1C);
        private static final int C_DOC = Color.rgb(0x93, 0xA1, 0xAB);
        private static final int C_AUDIO = Color.rgb(0x7C, 0x5C, 0xFF);
        private static final int C_VIDEO = Color.rgb(0xDF, 0x5F, 0x58);
        private static final int C_ARCHIVE = Color.rgb(0xA5, 0x77, 0x3D);
        private static final int C_IMAGE = Color.rgb(0x2A, 0xA3, 0x6B);

        private final Paint paint = new Paint(Paint.ANTI_ALIAS_FLAG);
        private final Path path = new Path();
        private final RectF rect = new RectF();
        private final float d;
        private String kind = DOC;

        TypeIcon(Context context) {
            super(context);
            d = context.getResources().getDisplayMetrics().density;
        }

        void setKind(String k) {
            if (!kind.equals(k)) {
                kind = k;
                invalidate();
            }
        }

        @Override
        protected void onDraw(Canvas canvas) {
            float s = Math.min(d * 18f, Math.min(getWidth(), getHeight()) - d * 10f);
            float left = (getWidth() - s) / 2f;
            float top = (getHeight() - s) / 2f;

            paint.setStyle(Paint.Style.FILL);
            paint.setStrokeWidth(d * 1.6f);

            if (FOLDER.equals(kind)) {
                paint.setColor(C_FOLDER_DARK);
                rect.set(left + s * 0.02f, top + s * 0.16f, left + s * 0.44f, top + s * 0.36f);
                canvas.drawRoundRect(rect, s * 0.06f, s * 0.06f, paint);
                paint.setColor(C_FOLDER);
                rect.set(left, top + s * 0.28f, left + s, top + s);
                canvas.drawRoundRect(rect, s * 0.1f, s * 0.1f, paint);
            } else if (AUDIO.equals(kind)) {
                paint.setColor(C_AUDIO);
                paint.setStyle(Paint.Style.FILL);
                paint.setTypeface(Typeface.DEFAULT);
                paint.setTextSize(s * 1.2f);
                paint.setTextAlign(Paint.Align.CENTER);
                Paint.FontMetrics fm = paint.getFontMetrics();
                canvas.drawText("♪", left + s / 2f, top + s / 2f - (fm.ascent + fm.descent) / 2f, paint);
                paint.setTextAlign(Paint.Align.LEFT);
            } else if (VIDEO.equals(kind)) {
                paint.setColor(C_VIDEO);
                paint.setStyle(Paint.Style.STROKE);
                rect.set(left + s * 0.04f, top + s * 0.1f, left + s * 0.96f, top + s * 0.9f);
                canvas.drawRoundRect(rect, s * 0.14f, s * 0.14f, paint);
                paint.setStyle(Paint.Style.FILL);
                path.reset();
                path.moveTo(left + s * 0.38f, top + s * 0.32f);
                path.lineTo(left + s * 0.38f, top + s * 0.68f);
                path.lineTo(left + s * 0.7f, top + s * 0.5f);
                path.close();
                canvas.drawPath(path, paint);
            } else if (ARCHIVE.equals(kind)) {
                paint.setColor(C_ARCHIVE);
                paint.setStyle(Paint.Style.STROKE);
                rect.set(left + s * 0.06f, top + s * 0.12f, left + s * 0.94f, top + s * 0.94f);
                canvas.drawRoundRect(rect, s * 0.1f, s * 0.1f, paint);
                canvas.drawLine(left + s * 0.18f, top + s * 0.34f, left + s * 0.82f, top + s * 0.34f, paint);
                canvas.drawLine(left + s * 0.5f, top + s * 0.34f, left + s * 0.5f, top + s * 0.82f, paint);
                paint.setStyle(Paint.Style.FILL);
                rect.set(left + s * 0.4f, top + s * 0.4f, left + s * 0.6f, top + s * 0.46f);
                canvas.drawRect(rect, paint);
                rect.set(left + s * 0.4f, top + s * 0.54f, left + s * 0.6f, top + s * 0.6f);
                canvas.drawRect(rect, paint);
                rect.set(left + s * 0.4f, top + s * 0.68f, left + s * 0.6f, top + s * 0.74f);
                canvas.drawRect(rect, paint);
            } else if (IMAGE.equals(kind)) {
                paint.setColor(C_IMAGE);
                paint.setStyle(Paint.Style.STROKE);
                rect.set(left + s * 0.04f, top + s * 0.12f, left + s * 0.96f, top + s * 0.9f);
                canvas.drawRoundRect(rect, s * 0.1f, s * 0.1f, paint);
                paint.setStyle(Paint.Style.FILL);
                path.reset();
                path.moveTo(left + s * 0.14f, top + s * 0.78f);
                path.lineTo(left + s * 0.42f, top + s * 0.42f);
                path.lineTo(left + s * 0.62f, top + s * 0.66f);
                path.lineTo(left + s * 0.74f, top + s * 0.56f);
                path.lineTo(left + s * 0.88f, top + s * 0.78f);
                path.close();
                canvas.drawPath(path, paint);
                canvas.drawCircle(left + s * 0.3f, top + s * 0.3f, s * 0.08f, paint);
            } else {
                paint.setColor(C_DOC);
                paint.setStyle(Paint.Style.STROKE);
                rect.set(left + s * 0.14f, top + s * 0.06f, left + s * 0.86f, top + s * 0.94f);
                canvas.drawRoundRect(rect, s * 0.08f, s * 0.08f, paint);
                paint.setStyle(Paint.Style.FILL);
                paint.setColor(Color.rgb(0xC9, 0xD3, 0xD9));
                canvas.drawRect(left + s * 0.3f, top + s * 0.36f, left + s * 0.7f, top + s * 0.42f, paint);
                canvas.drawRect(left + s * 0.3f, top + s * 0.52f, left + s * 0.7f, top + s * 0.58f, paint);
                canvas.drawRect(left + s * 0.3f, top + s * 0.68f, left + s * 0.58f, top + s * 0.74f, paint);
            }
        }
    }
}

