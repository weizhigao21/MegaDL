//! 多线程 Range 分块下载 + AES-CTR 解密，.part 预占位，完成后重命名。

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use crate::mega::{aes_ctr_decrypt, Error, FileInfo, Result};

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Collision {
    Rename,
    Overwrite,
    Skip,
}

pub struct Downloader {
    pub connections: usize,
    pub part_size: u64,
    prefer_eu: bool,
    client: reqwest::blocking::Client,
}

/// 下载进度回调：本次新增的字节数
pub type OnProgress<'a> = &'a (dyn Fn(u64) + Send + Sync);

impl Downloader {
    pub fn new(connections: usize, part_size_mb: u64, prefer_eu: bool, proxy: &str) -> Self {
        let part = (part_size_mb.max(1) * 1024 * 1024 / 16) * 16;
        let client = crate::mega::http_client(proxy, 15, 120);
        Self { connections: connections.max(1), part_size: part.max(16 * 1024), prefer_eu, client }
    }

    /// 目标：每个连接至少分到 4 块，避免中等文件只跑 2–3 条连接；
    /// 大文件仍用配置的大块，减少请求与握手开销。
    fn effective_part_size(&self, size: u64) -> u64 {
        let target = size / (self.connections as u64 * 4);
        let capped = target.min(self.part_size);
        ((capped / 16384) * 16384).max(16 * 1024)
    }

    /// 下载到 dest_path；返回实际落盘路径（重名策略可能改名/跳过）。
    pub fn download(
        &self,
        info: &FileInfo,
        key: &[u8],
        dest_path: &Path,
        collision: Collision,
        on_progress: Option<OnProgress<'_>>,
        cancel: Option<&AtomicBool>,
    ) -> Result<PathBuf> {
        if let Some(parent) = dest_path.parent() {
            fs::create_dir_all(parent).map_err(|e| Error::Local(format!("创建目录失败: {e}")))?;
        }
        let dest = match resolve_collision(dest_path, collision) {
            Some(p) => p,
            None => return Err(Error::Skipped),
        };
        let part_path = PathBuf::from(format!("{}.part", dest.display()));
        let size = info.size;

        if size == 0 {
            File::create(&part_path).map_err(io_err)?;
            finalize(&part_path, &dest)?;
            return Ok(dest);
        }

        {
            let f = File::create(&part_path).map_err(io_err)?;
            f.set_len(size).map_err(io_err)?;
        }

        let part_size = self.effective_part_size(size);
        let total_parts = (size + part_size - 1) / part_size;
        let next = Mutex::new(0u64);
        let first_err: Mutex<Option<Error>> = Mutex::new(None);
        let key = key.to_vec();
        let url = info.url.clone();
        let cancelled = AtomicBool::new(false);

        let n_threads = self.connections.min(total_parts as usize);
        std::thread::scope(|scope| {
            for _ in 0..n_threads {
                let next = &next;
                let first_err = &first_err;
                let cancelled = &cancelled;
                let key = &key;
                let url = &url;
                let part_path = &part_path;
                scope.spawn(move || loop {
                    if cancelled.load(Ordering::Relaxed) {
                        return;
                    }
                    let idx = {
                        let mut guard = next.lock().unwrap();
                        if first_err.lock().unwrap().is_some() {
                            return;
                        }
                        if let Some(c) = cancel {
                            if c.load(Ordering::Relaxed) {
                                *first_err.lock().unwrap() = Some(Error::Cancelled);
                                cancelled.store(true, Ordering::Relaxed);
                                return;
                            }
                        }
                        let idx = *guard;
                        if idx >= total_parts {
                            return;
                        }
                        *guard += 1;
                        idx
                    };
                    let start = idx * part_size;
                    let end = (start + part_size).min(size) - 1;
                    match self.fetch_part(url, key, start, end, part_path, cancel, &cancelled, on_progress) {
                        Ok(()) => {}
                        Err(e) => {
                            let mut guard = first_err.lock().unwrap();
                            if guard.is_none() {
                                *guard = Some(e);
                            }
                            cancelled.store(true, Ordering::Relaxed);
                            return;
                        }
                    }
                });
            }
        });

        if let Some(e) = first_err.into_inner().unwrap() {
            return Err(e);
        }
        finalize(&part_path, &dest)?;
        Ok(dest)
    }

