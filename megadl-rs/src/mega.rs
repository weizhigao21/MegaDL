//! MEGA 公共分享协议：链接解析、属性/密钥解密、API 请求。
//! 与 mega_proto/mega.py 逐字段对齐，来源为 MegaDownloader 1.8 的静态逆向。

use std::fmt;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit};
use aes::Aes128;
use base64::alphabet;
use base64::engine::{general_purpose::GeneralPurposeConfig, DecodePaddingMode, GeneralPurpose};
use base64::Engine;
use regex::Regex;
use serde_json::{json, Value};

pub const API_G: &str = "https://g.api.mega.co.nz/cs";
pub const API_EU: &str = "https://eu.api.mega.co.nz/cs";

const B64: GeneralPurpose = GeneralPurpose::new(
    &alphabet::URL_SAFE,
    GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

// ---------------------------------------------------------------- 错误

#[derive(Debug)]
pub enum Error {
    /// MEGA API 负数错误码
    Api(i64, String),
    /// 本地错误（链接格式、解密、IO、网络）
    Local(String),
    Cancelled,
    /// 重名策略为“跳过”且目标已存在
    Skipped,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Api(code, where_) => {
                write!(f, "{}", api_message(*code))?;
                if !where_.is_empty() {
                    write!(f, "（{where_}）")?;
                }
                Ok(())
            }
            Error::Local(msg) => write!(f, "{msg}"),
            Error::Cancelled => write!(f, "已取消"),
            Error::Skipped => write!(f, "已跳过（文件已存在）"),
        }
    }
}

impl std::error::Error for Error {}

pub fn api_message(code: i64) -> String {
    let text = match code {
        -1 => "EINTERNAL: 内部错误",
        -2 => "EARGS: 参数错误",
        -3 => "EAGAIN: 重试",
        -4 => "ERATELIMIT: 请求频率超限",
        -5 => "EFAILED: 请求失败",
        -6 => "ETOOMANY: 连接数过多",
        -7 => "ERANGE: Range 越界",
        -8 => "EEXPIRED: 下载链接过期",
        -9 => "ENOENT: 节点不存在",
        -10 => "ECIRCULAR: 循环链接",
        -11 => "EACCESS: 无访问权限",
        -12 => "EEXIST: 已存在",
        -13 => "EINCOMPLETE: 上传未完成",
        -14 => "EKEY: 密钥错误",
        -15 => "ESID: 会话过期",
        -16 => "EBLOCKED: 已被封禁",
        -17 => "EOVERQUOTA: 配额超限",
        -18 => "ETEMPUNAVAIL: 暂时不可用",
        -19 => "ETOOMANYCONNECTIONS: 连接过多",
        -20 => "EWRITE: 写入失败",
        -21 => "EREAD: 读取失败",
        -22 => "EAPPAUTH: 应用密钥错误",
        -23 => "ETEMPDOWN: 临时下载不可用",
        -24 => "EGOINGOVERQUOTA: 超出配额",
        _ => return format!("未知错误 {code}"),
    };
    text.to_string()
}

fn local(msg: impl Into<String>) -> Error {
    Error::Local(msg.into())
}

pub type Result<T> = std::result::Result<T, Error>;

// ---------------------------------------------------------------- 基础工具

/// 解析分享链接里的密钥。文件链接是 32 字节（43 个 base64url 字符），
/// 文件夹链接是 16 字节（22 个字符）。常见错误是从聊天软件/网页里复制时
/// 被截断，这里给出能看懂的提示。
pub fn link_key_bytes(link: &Link) -> Result<Vec<u8>> {
    let (chars, bytes) = match link.kind {
        LinkKind::File => (43usize, 32usize),
        LinkKind::Folder => (22usize, 16usize),
    };
    let cleaned: String = link
        .key
        .chars()
        .filter(|&c| c != ',' && !c.is_whitespace() && c != '=')
        .collect();
    let len = cleaned.chars().count();
    if len != chars {
        let what = match link.kind {
            LinkKind::File => "文件",
            LinkKind::Folder => "文件夹",
        };
        return Err(local(format!(
            "分享链接的密钥不完整：当前 {len} 个字符，{what}链接应为 {chars} 个。请重新复制完整链接（不要手工截断）"
        )));
    }
    let key = B64
        .decode(cleaned.as_bytes())
        .map_err(|_| local("分享链接的密钥无效，请重新复制完整链接".to_string()))?;
    if key.len() != bytes {
        return Err(local(format!(
            "分享链接的密钥长度异常：解析出 {} 字节，应为 {bytes} 字节。请重新复制完整链接",
            key.len()
        )));
    }
    Ok(key)
}

