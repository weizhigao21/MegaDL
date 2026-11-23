//! MegaDL：GUI 默认启动；--list / --get 为无界面模式（脚本与验证用）。
#![cfg_attr(windows, windows_subsystem = "windows")]

mod app;

use eframe::egui;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use megadl::download::{Collision, Downloader};
use megadl::mega::{self, LinkKind, MegaApi};

fn usage() -> ! {
    eprintln!(
        "MegaDL - MEGA 公共链接下载器\n\n\
         用法：\n  \
         megadl                          启动图形界面\n  \
         megadl --list <链接>            列出文件夹内容\n  \
         megadl --get <链接> [--out 目录] [-j 连接数] [--eu] [--proxy 地址]\n\n\
         示例：megadl --get \"https://mega.nz/file/...\" --out D:\\Downloads -j 8\n\
         代理示例：megadl --get \"...\" --proxy http://127.0.0.1:7890\n\n\
         支持 /file/、/folder/、#!、#F! 及裸 id!key 格式"
    );
    std::process::exit(2);
}

fn headless(args: &[String]) -> i32 {
    let mut mode = "";
    let mut link = String::new();
    let mut out = String::from(".");
    let mut connections = 4usize;
    let mut eu = false;
    let mut proxy = String::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--list" | "--get" => {
                mode = if args[i] == "--list" { "list" } else { "get" };
                i += 1;
                if i < args.len() {
                    link = args[i].clone();
                }
            }
            "--out" => {
                i += 1;
                if i < args.len() {
                    out = args[i].clone();
                }
            }
            "-j" => {
                i += 1;
                if i < args.len() {
                    connections = args[i].parse().unwrap_or(4);
                }
            }
            "--eu" => eu = true,
            "--proxy" => {
                i += 1;
                if i < args.len() {
                    proxy = args[i].clone();
                }
            }
            other => {
                eprintln!("未知参数：{other}");
                usage();
            }
        }
        i += 1;
    }
    if link.is_empty() {
        usage();
    }
    let api = MegaApi::new(eu, &proxy);
    let parsed = match mega::parse_link(&link) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("错误：{e}");
            return 2;
        }
    };
    let result = (|| -> mega::Result<()> {
        match (mode, parsed.kind.clone()) {
            ("list", LinkKind::Folder) => {
                let nodes: Vec<_> = api.enrich_folder(&parsed)?.into_iter().filter(|n| !n.is_dir).collect();
                println!("文件夹 {}，共 {} 个文件", parsed.id, nodes.len());
                for (i, n) in nodes.iter().enumerate() {
                    println!("{:>4}. {:>10}  {}", i + 1, mega::fmt_size(n.size), n.name);
                    println!("      https://mega.nz/file/{}#{}", n.handle, n.key_b64);
                }
                Ok(())
            }
            ("list", _) => Err(mega::Error::Local("该链接不是文件夹链接".into())),
            ("get", LinkKind::Folder) => {
                let nodes: Vec<_> = api.enrich_folder(&parsed)?.into_iter().filter(|n| !n.is_dir).collect();
                if nodes.is_empty() {
                    return Err(mega::Error::Local("文件夹内没有可下载的文件".into()));
                }
                let dl = Downloader::new(connections, 4, eu, &proxy);
                std::fs::create_dir_all(&out).map_err(|e| mega::Error::Local(e.to_string()))?;
                for (i, n) in nodes.iter().enumerate() {
                    println!("[{}/{}] {}", i + 1, nodes.len(), n.name);
                    let info = api.file_info(None, Some(&n.handle), Some(&parsed.id))?;
                    let dest = PathBuf::from(&out).join(mega::safe_filename(&n.name));
                    let done = Arc::new(AtomicU64::new(0));
                    let d2 = done.clone();
                    let total = info.size;
                    let cb = move |n: u64| {
                        let v = d2.fetch_add(n, Ordering::Relaxed) + n;
                        eprint!("\r  {}/{}", mega::fmt_size(v), mega::fmt_size(total));
                    };
                    let cancel = AtomicBool::new(false);
                    dl.download(&info, &n.key, &dest, Collision::Rename, Some(&cb), Some(&cancel))?;
                    eprintln!();
                }
                Ok(())
            }
            ("get", LinkKind::File) => {
                let key = mega::link_key_bytes(&parsed)?;
                let info = api.file_info(Some(&parsed.id), None, None)?;
                let attrs = mega::decrypt_attrs(&info.at, &key)?;
                let name = attrs.get("n").and_then(|v| v.as_str()).unwrap_or(&parsed.id);
                std::fs::create_dir_all(&out).map_err(|e| mega::Error::Local(e.to_string()))?;
                let dest = PathBuf::from(&out).join(mega::safe_filename(name));
                println!("下载 {name}（{}）→ {}", mega::fmt_size(info.size), dest.display());
                let dl = Downloader::new(connections, 4, eu, &proxy);
                let done = Arc::new(AtomicU64::new(0));
                let d2 = done.clone();
                let total = info.size;
                let cb = move |n: u64| {
                    let v = d2.fetch_add(n, Ordering::Relaxed) + n;
                    eprint!("\r  {}/{}", mega::fmt_size(v), mega::fmt_size(total));
                };
                let cancel = AtomicBool::new(false);
                let path = dl.download(&info, &key, &dest, Collision::Rename, Some(&cb), Some(&cancel))?;
                eprintln!();
                println!("完成：{}", path.display());
                Ok(())
            }
            _ => usage(),
        }
    })();
    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("错误：{e}");
            2
        }
    }
}