    fn fetch_part(
        &self,
        url: &str,
        key: &[u8],
        start: u64,
        end: u64,
        part_path: &Path,
        cancel: Option<&AtomicBool>,
        cancelled: &AtomicBool,
        on_progress: Option<OnProgress<'_>>,
    ) -> Result<()> {
        let mut candidates = vec![url.to_string()];
        if url.contains("g.api.mega.co.nz") {
            candidates.push(url.replace("g.api.mega.co.nz", "eu.api.mega.co.nz"));
        } else if url.contains("eu.api.mega.co.nz") {
            candidates.push(url.replace("eu.api.mega.co.nz", "g.api.mega.co.nz"));
        }
        if self.prefer_eu && candidates.len() > 1 {
            candidates.swap(0, 1);
        }
        let want = end - start + 1;
        let mut last = String::new();
        // 已经回报给界面的绝对结束偏移：重试时只补报新增字节，进度不会回退或翻倍
        let mut reported = start;
        for attempt in 0..5usize {
            if cancelled.load(Ordering::Relaxed) || cancel.map(|c| c.load(Ordering::Relaxed)).unwrap_or(false) {
                return Err(Error::Cancelled);
            }
            let target = &candidates[attempt % candidates.len()];
            let resp = self
                .client
                .get(target)
                .header("Range", format!("bytes={start}-{end}"))
                .send();
            match resp {
                Ok(mut r) => {
                    let status = r.status().as_u16();
                    if status == 509 {
                        return Err(Error::Api(-17, "带宽/配额限制".into()));
                    }
                    if status == 416 {
                        return Err(Error::Api(-7, "Range 不被接受".into()));
                    }
                    if status == 200 && start != 0 {
                        return Err(Error::Api(-7, "服务器忽略了 Range".into()));
                    }
                    if status != 200 && status != 206 {
                        // 502/503/504 这类网关错误换个线路再来
                        last = format!("下载 HTTP {status}");
                    } else {
                        match Self::stream_body(
                            &mut r,
                            want,
                            start,
                            key,
                            part_path,
                            on_progress,
                            cancel,
                            cancelled,
                            &mut reported,
                        )? {
                            None => return Ok(()),
                            Some(msg) => last = msg,
                        }
                    }
                }
                Err(e) => last = crate::mega::net_error_text(&e),
            }
            // 只用很短的退避：连接被服务器重置时，1 秒以上的固定等待会把整体速度打断成“停几秒”
            std::thread::sleep(retry_backoff(attempt));
        }
        Err(Error::Api(-5, format!("分块下载失败: {last}")))
    }

    /// 一边收一边解密落盘，每 256KB 回报一次进度，避免整块收完才刷新导致速度看上去归零。
    #[allow(clippy::too_many_arguments)]
    fn stream_body(
        resp: &mut reqwest::blocking::Response,
        want: u64,
        start: u64,
        key: &[u8],
        part_path: &Path,
        on_progress: Option<OnProgress<'_>>,
        cancel: Option<&AtomicBool>,
        cancelled: &AtomicBool,
        reported: &mut u64,
    ) -> Result<Option<String>> {
        let mut f = OpenOptions::new().write(true).open(part_path).map_err(io_err)?;
        f.seek(SeekFrom::Start(start)).map_err(io_err)?;
        let mut buf = vec![0u8; 256 * 1024];
        let mut pos = 0u64;
        loop {
            if cancelled.load(Ordering::Relaxed) || cancel.map(|c| c.load(Ordering::Relaxed)).unwrap_or(false) {
                return Err(Error::Cancelled);
            }
            match resp.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let n = n as u64;
                    if pos + n > want {
                        return Ok(Some(format!("分块超出预期：需要 {want}，收到 {}", pos + n)));
                    }
                    let plain = aes_ctr_decrypt(&buf[..n as usize], key, start + pos);
                    f.write_all(&plain).map_err(io_err)?;
                    pos += n;
                    let abs_end = start + pos;
                    if abs_end > *reported {
                        if let Some(cb) = on_progress {
                            cb(abs_end - *reported);
                        }
                        *reported = abs_end;
                    }
                }
                Err(e) => return Ok(Some(format!("分块读取中断: {e}"))),
            }
        }
        if pos != want {
            return Ok(Some(format!("分块长度不符：需要 {want}，收到 {pos}")));
        }
        if cancelled.load(Ordering::Relaxed) || cancel.map(|c| c.load(Ordering::Relaxed)).unwrap_or(false) {
            return Err(Error::Cancelled);
        }
        Ok(None)
    }
}