pub fn b64url_decode(s: &str) -> Result<Vec<u8>> {
    let cleaned: String = s.chars().filter(|&c| c != ',' && !c.is_whitespace()).collect();
    B64.decode(cleaned.as_bytes()).map_err(|e| local(format!("base64 解码失败: {e}")))
}

pub fn b64url_encode(b: &[u8]) -> String {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    URL_SAFE_NO_PAD.encode(b)
}

/// 网络错误转成便于阅读的短句，避免把带签名的长 URL 直接铺到界面上。
pub fn net_error_text(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "请求超时".into()
    } else if e.is_connect() {
        "无法连接 MEGA 服务器（检查网络或代理设置）".into()
    } else if e.is_request() {
        "请求发送失败".into()
    } else {
        let s = e.to_string();
        match s.find(" for url (") {
            Some(i) => s[..i].to_string(),
            None => s,
        }
    }
}

/// MEGA 文件密钥：前 16 字节与后 16 字节异或得到 AES-128 密钥。
pub fn file_aes_key(key: &[u8]) -> [u8; 16] {
    let mut out = [0u8; 16];
    if key.len() == 16 {
        out.copy_from_slice(key);
    } else if key.len() >= 32 {
        for i in 0..16 {
            out[i] = key[i] ^ key[i + 16];
        }
    }
    out
}

fn aes128(key: &[u8]) -> Aes128 {
    Aes128::new((&file_aes_key(key)).into())
}