#[cfg(windows)]
fn attach_parent_console() {
    #[link(name = "kernel32")]
    extern "system" {
        fn AttachConsole(dw_process_id: u32) -> i32;
        fn CreateFileW(
            name: *const u16,
            access: u32,
            share: u32,
            security: *mut core::ffi::c_void,
            disposition: u32,
            flags: u32,
            template: *mut core::ffi::c_void,
        ) -> *mut core::ffi::c_void;
        fn SetStdHandle(which: u32, handle: *mut core::ffi::c_void) -> i32;
    }
    const ATTACH_PARENT_PROCESS: u32 = 0xFFFF_FFFF;
    const STD_OUTPUT_HANDLE: u32 = 0xFFFF_FFF4;
    const STD_ERROR_HANDLE: u32 = 0xFFFF_FFF5;
    let conout: [u16; 8] = [b'C' as u16, b'O' as u16, b'N' as u16, b'O' as u16, b'U' as u16, b'T' as u16, b'$' as u16, 0];
    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS) != 0 {
            let handle = CreateFileW(
                conout.as_ptr(),
                0xC000_0000,
                0x0000_0002,
                std::ptr::null_mut(),
                3,
                0,
                std::ptr::null_mut(),
            );
            if !handle.is_null() {
                SetStdHandle(STD_OUTPUT_HANDLE, handle);
                SetStdHandle(STD_ERROR_HANDLE, handle);
            }
        }
    }
}

#[cfg(not(windows))]
fn attach_parent_console() {}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let headless_mode = args.iter().any(|a| a == "--list" || a == "--get" || a == "--help" || a == "-h");
    if headless_mode {
        attach_parent_console();
    }
    if args.iter().any(|a| a == "--list" || a == "--get") {
        std::process::exit(headless(&args));
    }
    if args.iter().any(|a| a == "--help" || a == "-h") {
        usage();
    }
    let demo = args.iter().any(|a| a == "--demo");
    let icon = egui::IconData {
        rgba: include_bytes!("../assets/icon64.rgba").to_vec(),
        width: 64,
        height: 64,
    };
    let options = eframe::NativeOptions {
        centered: true,
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 820.0])
            .with_min_inner_size([1020.0, 680.0])
            .with_title("MegaDL")
            .with_icon(icon),
        ..Default::default()
    };
    if let Err(e) = eframe::run_native(
        "MegaDL",
        options,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, demo)))),
    ) {
        eprintln!("界面启动失败：{e}");
        std::process::exit(1);
    }
}
