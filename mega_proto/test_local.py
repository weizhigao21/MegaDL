# -*- coding: utf-8 -*-
"""本地端到端测试：用假 MEGA 分块服务验证 Range 下载 + AES-CTR 解密 + 多线程写入。"""
import os
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import mega

DATA = bytes((i * 7) & 0xFF for i in range(3 * 1024 * 1024 + 1234))
KEY = bytes((i * 3 + 5) & 0xFF for i in range(32))


def encrypt(data, key, offset=0):
    return mega.aes_ctr_decrypt(data, key, offset)  # CTR 对称


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_GET(self):
        rng = self.headers.get("Range")
        total = len(DATA)
        if rng and rng.startswith("bytes="):
            a, b = rng[6:].split("-")
            start, end = int(a), int(b)
            end = min(end, total - 1)
            chunk = DATA[start:end + 1]
            self.send_response(206)
            self.send_header("Content-Range", f"bytes {start}-{end}/{total}")
        else:
            start, chunk = 0, DATA
            self.send_response(200)
        self.send_header("Content-Length", str(len(chunk)))
        self.end_headers()
        self.wfile.write(encrypt(chunk, KEY, start))


def main():
    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    url = f"http://127.0.0.1:{server.server_address[1]}/file"
    out = os.path.join(os.path.dirname(os.path.abspath(__file__)), "_test_out.bin")
    try:
        dl = mega.Downloader(mega.MegaAPI(), connections=4, part_size=1 << 20)
        info = {"size": len(DATA), "url": url}
        dl.download(info, KEY, out)
        got = open(out, "rb").read()
        assert got == DATA, "内容不一致"
        print("本地端到端测试通过：", len(got), "字节")
    finally:
        server.shutdown()
        if os.path.exists(out):
            os.remove(out)


if __name__ == "__main__":
    main()