/// 解密文件属性：AES-CBC，IV 全 0，密文尾部补 0 到 16 字节对齐，明文以 "MEGA" 开头。
pub fn decrypt_attrs(at_b64: &str, key: &[u8]) -> Result<Value> {
    let mut data = b64url_decode(at_b64)?;
    let pad = (16 - data.len() % 16) % 16;
    data.extend(std::iter::repeat(0u8).take(pad));
    if data.is_empty() {
        return Err(Error::Api(-14, "文件属性为空".into()));
    }
    let cipher = aes128(key);
    let mut prev = [0u8; 16];
    let mut plain = Vec::with_capacity(data.len());
    for chunk in data.chunks_exact(16) {
        let mut block = aes::Block::clone_from_slice(chunk);
        let ciphered = block;
        cipher.decrypt_block(&mut block);
        for i in 0..16 {
            block[i] ^= prev[i];
        }
        plain.extend_from_slice(&block);
        prev.copy_from_slice(&ciphered);
    }
    let text = String::from_utf8_lossy(&plain);
    let text = text.trim_matches('\0');
    let body = text.strip_prefix("MEGA").ok_or_else(|| Error::Api(-14, "文件属性解密失败".into()))?;
    match serde_json::from_str::<Value>(body) {
        Ok(v) => Ok(v),
        Err(_) => {
            // 与原型一致：JSON 损坏时退化为正则取文件名
            let re = Regex::new(r#""n"\s*:\s*"(.*?)""#).unwrap();
            if let Some(cap) = re.captures(body) {
                Ok(json!({"n": &cap[1]}))
            } else {
                Ok(json!({}))
            }
        }
    }
}

/// 用文件夹密钥 AES-128-ECB 解密节点密钥（k 字段形如 "handle:key"）。
pub fn decrypt_node_key(k_field: &str, folder_key: &[u8]) -> Result<Vec<u8>> {
    let part = k_field.split(':').nth(1).unwrap_or(k_field);
    let blob = b64url_decode(part)?;
    if blob.len() % 16 != 0 {
        return Err(Error::Api(-14, "节点密钥长度错误".into()));
    }
    let cipher = aes128(folder_key);
    let mut out = Vec::with_capacity(blob.len());
    for chunk in blob.chunks_exact(16) {
        let mut block = aes::Block::clone_from_slice(chunk);
        cipher.decrypt_block(&mut block);
        out.extend_from_slice(&block);
    }
    Ok(out)
}

/// 按文件绝对偏移解密一段数据（AES-CTR：nonce=key[16..24]+8 字节 0，块计数大端进位）。
pub fn aes_ctr_decrypt(data: &[u8], key: &[u8], offset: u64) -> Vec<u8> {
    let cipher = aes128(key);
    let mut counter = [0u8; 16];
    if key.len() >= 24 {
        counter[..8].copy_from_slice(&key[16..24]);
    }
    let base = u128::from_be_bytes(counter);
    let mut ctr = base.wrapping_add((offset / 16) as u128);
    let mut ks_pos = (offset % 16) as usize;
    let mut ks = aes::Block::from(ctr.to_be_bytes());
    cipher.encrypt_block(&mut ks);
    let mut out = Vec::with_capacity(data.len());
    for &b in data {
        if ks_pos == 16 {
            ctr = ctr.wrapping_add(1);
            ks = aes::Block::from(ctr.to_be_bytes());
            cipher.encrypt_block(&mut ks);
            ks_pos = 0;
        }
        out.push(b ^ ks[ks_pos]);
        ks_pos += 1;
    }
    out
}

// ---------------------------------------------------------------- 链接解析

#[derive(Debug, Clone, PartialEq)]
pub enum LinkKind {
    File,
    Folder,
}

#[derive(Debug, Clone)]
pub struct Link {
    pub kind: LinkKind,
    pub id: String,
    pub key: String,
}

pub fn parse_link(link: &str) -> Result<Link> {
    let link = link.trim();
    let id_pat = r"([A-Za-z0-9_-]{6,})";
    let key_pat = r"([A-Za-z0-9_\-+,]{20,})";
    let folder_new = Regex::new(&format!(r"/folder/{id_pat}#{key_pat}")).unwrap();
    if let Some(c) = folder_new.captures(link) {
        return Ok(Link { kind: LinkKind::Folder, id: c[1].into(), key: c[2].into() });
    }
    let file_new = Regex::new(&format!(r"/file/{id_pat}#{key_pat}")).unwrap();
    if let Some(c) = file_new.captures(link) {
        return Ok(Link { kind: LinkKind::File, id: c[1].into(), key: c[2].into() });
    }
    let old = Regex::new(&format!(r"#(!|F!){id_pat}!{key_pat}")).unwrap();
    if let Some(c) = old.captures(link) {
        let kind = if c[1].starts_with('F') { LinkKind::Folder } else { LinkKind::File };
        return Ok(Link { kind, id: c[2].into(), key: c[3].into() });
    }
    let bare = Regex::new(&format!(r"(?:^|!){id_pat}!([A-Za-z0-9_\-+,]{{16,}})")).unwrap();
    if let Some(c) = bare.captures(link) {
        return Ok(Link { kind: LinkKind::File, id: c[1].into(), key: c[2].into() });
    }
    Err(local(format!("无法识别的 MEGA 链接：{link}")))
}

// ---------------------------------------------------------------- API

#[derive(Debug, Clone)]
pub struct FileInfo {
    pub size: u64,
    pub at: String,
    pub url: String,
}

#[derive(Debug, Clone)]
pub struct Node {
    pub name: String,
    pub size: u64,
    pub handle: String,
    pub key: Vec<u8>,
    pub key_b64: String,
    pub parent: Option<String>,
    pub is_dir: bool,
}

pub struct MegaApi {
    endpoints: Mutex<Vec<String>>,
    seq: Mutex<u64>,
    client: reqwest::blocking::Client,
}

/// 统一构造 HTTP 客户端；proxy 为空时直连（reqwest 自动读取 HTTP(S)_PROXY 环境变量）。
pub fn http_client(proxy: &str, connect_secs: u64, total_secs: u64) -> reqwest::blocking::Client {
    let mut builder = reqwest::blocking::Client::builder()
        .user_agent("MegaDL/0.1 (MegaDownloader compatible)")
        .connect_timeout(Duration::from_secs(connect_secs))
        .timeout(Duration::from_secs(total_secs))
        // 分块下载会同时占用多条连接，池容量要够，避免每条 Range 都重新握手
        .pool_max_idle_per_host(32)
        .pool_idle_timeout(Duration::from_secs(90));
    let trimmed = proxy.trim();
    if !trimmed.is_empty() {
        match reqwest::Proxy::all(trimmed) {
            Ok(p) => builder = builder.proxy(p),
            Err(e) => eprintln!("代理地址无效，已忽略：{e}"),
        }
    }
    builder.build().expect("HTTP client")
}

impl MegaApi {
    pub fn new(eu_first: bool, proxy: &str) -> Self {
        let endpoints = if eu_first {
            vec![API_EU.to_string(), API_G.to_string()]
        } else {
            vec![API_G.to_string(), API_EU.to_string()]
        };
        let seq = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64;
        let client = http_client(proxy, 15, 60);
        Self { endpoints: Mutex::new(endpoints), seq: Mutex::new(seq), client }
    }

    /// 当前优先线路（用于状态栏展示）
    pub fn preferred(&self) -> String {
        let eps = self.endpoints.lock().unwrap();
        if eps[0].contains("eu.api") { "EU".into() } else { "G".into() }
    }

    fn next_id(&self) -> u64 {
        let mut seq = self.seq.lock().unwrap();
        *seq += 1;
        *seq
    }

    pub fn request(&self, payload: &Value, node: Option<&str>) -> Result<Value> {
        let id = self.next_id();
        let mut last = String::new();
        for attempt in 0..6u64 {
            let base = self.endpoints.lock().unwrap()[0].clone();
            let mut url = format!("{base}?id={id}");
            if let Some(n) = node {
                url.push_str(&format!("&n={n}"));
            }
            let resp = self
                .client
                .post(&url)
                .header("Content-Type", "application/json")
                .body(payload.to_string())
                .send();
            match resp {
                Ok(r) => {
                    let status = r.status().as_u16();
                    if status == 509 {
                        return Err(Error::Api(-17, "带宽/配额限制".into()));
                    }
                    let body = r.text().unwrap_or_default();
                    if status >= 400 {
                        last = format!("HTTP {status}");
                    } else {
                        let data: Value = serde_json::from_str(body.trim())
                            .map_err(|_| Error::Api(-5, format!("响应不是 JSON: {}", &body[..body.len().min(200)])))?;
                        if let Some(code) = data.as_i64() {
                            return Err(Error::Api(code, String::new()));
                        }
                        if let Some(arr) = data.as_array() {
                            if let Some(code) = arr.first().and_then(|v| v.as_i64()) {
                                return Err(Error::Api(code, String::new()));
                            }
                        }
                        return Ok(data);
                    }
                }
                Err(e) => {
                    last = net_error_text(&e);
                    // 发送失败：G/EU 互换后重试（本机 g.api 常不可达，EU 偶发抖动）
                    let rotate = {
                        let mut eps = self.endpoints.lock().unwrap();
                        if eps.len() > 1 {
                            eps.rotate_left(1);
                            true
                        } else {
                            false
                        }
                    };
                    if rotate {
                        std::thread::sleep(Duration::from_millis(300 * (attempt + 1)));
                        continue;
                    }
                }
            }
            std::thread::sleep(Duration::from_secs(1 + attempt * 2));
        }
        Err(Error::Api(-5, format!("网络请求失败: {last}")))
    }

    /// g 请求。文件夹分享内的文件：body 用 n=<文件句柄>，URL 的 &n= 用 <文件夹 ID>。
    pub fn file_info(&self, file_id: Option<&str>, node: Option<&str>, folder: Option<&str>) -> Result<FileInfo> {
        let mut item = json!({"a": "g", "g": 1, "ssl": 0});
        if let Some(n) = node {
            item["n"] = json!(n);
        } else {
            item["p"] = json!(file_id.unwrap_or_default());
        }
        let data = self.request(&json!([item]), folder.or(node))?;
        let info = if data.is_array() { data[0].clone() } else { data };
        if let Some(code) = info.get("e").and_then(|v| v.as_i64()) {
            if code != 0 {
                return Err(Error::Api(code, "获取文件信息".into()));
            }
        }
        let size = info.get("s").and_then(|v| v.as_u64());
        let at = info.get("at").and_then(|v| v.as_str());
        let url = info.get("g").and_then(|v| v.as_str());
        match (size, at, url) {
            (Some(s), Some(a), Some(g)) => Ok(FileInfo { size: s, at: a.into(), url: g.into() }),
            _ => Err(Error::Api(-5, "响应缺少字段 s/at/g".into())),
        }
    }

    /// f 请求：列出公共文件夹节点。
    pub fn folder_nodes(&self, folder_id: &str) -> Result<Vec<Value>> {
        let data = self.request(&json!([{"a": "f", "c": 1, "r": 1}]), Some(folder_id))?;
        let resp = if data.is_array() { data[0].clone() } else { data };
        if let Some(code) = resp.get("e").and_then(|v| v.as_i64()) {
            if code != 0 {
                return Err(Error::Api(code, "获取文件夹列表".into()));
            }
        }
        Ok(resp.get("f").and_then(|v| v.as_array()).cloned().unwrap_or_default())
    }

    /// 解密文件夹全部节点（t=0 文件、t=1 文件夹），并保留父子句柄用于构建文件树。
    pub fn enrich_folder(&self, link: &Link) -> Result<Vec<Node>> {
        let folder_key = link_key_bytes(link)?;
        let raw = self.folder_nodes(&link.id)?;
        let mut nodes = Vec::new();
        for n in raw {
            let node_type = n.get("t").and_then(|v| v.as_i64()).unwrap_or(0);
            if node_type != 0 && node_type != 1 {
                continue;
            }
            let k = match n.get("k").and_then(|v| v.as_str()) {
                Some(k) => k,
                None => continue,
            };
            let a = match n.get("a").and_then(|v| v.as_str()) {
                Some(a) => a,
                None => continue,
            };
            let node_key = match decrypt_node_key(k, &folder_key) {
                Ok(k) => k,
                Err(_) => continue,
            };
            let attrs = match decrypt_attrs(a, &node_key) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let handle = n.get("h").and_then(|v| v.as_str()).unwrap_or("unnamed").to_string();
            let name = attrs.get("n").and_then(|v| v.as_str()).unwrap_or(&handle).to_string();
            nodes.push(Node {
                name,
                size: if node_type == 0 { n.get("s").and_then(|v| v.as_u64()).unwrap_or(0) } else { 0 },
                handle,
                key_b64: b64url_encode(&node_key),
                key: node_key,
                parent: n.get("p").and_then(|v| v.as_str()).map(str::to_string),
                is_dir: node_type == 1,
            });
        }
        Ok(nodes)
    }
}

// ---------------------------------------------------------------- 杂项

pub fn fmt_size(n: u64) -> String {
    let mut v = n as f64;
    for unit in ["B", "KB", "MB", "GB", "TB"] {
        if v < 1024.0 || unit == "TB" {
            return if unit == "B" { format!("{n}B") } else { format!("{v:.1}{unit}") };
        }
        v /= 1024.0;
    }
    unreachable!()
}

pub fn safe_filename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if "<>:\"/\\|?*".contains(c) || (c as u32) < 0x20 { '_' } else { c })
        .collect();
    let trimmed = cleaned.trim_matches(|c| c == ' ' || c == '.');
    if trimmed.is_empty() { "unnamed".into() } else { trimmed.into() }
}

