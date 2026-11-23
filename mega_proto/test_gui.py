# -*- coding: utf-8 -*-
"""GUI 冒烟测试：真实 Tk 窗口 + 本地假 MEGA 服务，跑一次“下载选中”流程。"""
import os
import shutil
import sys
import tempfile
import threading
import time
import tkinter as tk
from http.server import ThreadingHTTPServer

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import mega
import mega_gui
from test_local import DATA, KEY, Handler


def main():
    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    url = f"http://127.0.0.1:{server.server_address[1]}/file"

    outdir = tempfile.mkdtemp(prefix="mega_gui_test_")
    root = tk.Tk()
    root.withdraw()
    app = mega_gui.MegaApp(root)
    app.outdir.set(outdir)

    item = {
        "index": 0,
        "name": "sample.bin",
        "size": len(DATA),
        "handle": "FAKEHANDLE",
        "key": KEY,
        "key_b64": "x" * 43,
        "selected": True,
        "status": "等待",
        "info": {"size": len(DATA), "url": url},
    }
    app.items = [item]
    app.tree.insert("", "end", iid="0", values=("☑", item["name"], mega.fmt_size(item["size"]), "等待"))
    app._set_actions_enabled(True)

    app.start_download([item])
    deadline = time.time() + 60
    while app.busy and time.time() < deadline:
        root.update()
        time.sleep(0.05)
    root.update()

    dest = os.path.join(outdir, "sample.bin")
    assert os.path.exists(dest), "文件未生成"
    with open(dest, "rb") as f:
        got = f.read()
    assert got == DATA, f"内容不一致：{len(got)} vs {len(DATA)}"
    print("GUI 下载流程通过：", len(got), "字节，状态 =", item["status"])
    print("状态栏：", app.status_var.get())

    root.destroy()
    server.shutdown()
    shutil.rmtree(outdir, ignore_errors=True)


if __name__ == "__main__":
    main()
