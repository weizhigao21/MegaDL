#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
MegaDownloader 逆向原型 - 桌面 UI（Tkinter）

流程：粘贴/输入 MEGA 链接 → 解析 → 勾选文件 → 下载选中 / 全部下载。
"""

import os
import queue
import threading
import time
import tkinter as tk
from tkinter import filedialog, messagebox, ttk

import mega


class MegaApp:
    def __init__(self, root):
        self.root = root
        self.root.title("MegaDownloader 逆向原型")
        self.root.geometry("1020x700")
        self.root.minsize(860, 600)

        self.api = mega.MegaAPI()
        self.queue = queue.Queue()
        self.items = []             # 当前链接解析出的文件
        self.folder_id = None       # 文件夹链接时用于下载上下文
        self.link_kind = None
        self.busy = False
        self.cancel = threading.Event()

        self.outdir = tk.StringVar(value=self._default_dir())
        self.link_var = tk.StringVar()
        self.conn_var = tk.IntVar(value=4)
        self.part_var = tk.IntVar(value=1)
        self.status_var = tk.StringVar(value="就绪：粘贴 MEGA 文件或文件夹链接后点击“解析”")
        self.speed_var = tk.StringVar(value="")
        self.summary_var = tk.StringVar(value="")

        self._setup_style()
        self._build_ui()
        self.root.after(100, self._poll_queue)

    # ------------------------------------------------------------ 界面
    def _setup_style(self):
        style = ttk.Style(self.root)
        try:
            style.theme_use("clam")
        except tk.TclError:
            pass
        style.configure("Treeview", rowheight=26, font=("Microsoft YaHei UI", 9))
        style.configure("Treeview.Heading", font=("Microsoft YaHei UI", 9, "bold"), background="#f0f2f5")
        style.map("Treeview", background=[("selected", "#dbeafe")])
        style.configure("Horizontal.TProgressbar", thickness=14, background="#3b82f6",
                        troughcolor="#eceff3", bordercolor="#dfe3e8")
        style.configure("Title.TLabel", font=("Microsoft YaHei UI", 13, "bold"))
        style.configure("Hint.TLabel", foreground="#666666")
        style.configure("Accent.TButton", font=("Microsoft YaHei UI", 9, "bold"))

    def _build_ui(self):
        pad = {"padx": 10, "pady": 4}
        outer = ttk.Frame(self.root, padding=12)
        outer.pack(fill="both", expand=True)

        header = ttk.Frame(outer)
        header.pack(fill="x")
        ttk.Label(header, text="MegaDownloader 逆向原型", style="Title.TLabel").pack(side="left")
        ttk.Label(
            header, text="MPRESS 解包 / MEGA API / AES-CTR · Python 版基础流程",
            style="Hint.TLabel",
        ).pack(side="left", padx=12, pady=(6, 0))

        link_row = ttk.Frame(outer)
        link_row.pack(fill="x", pady=(12, 4))
        ttk.Label(link_row, text="MEGA 链接").pack(side="left")
        self.link_entry = ttk.Entry(link_row, textvariable=self.link_var, font=("Consolas", 10))
        self.link_entry.pack(side="left", fill="x", expand=True, padx=8)
        self.link_entry.bind("<Return>", lambda _e: self.parse_link())
        self.btn_paste = ttk.Button(link_row, text="粘贴", width=6, command=self.paste_link)
        self.btn_paste.pack(side="left", padx=2)
        self.btn_parse = ttk.Button(link_row, text="解析", width=8, command=self.parse_link, style="Accent.TButton")
        self.btn_parse.pack(side="left", padx=(2, 0))

        dir_row = ttk.Frame(outer)
        dir_row.pack(fill="x", pady=4)
        ttk.Label(dir_row, text="保存目录").pack(side="left")
        self.dir_entry = ttk.Entry(dir_row, textvariable=self.outdir, state="readonly")
        self.dir_entry.pack(side="left", fill="x", expand=True, padx=8)
        ttk.Button(dir_row, text="选择…", width=8, command=self.choose_dir).pack(side="left")

        list_frame = ttk.Frame(outer)
        list_frame.pack(fill="both", expand=True, pady=(8, 4))
        columns = ("sel", "name", "size", "status")
        self.tree = ttk.Treeview(list_frame, columns=columns, show="headings", selectmode="browse")
        self.tree.heading("sel", text="选择")
        self.tree.heading("name", text="文件名")
        self.tree.heading("size", text="大小")
        self.tree.heading("status", text="状态")
        self.tree.column("sel", width=52, anchor="center", stretch=False)
        self.tree.column("name", width=560, anchor="w")
        self.tree.column("size", width=110, anchor="e", stretch=False)
        self.tree.column("status", width=150, anchor="w", stretch=False)
        vsb = ttk.Scrollbar(list_frame, orient="vertical", command=self.tree.yview)
        self.tree.configure(yscrollcommand=vsb.set)
        self.tree.pack(side="left", fill="both", expand=True)
        vsb.pack(side="right", fill="y")
        self.tree.bind("<Button-1>", self._on_tree_click)
        self.tree.bind("<Double-1>", self._on_tree_double_click)
        self.tree.tag_configure("done", foreground="#1b7f3a")
        self.tree.tag_configure("failed", foreground="#b3261e")
        self.tree.tag_configure("running", foreground="#125aa5")

        toolbar = ttk.Frame(outer)
        toolbar.pack(fill="x", pady=(4, 6))
        self.btn_all_sel = ttk.Button(toolbar, text="全选", width=7, command=lambda: self.set_all(True))
        self.btn_all_sel.pack(side="left")
        self.btn_none_sel = ttk.Button(toolbar, text="全不选", width=7, command=lambda: self.set_all(False))
        self.btn_none_sel.pack(side="left", padx=4)

        self.btn_dl_sel = ttk.Button(toolbar, text="下载选中", width=11, command=self.download_selected, style="Accent.TButton")
        self.btn_dl_sel.pack(side="left", padx=(14, 4))
        self.btn_dl_all = ttk.Button(toolbar, text="全部下载", width=11, command=self.download_all)
        self.btn_dl_all.pack(side="left", padx=4)
        self.btn_stop = ttk.Button(toolbar, text="停止", width=7, command=self.stop_download, state="disabled")
        self.btn_stop.pack(side="left", padx=4)

        ttk.Separator(toolbar, orient="vertical").pack(side="left", fill="y", padx=10)
        ttk.Label(toolbar, text="连接数").pack(side="left")
        ttk.Spinbox(toolbar, from_=1, to=16, width=4, textvariable=self.conn_var).pack(side="left", padx=(4, 10))
        ttk.Label(toolbar, text="分块 MB").pack(side="left")
        ttk.Spinbox(toolbar, from_=1, to=16, width=4, textvariable=self.part_var).pack(side="left", padx=4)

        right_btns = ttk.Frame(toolbar)
        right_btns.pack(side="right")
        ttk.Button(right_btns, text="复制链接", width=10, command=self.copy_selected_link).pack(side="left", padx=3)
        ttk.Button(right_btns, text="打开目录", width=10, command=self.open_outdir).pack(side="left")

        prog_frame = ttk.Frame(outer)
        prog_frame.pack(fill="x", pady=(4, 2))
        self.progress = ttk.Progressbar(prog_frame, orient="horizontal", mode="determinate", maximum=100)
        self.progress.pack(fill="x")
        info_row = ttk.Frame(outer)
        info_row.pack(fill="x", pady=(4, 2))
        ttk.Label(info_row, textvariable=self.status_var, style="Hint.TLabel").pack(side="left")
        ttk.Label(info_row, textvariable=self.summary_var, style="Hint.TLabel").pack(side="left", padx=12)
        ttk.Label(info_row, textvariable=self.speed_var, style="Hint.TLabel").pack(side="right")

        log_frame = ttk.Frame(outer)
        log_frame.pack(fill="both", expand=False, pady=(8, 0))
        ttk.Label(log_frame, text="日志").pack(anchor="w")
        self.log_text = tk.Text(log_frame, height=7, wrap="none", state="disabled",
                                font=("Consolas", 9), background="#fafafa", relief="solid", borderwidth=1)
        log_vsb = ttk.Scrollbar(log_frame, orient="vertical", command=self.log_text.yview)
        self.log_text.configure(yscrollcommand=log_vsb.set)
        self.log_text.pack(side="left", fill="both", expand=True)
        log_vsb.pack(side="right", fill="y")

        ttk.Label(
            outer, text="提示：单击“选择”列切换勾选，双击行可直接下载该文件。",
            style="Hint.TLabel",
        ).pack(anchor="w", pady=(6, 0))

        self._set_actions_enabled(False)

    # ------------------------------------------------------------ 小工具
    @staticmethod
    def _default_dir():
        home = os.path.expanduser("~")
        downloads = os.path.join(home, "Downloads")
        return downloads if os.path.isdir(downloads) else home

    def log(self, text):
        ts = time.strftime("%H:%M:%S")
        self.log_text.configure(state="normal")
        self.log_text.insert("end", f"[{ts}] {text}\n")
        self.log_text.see("end")
        self.log_text.configure(state="disabled")

    def _log_async(self, text):
        self.queue.put(("log", text))

    def _set_actions_enabled(self, has_items):
        state = "normal" if (has_items and not self.busy) else "disabled"
        for btn in (self.btn_dl_sel, self.btn_dl_all, self.btn_all_sel, self.btn_none_sel):
            btn.configure(state=state)

    def paste_link(self):
        try:
            self.link_var.set(self.root.clipboard_get().strip())
        except tk.TclError:
            self._log_async("剪贴板为空")

    def choose_dir(self):
        picked = filedialog.askdirectory(initialdir=self.outdir.get() or self._default_dir())
        if picked:
            self.outdir.set(picked)

    def open_outdir(self):
        path = self.outdir.get()
        if path and os.path.isdir(path):
            os.startfile(path)  # noqa: S606 - Windows 桌面打开目录
        else:
            messagebox.showwarning("提示", "保存目录不存在")

    def copy_selected_link(self):
        idx = self._first_selected_row()
        if idx is None:
            messagebox.showinfo("提示", "请先在列表中选择一行")
            return
        item = self.items[idx]
        link = f"https://mega.nz/file/{item['handle']}#{item['key_b64']}"
        self.root.clipboard_clear()
        self.root.clipboard_append(link)
        self.log("已复制链接：" + link)

    def _first_selected_row(self):
        sel = self.tree.selection()
        return int(sel[0]) if sel else None

    def set_all(self, value):
        for item in self.items:
            item["selected"] = value
        self._refresh_rows()

    def _refresh_rows(self):
        for i, item in enumerate(self.items):
            mark = "☑" if item["selected"] else "☐"
            self.tree.item(str(i), values=(mark, item["name"], mega.fmt_size(item["size"]), item.get("status", "等待")))
            tags = (item["tag"],) if item.get("tag") else ()
            self.tree.item(str(i), tags=tags)

    def _on_tree_click(self, event):
        region = self.tree.identify("region", event.x, event.y)
        if region != "cell":
            return
        row = self.tree.identify_row(event.y)
        col = self.tree.identify_column(event.x)
        if row and col == "#1":
            idx = int(row)
            self.items[idx]["selected"] = not self.items[idx]["selected"]
            self._refresh_rows()
            return "break"

    def _on_tree_double_click(self, event):
        row = self.tree.identify_row(event.y)
        if row and not self.busy:
            self.start_download([self.items[int(row)]])

    # ------------------------------------------------------------ 解析
    def parse_link(self):
        if self.busy:
            return
        link = self.link_var.get().strip()
        if not link:
            messagebox.showinfo("提示", "请先输入 MEGA 链接")
            return
        self.btn_parse.configure(state="disabled")
        self.status_var.set("正在解析链接…")
        threading.Thread(target=self._parse_worker, args=(link,), daemon=True).start()

    def _parse_worker(self, link):
        try:
            parsed = mega.parse_link(link)
            if parsed["kind"] == "folder":
                nodes = mega.enrich_folder(self.api, parsed)
                items = []
                for i, n in enumerate(nodes):
                    items.append({
                        "index": i,
                        "name": n["name"],
                        "size": n["size"],
                        "handle": n["handle"],
                        "key": n["key"],
                        "key_b64": n["key_b64"],
                        "selected": True,
                        "status": "等待",
                    })
                self.queue.put(("link", "folder", parsed["id"], items, link))
            else:
                key = mega.b64url_decode(parsed["key"])
                info = self.api.file_info(file_id=parsed["id"])
                attrs = mega.decrypt_attrs(info["at"], key)
                name = attrs.get("n") or parsed["id"]
                item = {
                    "index": 0,
                    "name": name,
                    "size": info["size"],
                    "handle": parsed["id"],
                    "key": key,
                    "key_b64": mega.b64url_encode(key),
                    "selected": True,
                    "status": "等待",
                    "info": info,
                }
                self.queue.put(("link", "file", None, [item], link))
        except Exception as exc:  # noqa: BLE001
            self.queue.put(("parse_error", str(exc)))

    def _apply_link(self, kind, folder_id, items, link):
        self.link_kind = kind
        self.folder_id = folder_id
        self.items = items
        self.tree.delete(*self.tree.get_children())
        for i, item in enumerate(items):
            self.tree.insert("", "end", iid=str(i), values=("☑", item["name"], mega.fmt_size(item["size"]), item["status"]))
        total = sum(i["size"] for i in items)
        self.summary_var.set(f"{len(items)} 个文件 · 共 {mega.fmt_size(total)}")
        self.status_var.set("解析完成，选择文件后开始下载")
        self.btn_parse.configure(state="normal")
        self._set_actions_enabled(bool(items))
        self.log(f"解析成功：{len(items)} 个文件，共 {mega.fmt_size(total)}")
        if kind == "folder":
            self.log(f"文件夹 ID：{folder_id}")

    # ------------------------------------------------------------ 下载
    def download_selected(self):
        if self.busy:
            return
        targets = [i for i in self.items if i["selected"]]
        if not targets:
            messagebox.showinfo("提示", "还没有勾选文件")
            return
        self.start_download(targets)

    def download_all(self):
        if self.busy:
            return
        if not self.items:
            return
        self.set_all(True)
        self.start_download(list(self.items))

    def start_download(self, targets):
        if self.busy or not targets:
            return
        outdir = self.outdir.get()
        try:
            os.makedirs(outdir, exist_ok=True)
        except OSError as exc:
            messagebox.showerror("错误", f"无法创建保存目录：{exc}")
            return
        self.cancel.clear()
        self.busy = True
        self.btn_parse.configure(state="disabled")
        self.btn_stop.configure(state="normal")
        self._set_actions_enabled(False)
        self.progress.configure(value=0)
        self.speed_var.set("")
        threading.Thread(
            target=self._download_worker,
            args=(targets, self.folder_id, outdir, max(1, self.conn_var.get()), max(1, self.part_var.get())),
            daemon=True,
        ).start()

    def _download_worker(self, targets, folder_id, outdir, connections, part_mb):
        total = len(targets)
        done_count = 0
        failed = 0
        for seq, item in enumerate(targets, 1):
            if self.cancel.is_set():
                break
            idx = item["index"]
            self.queue.put(("status", idx, "获取直链…"))
            try:
                if item.get("info") is not None:
                    info = item["info"]
                else:
                    info = self.api.file_info(node=item["handle"], folder=folder_id)
                dest = self._unique_dest(os.path.join(outdir, mega.safe_filename(item["name"])))
                self.queue.put(("overall", f"第 {seq}/{total} 个：{item['name']}"))
                self.queue.put(("status", idx, "下载中"))
                self.queue.put(("filebar", item["name"]))
                dl = mega.Downloader(self.api, connections=connections, part_size=part_mb * 1024 * 1024)
                dl.download(
                    info, item["key"], dest,
                    on_progress=lambda d, t, s, i=idx: self.queue.put(("progress", i, d, t, s)),
                    cancel_event=self.cancel,
                )
                item["status"] = "完成"
                item["tag"] = "done"
                done_count += 1
                self.queue.put(("status", idx, "完成"))
                self.queue.put(("log", f"完成：{item['name']} → {dest}"))
            except mega.DownloadCancelled:
                item["status"] = "已停止"
                item["tag"] = ""
                self.queue.put(("status", idx, "已停止"))
                self.queue.put(("log", f"已停止：{item['name']}"))
                break
            except Exception as exc:  # noqa: BLE001
                item["status"] = "失败"
                item["tag"] = "failed"
                failed += 1
                self.queue.put(("status", idx, "失败"))
                self.queue.put(("log", f"失败：{item['name']} - {exc}"))
        self.queue.put(("finish", done_count, failed, total))

    @staticmethod
    def _unique_dest(path):
        if not os.path.exists(path):
            return path
        root, ext = os.path.splitext(path)
        i = 1
        while os.path.exists(f"{root} ({i}){ext}"):
            i += 1
        return f"{root} ({i}){ext}"

    def stop_download(self):
        if not self.busy:
            return
        self.cancel.set()
        self.btn_stop.configure(state="disabled")
        self.status_var.set("正在停止…（等待当前分块结束）")
        self.log("已请求停止下载")

    # ------------------------------------------------------------ 进度队列
    def _poll_queue(self):
        try:
            while True:
                msg = self.queue.get_nowait()
                kind = msg[0]
                if kind == "log":
                    self.log(msg[1])
                elif kind == "link":
                    self._apply_link(msg[1], msg[2], msg[3], msg[4])
                elif kind == "parse_error":
                    self.status_var.set("解析失败")
                    self.log("解析失败：" + msg[1])
                    self.btn_parse.configure(state="normal")
                    messagebox.showerror("解析失败", msg[1])
                elif kind == "status":
                    self._set_row_status(msg[1], msg[2])
                elif kind == "filebar":
                    self.progress.configure(value=0)
                    self.status_var.set("正在下载：" + msg[1])
                elif kind == "progress":
                    _, idx, done, total, speed = msg
                    pct = (done * 100.0 / total) if total else 100
                    self.progress.configure(value=pct)
                    self.summary_var.set(f"{mega.fmt_size(done)} / {mega.fmt_size(total)}")
                    self.speed_var.set(f"{mega.fmt_size(speed)}/s")
                    self._set_row_status(idx, f"下载中 {pct:.1f}%")
                elif kind == "overall":
                    self.status_var.set(msg[1])
                elif kind == "finish":
                    done_count, failed, total = msg[1], msg[2], msg[3]
                    self.busy = False
                    self.btn_stop.configure(state="disabled")
                    self.btn_parse.configure(state="normal")
                    self._set_actions_enabled(bool(self.items))
                    self.speed_var.set("")
                    if self.cancel.is_set():
                        self.status_var.set(f"已停止（完成 {done_count}，失败 {failed}）")
                    else:
                        self.status_var.set(f"全部结束：成功 {done_count}，失败 {failed} / 共 {total}")
                        if done_count and not failed:
                            self.progress.configure(value=100)
                    self.log(self.status_var.get())
        except queue.Empty:
            pass
        self.root.after(100, self._poll_queue)

    def _set_row_status(self, idx, text):
        if idx < 0 or idx >= len(self.items):
            return
        item = self.items[idx]
        item["status"] = text
        mark = "☑" if item["selected"] else "☐"
        self.tree.item(str(idx), values=(mark, item["name"], mega.fmt_size(item["size"]), text))
        tags = (item["tag"],) if item.get("tag") else ()
        self.tree.item(str(idx), tags=tags)


def main():
    root = tk.Tk()
    app = MegaApp(root)
    app.link_entry.focus_set()
    root.mainloop()


if __name__ == "__main__":
    main()