// ---------------------------------------------------------------- 单元测试（与 selftest.py 同源）

#[cfg(test)]
mod tests {
    use super::*;

    fn make_key(seed: u8) -> Vec<u8> {
        (0..32u8).map(|i| seed.wrapping_add(i)).collect()
    }

    #[test]
    fn parse_links() {
        let k = "A".repeat(43);
        let cases = [
            (format!("https://mega.nz/file/AbCdEf12#{k}"), LinkKind::File),
            (format!("https://mega.nz/folder/AbCdEf12#{k}"), LinkKind::Folder),
            (format!("https://mega.nz/#!AbCdEf12!{k}"), LinkKind::File),
            (format!("https://mega.co.nz/#F!AbCdEf12!{k}"), LinkKind::Folder),
        ];
        for (link, kind) in cases {
            let p = parse_link(&link).unwrap();
            assert_eq!(p.kind, kind);
            assert_eq!(p.id, "AbCdEf12");
            assert_eq!(p.key, k);
        }
        assert!(parse_link("not a link").is_err());
    }

    #[test]
    fn link_key_validation() {
        let file = Link { kind: LinkKind::File, id: "x".into(), key: "A".repeat(43) };
        assert_eq!(link_key_bytes(&file).unwrap().len(), 32);
        let folder = Link { kind: LinkKind::Folder, id: "x".into(), key: "A".repeat(22) };
        assert_eq!(link_key_bytes(&folder).unwrap().len(), 16);
        let short = Link {
            kind: LinkKind::File,
            id: "x".into(),
            key: "NaIRSBspvG31nsKsb7BOXDjzjUMeYIzGNIwjmJCepc".into(),
        };
        let err = link_key_bytes(&short).unwrap_err().to_string();
        assert!(err.contains("42") && err.contains("43"), "{err}");
        let bad_folder = Link { kind: LinkKind::Folder, id: "x".into(), key: "A".repeat(43) };
        assert!(link_key_bytes(&bad_folder).is_err());
    }

