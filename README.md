# MegaDL

MEGA 公共链接下载器，针对 MEGA 匿名 API 与客户端加密做的一套开源实现。
仓库里有两份互相独立的成品：

| 目录 | 平台 | 技术栈 | 版本 |
| --- | --- | --- | --- |
| `megadl-rs` | Windows 桌面 | Rust + egui/eframe | v0.1.4 |
| `android` | Android | 纯 Java + 原生 Activity | v0.2.4 |

`mega_proto` 是最早的 Python 协议原型，用于和逆向结论逐字段对照；
`逆向分析笔记.md` 记录了对 MegaDownloader 1.8（MPRESS 壳）的静态分析结论。

## 能做什么

- 解析 `/file/`、`/folder/`、`#!`、`#F!` 与裸 `id!key` 链接
- 匿名 g / eu API，AES-CBC 解属性、AES-ECB 解节点密钥、AES-CTR 解文件内容
- 文件夹完整树、三态勾选、按深度缩进
- 8 连接 × 3 文件并发、4MB 分块 Range 下载、边收边解密、失败换线路重试
- 重名自动加序号，`.part` 临时文件，完成后重命名

## 快速开始

桌面版编译与运行见 `megadl-rs/README.md`，安卓版构建与安装见 `android/README.md`。


