#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
MegaDownloader 逆向原型（Python）

仅实现基础功能：
  - 解析 MEGA 文件链接 / 文件夹链接
  - 调用 MEGA 公共 API 获取文件信息（与 MegaDownloader 1.8 相同的请求格式）
  - AES-128-CBC 解密文件属性（文件名）
  - 多连接分块 Range 下载，AES-128-CTR 解密文件内容
  - 文件夹链接：列出节点、批量下载

依赖：requests、pycryptodome
"""

import argparse
import base64
import json
import math
import os
import re
import sys
import threading
import time
import urllib.parse

import requests
from Crypto.Cipher import AES

API_G = "https://g.api.mega.co.nz/cs"
API_EU = "https://eu.api.mega.co.nz/cs"

DEFAULT_PART_SIZE = 1 << 20      # 1 MiB，必须是 16 的倍数
DEFAULT_CONNECTIONS = 4

ERRORS = {
    -1: "EINTERNAL: 内部错误",
    -2: "EARGS: 参数错误",
    -3: "EAGAIN: 重试",
    -4: "ERATELIMIT: 请求频率超限",
    -5: "EFAILED: 请求失败",
    -6: "ETOOMANY: 连接数过多",
    -7: "ERANGE: Range 越界",
    -8: "EEXPIRED: 下载链接过期",
    -9: "ENOENT: 节点不存在",
    -10: "ECIRCULAR: 循环链接",
    -11: "EACCESS: 无访问权限",
    -12: "EEXIST: 已存在",
    -13: "EINCOMPLETE: 上传未完成",
    -14: "EKEY: 密钥错误",
    -15: "ESID: 会话过期",
    -16: "EBLOCKED: 已被封禁",
    -17: "EOVERQUOTA: 配额超限",
    -18: "ETEMPUNAVAIL: 暂时不可用",
    -19: "ETOOMANYCONNECTIONS: 连接过多",
    -20: "EWRITE: 写入失败",
    -21: "EREAD: 读取失败",
    -22: "EAPPAUTH: 应用密钥错误",
    -23: "ETEMPDOWN: 临时下载不可用",
    -24: "EGOINGOVERQUOTA: 超出配额",
}


class MegaError(Exception):
    def __init__(self, code, where=""):
        self.code = code
        msg = ERRORS.get(code, f"未知错误 {code}")
        super().__init__(f"{msg}{('（' + where + '）') if where else ''}")


class DownloadCancelled(Exception):
    """用户主动停止下载。"""


# ---------------------------------------------------------------- 基础工具

def b64url_decode(s):
    """MEGA 的 base64url 解码（兼容 -/_、无填充、含逗号）。"""
    s = s.replace("-", "+").replace("_", "/").replace(",", "").strip()
    s += "=" * ((-len(s)) % 4)
    return base64.b64decode(s)


def b64url_encode(b):
    return base64.b64encode(b).decode("ascii").replace("+", "-").replace("/", "_").rstrip("=")


def xor_halves(key):
    """MEGA 文件密钥：前 16 字节与后 16 字节异或，得到 AES-128 密钥。"""
    if len(key) == 16:
        return key
    return bytes(a ^ b for a, b in zip(key[:16], key[16:32]))


def file_aes_key(key):
    return xor_halves(key)


def decrypt_attrs(at_b64, key):
    """解密 MEGA 文件属性：AES-CBC，IV 全 0，密文尾部补 0 到 16 字节整数倍。"""
    aes_key = file_aes_key(key)
    data = b64url_decode(at_b64)
    pad = (-len(data)) % 16
    plain = AES.new(aes_key, AES.MODE_CBC, iv=b"\x00" * 16).decrypt(data + b"\x00" * pad)
    text = plain.decode("utf-8", "ignore").rstrip("\x00").lstrip("\x00")
    if not text.startswith("MEGA"):
        raise MegaError(-14, "文件属性解密失败")
    body = text[4:]
    try:
        return json.loads(body)
    except json.JSONDecodeError:
        m = re.search(r'"n"\s*:\s*"(.*?)"', body)
        return {"n": m.group(1)} if m else {}


def decrypt_node_key(node_key_b64, folder_key):
    """用文件夹密钥 AES-128-ECB 解密节点密钥（文件夹链接里的 k 字段，形如 "xxx:key"）。"""
    part = node_key_b64.split(":")[1] if ":" in node_key_b64 else node_key_b64
    blob = b64url_decode(part)
    aes_key = xor_halves(folder_key)
    ecb = AES.new(aes_key, AES.MODE_ECB)
    return ecb.decrypt(blob)


def aes_ctr_decrypt(data, key, offset):
    """按文件绝对偏移解密一段数据（AES-CTR：nonce|0 + 块计数，大端进位）。"""
    aes_key = file_aes_key(key)
    nonce = key[16:24] + b"\x00" * 8
    block = offset // 16
    skip = offset % 16
    initial = int.from_bytes(nonce, "big") + block
    length = skip + len(data)
    keystream = AES.new(aes_key, AES.MODE_CTR, nonce=b"", initial_value=initial).encrypt(b"\x00" * length)
    keystream = keystream[skip:]
    return bytes(a ^ b for a, b in zip(data, keystream))


# ---------------------------------------------------------------- 链接解析

LINK_FILE_NEW = re.compile(r"/file/([A-Za-z0-9_-]{6,})#([A-Za-z0-9_\-+,]{20,})")
LINK_FOLDER_NEW = re.compile(r"/folder/([A-Za-z0-9_-]{6,})#([A-Za-z0-9_\-+,]{20,})")
LINK_OLD = re.compile(r"#(!|F!)([A-Za-z0-9_-]{6,})!([A-Za-z0-9_\-+,]{20,})")
LINK_BARE = re.compile(r"(?:^|!)([A-Za-z0-9_-]{6,})(?:!)([A-Za-z0-9_\-+,]{16,})")


def parse_link(link):
    link = (link or "").strip()
    m = LINK_FOLDER_NEW.search(link)
    if m:
        return {"kind": "folder", "id": m.group(1), "key": m.group(2), "node": None}
    m = LINK_FILE_NEW.search(link)
    if m:
        return {"kind": "file", "id": m.group(1), "key": m.group(2), "node": None}
    m = LINK_OLD.search(link)
    if m:
        kind = "folder" if m.group(1).startswith("F") else "file"
        return {"kind": kind, "id": m.group(2), "key": m.group(3), "node": None}
    m = LINK_BARE.search(link)
    if m:
        return {"kind": "file", "id": m.group(1), "key": m.group(2), "node": None}
    raise ValueError("无法识别的 MEGA 链接：" + link)


# ---------------------------------------------------------------- API

class MegaAPI:
    def __init__(self, eu=False, timeout=40):
        # MegaDownloader 默认走 g.api（useGlobalCDN），部分网络下 g.api 不可达，自动回退 eu.api
        self.endpoints = [API_EU, API_G] if eu else [API_G, API_EU]
        self.timeout = timeout
        self._seq = int(time.time() * 1000)
        self._lock = threading.Lock()
        self.session = requests.Session()
        self.session.headers.update({"User-Agent": "MegaDownloader/1.8 (Python prototype)"})

    def _next_id(self):
        with self._lock:
            self._seq += 1
            return self._seq

    def request(self, payload, node=None, retries=3, timeout=None):
        params = {"id": self._next_id()}
        if node:
            params["n"] = node
        last = None
        for attempt in range(retries):
            base = self.endpoints[0]
            try:
                r = self.session.post(
                    base, params=params, data=json.dumps(payload),
                    headers={"Content-Type": "application/json"},
                    timeout=timeout or self.timeout,
                )
                if r.status_code == 509:
                    raise MegaError(-17, "带宽/配额限制")
                r.raise_for_status()
                body = r.text.strip()
                try:
                    data = json.loads(body)
                except json.JSONDecodeError:
                    raise MegaError(-5, "响应不是 JSON: " + body[:200])
                if isinstance(data, int):
                    raise MegaError(data)
                if isinstance(data, list) and data and isinstance(data[0], int):
                    raise MegaError(data[0])
                return data
            except MegaError:
                raise
            except (requests.ConnectionError, requests.Timeout) as exc:
                last = exc
                if len(self.endpoints) > 1:
                    self.endpoints.append(self.endpoints.pop(0))
                    continue
                time.sleep(1 + attempt * 2)
            except Exception as exc:  # noqa: BLE001
                last = exc
                time.sleep(1 + attempt * 2)
        raise MegaError(-5, f"网络请求失败: {last}")

    def file_info(self, file_id=None, node=None, folder=None):
        """对应 MegaDownloader::ObtenerInformacionFichero 的 g 请求。

        文件夹分享内的文件：body 用 n=<文件句柄>，URL 的 &n= 用 <文件夹 ID>。
        """
        item = {"a": "g", "g": 1, "ssl": 0}
        if node:
            item["n"] = node
        else:
            item["p"] = file_id
        data = self.request([item], node=folder or node)
        info = data[0] if isinstance(data, list) else data
        if not isinstance(info, dict):
            raise MegaError(-5, "g 请求返回异常")
        if info.get("e"):
            raise MegaError(int(info["e"]), "获取文件信息")
        for field in ("s", "at", "g"):
            if field not in info:
                raise MegaError(-5, f"响应缺少字段 {field}")
        return {"size": int(info["s"]), "at": info["at"], "url": info["g"]}

    def folder_nodes(self, folder_id):
        """对应 MegaFolderHelper::RetrieveLinksFromFolder 的 f 请求。"""
        data = self.request([{"a": "f", "c": 1, "r": 1}], node=folder_id)
        resp = data[0] if isinstance(data, list) else data
        if isinstance(resp, dict) and resp.get("e"):
            raise MegaError(int(resp["e"]), "获取文件夹列表")
        return (resp or {}).get("f", [])


# ---------------------------------------------------------------- 下载

class Progress:
    def __init__(self, total, callback=None):
        self.total = total
        self.done = 0
        self.start = time.time()
        self.callback = callback
        self._lock = threading.Lock()
        self._tty = sys.stderr.isatty()

    def add(self, n):
        with self._lock:
            self.done += n

    def render(self, final=False):
        now = time.time()
        elapsed = max(now - self.start, 1e-6)
        speed = self.done / elapsed
        if self.callback is not None:
            try:
                self.callback(self.done, self.total, int(speed))
            except Exception:
                pass
            return
        pct = (self.done * 100.0 / self.total) if self.total else 100.0
        line = f"\r{pct:6.2f}%  {fmt_size(self.done)}/{fmt_size(self.total)}  {fmt_size(int(speed))}/s"
        if final:
            sys.stderr.write(line + "\n")
            sys.stderr.flush()
        elif self._tty:
            sys.stderr.write(line)
            sys.stderr.flush()


def fmt_size(n):
    n = float(n)
    for unit in ("B", "KB", "MB", "GB", "TB"):
        if n < 1024 or unit == "TB":
            return f"{n:.1f}{unit}" if unit != "B" else f"{int(n)}B"
        n /= 1024


def safe_filename(name):
    name = re.sub(r'[<>:"/\\|?*\x00-\x1f]', "_", name).strip(" .")
    return name or "unnamed"


class Downloader:
    def __init__(self, api, connections=4, part_size=DEFAULT_PART_SIZE):
        self.api = api
        self.connections = max(1, connections)
        self.part_size = max(16 * 1024, (part_size // 16) * 16)

    def download(self, info, key, dest_path, on_progress=None, cancel_event=None):
        size = info["size"]
        url = info["url"]
        os.makedirs(os.path.dirname(os.path.abspath(dest_path)), exist_ok=True)
        part_path = dest_path + ".part"

        if size == 0:
            open(part_path, "wb").close()
            self._finalize(part_path, dest_path)
            return dest_path

        total_parts = math.ceil(size / self.part_size)
        progress = Progress(size, callback=on_progress)
        lock = threading.Lock()
        state = {"next": 0, "error": None}

        with open(part_path, "wb") as f:
            f.truncate(size)

        def worker():
            while True:
                with lock:
                    if state["error"] is not None:
                        return
                    if cancel_event is not None and cancel_event.is_set():
                        state["error"] = DownloadCancelled("已取消")
                        return
                    idx = state["next"]
                    if idx >= total_parts:
                        return
                    state["next"] += 1
                start = idx * self.part_size
                end = min(start + self.part_size, size) - 1
                try:
                    self._fetch_part(url, key, start, end, part_path, cancel_event)
                    progress.add(end - start + 1)
                except Exception as exc:  # noqa: BLE001
                    with lock:
                        state["error"] = exc
                    return

        threads = [threading.Thread(target=worker, daemon=True) for _ in range(min(self.connections, total_parts))]
        for t in threads:
            t.start()
        while any(t.is_alive() for t in threads):
            progress.render()
            time.sleep(0.5)
        for t in threads:
            t.join()

        if state["error"] is not None:
            raise state["error"]

        progress.render(final=True)
        self._finalize(part_path, dest_path)
        return dest_path

    def _fetch_part(self, url, key, start, end, part_path, cancel_event=None):
        candidates = [url]
        if "g.api.mega.co.nz" in url:
            candidates.append(url.replace("g.api.mega.co.nz", "eu.api.mega.co.nz"))
        elif "eu.api.mega.co.nz" in url:
            candidates.append(url.replace("eu.api.mega.co.nz", "g.api.mega.co.nz"))
        header = {"Range": f"bytes={start}-{end}"}
        last = None
        for attempt in range(4):
            if cancel_event is not None and cancel_event.is_set():
                raise DownloadCancelled("已取消")
            target = candidates[attempt % len(candidates)]
            try:
                with requests.get(target, headers=header, stream=True, timeout=(15, 120)) as r:
                    if r.status_code == 509:
                        raise MegaError(-17, "带宽/配额限制")
                    if r.status_code == 416:
                        raise MegaError(-7, "Range 不被接受")
                    if r.status_code not in (200, 206):
                        raise MegaError(-5, f"下载 HTTP {r.status_code}")
                    body = r.content
                if r.status_code == 200 and start != 0:
                    raise MegaError(-7, "服务器忽略了 Range")
                want = end - start + 1
                if len(body) != want:
                    raise MegaError(-5, f"分块长度不符：需要 {want}，收到 {len(body)}")
                if cancel_event is not None and cancel_event.is_set():
                    raise DownloadCancelled("已取消")
                plain = aes_ctr_decrypt(body, key, start)
                with open(part_path, "r+b") as f:
                    f.seek(start)
                    f.write(plain)
                return
            except DownloadCancelled:
                raise
            except MegaError:
                raise
            except Exception as exc:  # noqa: BLE001
                last = exc
                time.sleep(1 + attempt * 2)
        raise MegaError(-5, f"分块下载失败: {last}")

    @staticmethod
    def _finalize(part_path, dest_path):
        if os.path.exists(dest_path):
            root, ext = os.path.splitext(dest_path)
            i = 1
            while os.path.exists(f"{root} ({i}){ext}"):
                i += 1
            dest_path = f"{root} ({i}){ext}"
        os.replace(part_path, dest_path)
        return dest_path


# ---------------------------------------------------------------- 命令实现

def cmd_info(api, args):
    link = parse_link(args.link)
    if link["kind"] == "folder":
        nodes = enrich_folder(api, link)
        print(f"文件夹 {link['id']}，共 {len(nodes)} 个文件")
        for n in nodes:
            print(f"  {fmt_size(n['size']):>10}  {n['name']}")
        return
    key = b64url_decode(link["key"])
    info = api.file_info(file_id=link["id"], node=link["node"])
    attrs = decrypt_attrs(info["at"], key)
    print(f"文件名 : {attrs.get('n')}")
    print(f"大小   : {fmt_size(info['size'])} ({info['size']} 字节)")
    print(f"文件ID : {link['id']}")
    print(f"下载URL: {info['url']}")


def enrich_folder(api, link):
    folder_key = b64url_decode(link["key"])
    raw = api.folder_nodes(link["id"])
    nodes = []
    for n in raw:
        if n.get("t") != 0:
            continue  # t=0 文件，t=1 文件夹（MEGA 公共文件夹 API）
        try:
            node_key = decrypt_node_key(n["k"], folder_key)
            attrs = decrypt_attrs(n["a"], node_key)
        except Exception:
            continue
        name = attrs.get("n") or n.get("h", "unnamed")
        nodes.append({
            "name": name,
            "size": int(n.get("s", 0)),
            "handle": n.get("h"),
            "key": node_key,
            "key_b64": b64url_encode(node_key),
            "parent": n.get("p"),
        })
    return nodes


def cmd_list(api, args):
    link = parse_link(args.link)
    if link["kind"] != "folder":
        raise SystemExit("该链接不是文件夹链接")
    nodes = enrich_folder(api, link)
    for i, n in enumerate(nodes, 1):
        print(f"{i:>4}. {fmt_size(n['size']):>10}  {n['name']}")
        print(f"      https://mega.nz/file/{n['handle']}#{n['key_b64']}")


def cmd_download(api, args):
    link = parse_link(args.link)
    outdir = os.path.abspath(args.out or ".")
    os.makedirs(outdir, exist_ok=True)
    dl = Downloader(api, connections=args.connections, part_size=args.part_size * 1024 * 1024)

    if link["kind"] == "folder":
        nodes = enrich_folder(api, link)
        if not nodes:
            raise SystemExit("文件夹内没有可下载的文件")
        for i, n in enumerate(nodes, 1):
            dest = os.path.join(outdir, safe_filename(n["name"]))
            print(f"[{i}/{len(nodes)}] {n['name']}")
            info = api.file_info(node=n["handle"], folder=link["id"])
            dl.download(info, n["key"], dest)
        return

    key = b64url_decode(link["key"])
    info = api.file_info(file_id=link["id"], node=link["node"])
    attrs = decrypt_attrs(info["at"], key)
    name = safe_filename(attrs.get("n") or link["id"])
    dest = args.out_file if args.out_file else os.path.join(outdir, name)
    print(f"下载 {attrs.get('n')} ({fmt_size(info['size'])}) -> {dest}")
    dl.download(info, key, dest)
    print("完成：" + dest)


def main(argv=None):
    parser = argparse.ArgumentParser(description="MegaDownloader 逆向原型（Python）")
    parser.add_argument("--eu", action="store_true", help="使用 eu.api.mega.co.nz")
    sub = parser.add_subparsers(dest="cmd", required=True)

    p_info = sub.add_parser("info", help="查看链接信息")
    p_info.add_argument("link")
    p_info.set_defaults(func=cmd_info)

    p_list = sub.add_parser("list", help="列出文件夹内容")
    p_list.add_argument("link")
    p_list.set_defaults(func=cmd_list)

    p_dl = sub.add_parser("download", help="下载文件或整个文件夹")
    p_dl.add_argument("link")
    p_dl.add_argument("-o", "--out", help="输出目录")
    p_dl.add_argument("-f", "--out-file", help="指定输出文件路径")
    p_dl.add_argument("-j", "--connections", type=int, default=DEFAULT_CONNECTIONS)
    p_dl.add_argument("-s", "--part-size", type=int, default=1, help="分块大小（MB，默认 1）")
    p_dl.set_defaults(func=cmd_download)

    args = parser.parse_args(argv)
    api = MegaAPI(eu=args.eu)
    try:
        args.func(api, args)
    except (MegaError, ValueError) as exc:
        print("错误：" + str(exc), file=sys.stderr)
        return 2
    except KeyboardInterrupt:
        print("\n已中断", file=sys.stderr)
        return 130
    return 0


if __name__ == "__main__":
    sys.exit(main())