    #[test]
    fn attrs_roundtrip() {
        let key = make_key(1);
        let plain_body = r#"{"n":"测试文件.bin"}"#;
        let mut plain = format!("MEGA{plain_body}").into_bytes();
        let pad = (16 - plain.len() % 16) % 16;
        plain.extend(std::iter::repeat(0u8).take(pad));
        // CBC 加密（IV=0）
        let cipher = aes128(&key);
        let mut prev = [0u8; 16];
        let mut enc = Vec::new();
        for chunk in plain.chunks_exact(16) {
            let mut block = aes::Block::clone_from_slice(chunk);
            for i in 0..16 {
                block[i] ^= prev[i];
            }
            cipher.encrypt_block(&mut block);
            prev.copy_from_slice(&block);
            enc.extend_from_slice(&block);
        }
        let at = b64url_encode(&enc);
        let got = decrypt_attrs(&at, &key).unwrap();
        assert_eq!(got["n"], "测试文件.bin");
    }

    #[test]
    fn ctr_offsets() {
        let key = make_key(7);
        let data: Vec<u8> = (0..1024).map(|i| (i % 256) as u8).collect();
        let full = aes_ctr_decrypt(&data, &key, 0);
        for off in [16u64, 32, 48, 160] {
            let part = aes_ctr_decrypt(&data[off as usize..off as usize + 64], &key, off);
            assert_eq!(part, full[off as usize..off as usize + 64], "offset {off}");
        }
        let part = aes_ctr_decrypt(&data[9..41], &key, 9);
        assert_eq!(part, full[9..41]);
    }

