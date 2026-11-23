//! 端到端测试：本地假 MEGA Range 服务，验证多线程分块下载 + CTR 解密 + 重名策略。
//! 数据与 mega_proto/test_local.py 完全同源（3 MiB + 1234 字节）。

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::AtomicBool;
use std::thread;

use megadl::download::{Collision, Downloader};
use megadl::mega::{aes_ctr_decrypt, FileInfo};

fn data() -> Vec<u8> {
    (0..3 * 1024 * 1024 + 1234usize).map(|i| ((i * 7) & 0xFF) as u8).collect()
}

fn key() -> Vec<u8> {
    (0..32u8).map(|i| i.wrapping_mul(3).wrapping_add(5)).collect()
}

fn serve(listener: TcpListener, plain: Vec<u8>, key: Vec<u8>) {
    for stream in listener.incoming() {
        let Ok(mut stream) = stream else { continue };
        let plain = plain.clone();
        let key = key.clone();
        thread::spawn(move || {
            let mut buf = [0u8; 4096];
            let mut req = Vec::new();
            loop {
                let Ok(n) = stream.read(&mut buf) else { return };
                if n == 0 {
                    return;
                }
                req.extend_from_slice(&buf[..n]);
                if req.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            let text = String::from_utf8_lossy(&req);
            let total = plain.len();
            let range = text.lines().find_map(|l| {
                let lower = l.to_ascii_lowercase();
                lower.strip_prefix("range: bytes=").map(|r| r.trim().to_string())
            });
            let (start, end, status) = match range {
                Some(r) => {
                    let (a, b) = r.split_once('-').unwrap();
                    let start: usize = a.parse().unwrap();
                    let end: usize = b.parse::<usize>().unwrap().min(total - 1);
                    (start, end, "206 Partial Content")
                }
                None => (0, total - 1, "200 OK"),
            };
            let chunk = &plain[start..=end];
            let enc = aes_ctr_decrypt(chunk, &key, start as u64);
            let head = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nContent-Range: bytes {start}-{end}/{total}\r\nConnection: close\r\n\r\n",
                enc.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(&enc);
        });
    }
}

#[test]
fn download_end_to_end() {
    let plain = data();
    let k = key();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    {
        let plain = plain.clone();
        let k = k.clone();
        thread::spawn(move || serve(listener, plain, k));
    }
    let url = format!("http://127.0.0.1:{port}/file");
    let dir = std::env::temp_dir().join(format!("megadl_e2e_{port}"));
    std::fs::create_dir_all(&dir).unwrap();
    let dest = dir.join("sample.bin");
    let info = FileInfo { size: plain.len() as u64, at: String::new(), url };

    let dl = Downloader::new(4, 1, false, "");
    let cancel = AtomicBool::new(false);
    let p1 = dl.download(&info, &k, &dest, Collision::Rename, None, Some(&cancel)).unwrap();
    assert_eq!(std::fs::read(&p1).unwrap(), plain, "内容不一致");

    // 重名策略：自动重命名
    let p2 = dl.download(&info, &k, &dest, Collision::Rename, None, Some(&cancel)).unwrap();
    assert_ne!(p1, p2);
    assert!(p2.file_name().unwrap().to_string_lossy().contains("(1)"));
    assert_eq!(std::fs::read(&p2).unwrap(), plain);

    // 跳过策略
    let err = dl.download(&info, &k, &dest, Collision::Skip, None, Some(&cancel)).unwrap_err();
    assert!(matches!(err, megadl::mega::Error::Skipped));

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn cancel_stops_download() {
    let plain = data();
    let k = key();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    {
        let plain = plain.clone();
        let k = k.clone();
        thread::spawn(move || serve(listener, plain, k));
    }
    let url = format!("http://127.0.0.1:{port}/file");
    let dir = std::env::temp_dir().join(format!("megadl_cancel_{port}"));
    std::fs::create_dir_all(&dir).unwrap();
    let dest = dir.join("cancel.bin");
    let info = FileInfo { size: plain.len() as u64, at: String::new(), url };
    let dl = Downloader::new(2, 1, false, "");
    let cancel = AtomicBool::new(true); // 预先取消
    let err = dl.download(&info, &k, &dest, Collision::Rename, None, Some(&cancel)).unwrap_err();
    assert!(matches!(err, megadl::mega::Error::Cancelled));
    assert!(!dest.exists());
    std::fs::remove_dir_all(&dir).ok();
}
