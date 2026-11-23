# -*- coding: utf-8 -*-
"""原型自检：不联网，验证链接解析、属性解密、CTR 偏移解密。"""
import base64
import json
import os
import sys

from Crypto.Cipher import AES

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import mega


def make_key(seed=1):
    return bytes((seed + i) & 0xFF for i in range(32))


def test_parse_link():
    k = "A" * 43
    cases = [
        (f"https://mega.nz/file/AbCdEf12#{k}", "file", "AbCdEf12"),
        (f"https://mega.nz/folder/AbCdEf12#{k}", "folder", "AbCdEf12"),
        (f"https://mega.nz/#!AbCdEf12!{k}", "file", "AbCdEf12"),
        (f"https://mega.co.nz/#F!AbCdEf12!{k}", "folder", "AbCdEf12"),
    ]
    for link, kind, file_id in cases:
        parsed = mega.parse_link(link)
        assert parsed["kind"] == kind, parsed
        assert parsed["id"] == file_id, parsed
        assert parsed["key"] == k, parsed
    print("parse_link OK")


def test_attrs():
    key = make_key()
    attrs = {"n": "测试文件.bin"}
    plain = ("MEGA" + json.dumps(attrs, separators=(",", ":"))).encode("utf-8")
    plain += b"\x00" * ((-len(plain)) % 16)
    aes_key = mega.file_aes_key(key)
    cipher = AES.new(aes_key, AES.MODE_CBC, iv=b"\x00" * 16)
    at = mega.b64url_encode(cipher.encrypt(plain))
    got = mega.decrypt_attrs(at, key)
    assert got == attrs, got
    print("decrypt_attrs OK")


def test_ctr_offsets():
    key = make_key(7)
    data = bytes(range(256)) * 4
    full = mega.aes_ctr_decrypt(data, key, 0)
    for off in (16, 32, 48, 160):
        part = mega.aes_ctr_decrypt(data[off:off + 64], key, off)
        assert part == full[off:off + 64], off
    # 非 16 对齐偏移（续传场景）
    part = mega.aes_ctr_decrypt(data[9:41], key, 9)
    assert part == full[9:41]
    print("aes_ctr_decrypt offsets OK")


def test_node_key():
    folder_key = make_key(21)
    node_key = make_key(33)
    aes_key = mega.file_aes_key(folder_key)
    enc = AES.new(aes_key, AES.MODE_ECB).encrypt(node_key)
    k_field = "handle:" + mega.b64url_encode(enc)
    got = mega.decrypt_node_key(k_field, folder_key)
    assert got == node_key, got
    print("decrypt_node_key OK")


if __name__ == "__main__":
    test_parse_link()
    test_attrs()
    test_ctr_offsets()
    test_node_key()
    print("全部自检通过")