    #[test]
    fn node_key_roundtrip() {
        let folder_key = make_key(21);
        let node_key = make_key(33);
        let cipher = aes128(&folder_key);
        let mut enc = Vec::new();
        for chunk in node_key.chunks_exact(16) {
            let mut block = aes::Block::clone_from_slice(chunk);
            cipher.encrypt_block(&mut block);
            enc.extend_from_slice(&block);
        }
        let k_field = format!("handle:{}", b64url_encode(&enc));
        let got = decrypt_node_key(&k_field, &folder_key).unwrap();
        assert_eq!(got, node_key);
    }

    #[test]
    fn filename_sanitize() {
        assert_eq!(safe_filename("a/b\\c:d?.mp3"), "a_b_c_d_.mp3");
        assert_eq!(safe_filename("  .. "), "unnamed");
    }

    /// 跨语言校验：以下向量由 mega_proto（Python + pycryptodome）生成，
    /// 保证 Rust 实现与已验证过真实 API 的 Python 原型逐字节一致。
    #[test]
    fn cross_language_vectors() {
        // attrs：key=seed1，明文 MEGA{"n":"测试文件.bin"}（UTF-8 原文）
        let key = make_key(1);
        let at = "o4iU-WzKJIsne6Y5w4D3W4dRzv2oBmaWAl9XN_MU1Qk";
        let attrs = decrypt_attrs(at, &key).unwrap();
        assert_eq!(attrs["n"], "测试文件.bin");

        // CTR：key=seed7，data=bytes(range(256))*4，offset 9 取 32 字节
        let k7 = make_key(7);
        let data: Vec<u8> = (0..1024).map(|i| (i % 256) as u8).collect();
        let got = aes_ctr_decrypt(&data[9..41], &k7, 9);
        let expect = hex_bytes("70c8659c9fe9ae5d310d373b152e38961e356b8051dc9258ce9b38ddeafe027d");
        assert_eq!(got, expect);

        // 节点密钥：folder=seed21，node=seed33
        let fk = make_key(21);
        let nk = make_key(33);
        let k_field = format!("handle:YWMt0eXYsyCkt2U12-d0TniXxk1Ue9wCrlBLlCub12Y");
        assert_eq!(decrypt_node_key(&k_field, &fk).unwrap(), nk);
    }

    fn hex_bytes(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }
}