/// 120ms 起步的指数退避，带一点抖动，避免所有连接同时重试。
fn retry_backoff(attempt: usize) -> Duration {
    let base = [120u64, 280, 600, 1200, 2000][attempt.min(4)];
    let jitter = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64 % 90)
        .unwrap_or(0);
    Duration::from_millis(base + jitter)
}

fn io_err(e: std::io::Error) -> Error {
    Error::Local(format!("文件写入失败: {e}"))
}

fn resolve_collision(dest: &Path, policy: Collision) -> Option<PathBuf> {
    if !occupied(dest) || policy == Collision::Overwrite {
        return Some(dest.to_path_buf());
    }
    if policy == Collision::Skip {
        return None;
    }
    let stem = dest.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let ext = dest.extension().map(|s| format!(".{}", s.to_string_lossy())).unwrap_or_default();
    let parent = dest.parent().unwrap_or(Path::new("."));
    for i in 1..10000 {
        let candidate = parent.join(format!("{stem} ({i}){ext}"));
        if !occupied(&candidate) {
            return Some(candidate);
        }
    }
    Some(dest.to_path_buf())
}

/// 目标文件或它的 .part 占位都已视为“已占用”，避免并发下载同名文件互相覆盖。
fn occupied(dest: &Path) -> bool {
    dest.exists() || PathBuf::from(format!("{}.part", dest.display())).exists()
}

fn finalize(part: &Path, dest: &Path) -> Result<()> {
    if dest.exists() {
        fs::remove_file(dest).map_err(io_err)?;
    }
    fs::rename(part, dest).map_err(io_err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mega::MegaApi;

    fn same_bytes(a: &Path, b: &Path) -> bool {
        let (fa, fb) = (fs::read(a).unwrap(), fs::read(b).unwrap());
        fa == fb
    }

    /// 真实网络：同一个文件用 8 连接和 1 连接各下一次，校验流式解密结果一致。
    #[test]
    #[ignore = "需要访问 MEGA"]
    fn real_download_streaming_matches() {
        const FOLDER: &str = "https://mega.nz/folder/cOowhBzB#8Z4nLRx0w1eriG3ejcHWWA";
        let link = crate::mega::parse_link(FOLDER).unwrap();
        let api = MegaApi::new(true, "");
        let nodes = api.enrich_folder(&link).unwrap();
        let node = nodes
            .iter()
            .find(|n| !n.is_dir && n.size > 20 * 1024 * 1024 && n.size < 45 * 1024 * 1024)
            .expect("测试文件夹里应有 20–45MB 的文件");
        let info = api.file_info(None, Some(&node.handle), Some(&link.id)).unwrap();
        let dir = std::env::temp_dir().join("megadl_stream_test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let cancel = AtomicBool::new(false);

        let d8 = Downloader::new(8, 4, true, "");
        let p8 = d8
            .download(&info, &node.key, &dir.join("eight.bin"), Collision::Overwrite, None, Some(&cancel))
            .unwrap();
        let d1 = Downloader::new(1, 4, true, "");
        let p1 = d1
            .download(&info, &node.key, &dir.join("one.bin"), Collision::Overwrite, None, Some(&cancel))
            .unwrap();
        assert_eq!(fs::metadata(&p8).unwrap().len(), info.size);
        assert!(same_bytes(&p8, &p1), "流式下载结果不一致");
        let _ = fs::remove_dir_all(&dir);
    }
}
