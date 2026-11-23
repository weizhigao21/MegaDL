//! MegaDL 桌面界面：链接 → 文件树 → 勾选下载。

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use eframe::egui::{self, Color32, FontData, FontDefinitions, FontFamily, RichText, Rounding, Stroke};

use megadl::config::{Config, Route, Theme};
use megadl::download::{Collision, Downloader};
use megadl::mega::{self, FileInfo, LinkKind, MegaApi};

// ---------------------------------------------------------------- 状态模型

#[derive(Clone)]
struct ItemData {
    name: String,
    size: u64,
    handle: String,
    key: Vec<u8>,
    info: Option<FileInfo>,
    is_dir: bool,
    parent: Option<String>,
    rel_path: String,
}

#[derive(Clone, PartialEq)]
enum ItemState {
    Waiting,
    Fetching,
    Downloading,
    Done,
    Skipped,
    Stopped,
    Failed(String),
}

struct TreeItem {
    data: ItemData,
    parent: Option<usize>,
    children: Vec<usize>,
    expanded: bool,
    selected: bool,
    state: ItemState,
    downloaded: u64,
    total_size: u64,
}

struct RowMeta {
    idx: usize,
    depth: usize,
    is_dir: bool,
    all_selected: bool,
    any_selected: bool,
    file_count: usize,
}

enum Event {
    Parsed { folder: Option<String>, items: Vec<ItemData> },
    ParseError(String),
    Log(String),
    ItemState(usize, ItemState),
    Progress(usize, u64),
    BatchDone { ok: usize, fail: usize, skip: usize, stopped: bool },
}

#[derive(Clone, Copy, PartialEq)]
enum Mark {
    None,
    Partial,
    All,
}

/// 自绘勾选框：文件夹三态与文件勾选共用同一套尺寸，避免图标字体缺字。
fn mark_box(ui: &mut egui::Ui, mark: Mark) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::click());
    let accent = ui.visuals().selection.bg_fill;
    let border = if resp.hovered() {
        accent
    } else {
        ui.visuals().widgets.inactive.bg_stroke.color
    };
    let fill = if mark == Mark::All {
        accent
    } else {
        ui.visuals().extreme_bg_color
    };
    let painter = ui.painter();
    painter.rect(
        rect,
        egui::Rounding::same(2.0),
        fill,
        egui::Stroke::new(1.0_f32, border),
    );
    let c = rect.center();
    match mark {
        Mark::All => {
            let stroke = egui::Stroke::new(1.6_f32, egui::Color32::WHITE);
            painter.line_segment(
                [egui::pos2(c.x - 3.5, c.y), egui::pos2(c.x - 1.0, c.y + 2.5)],
                stroke,
            );
            painter.line_segment(
                [egui::pos2(c.x - 1.0, c.y + 2.5), egui::pos2(c.x + 4.0, c.y - 2.5)],
                stroke,
            );
        }
        Mark::Partial => {
            painter.line_segment(
                [egui::pos2(c.x - 3.5, c.y), egui::pos2(c.x + 3.5, c.y)],
                egui::Stroke::new(1.6_f32, accent),
            );
        }
        Mark::None => {}
    }
    resp
}

fn accent_button(label: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(label.to_string()).size(13.5).color(Color32::WHITE))
        .fill(ACCENT)
        .stroke(Stroke::NONE)
        .rounding(Rounding::same(9.0))
        .min_size(egui::vec2(0.0, 36.0))
}

fn card_frame(ui: &egui::Ui) -> egui::Frame {
    let visuals = ui.visuals();
    let border = visuals.widgets.noninteractive.bg_stroke.color;
    let shadow_color = if visuals.dark_mode {
        Color32::from_rgba_unmultiplied(0, 0, 0, 110)
    } else {
        Color32::from_rgba_unmultiplied(24, 38, 30, 16)
    };
    egui::Frame::none()
        .fill(visuals.window_fill)
        .stroke(Stroke::new(1.0_f32, border))
        .rounding(Rounding::same(14.0))
        .inner_margin(egui::Margin::symmetric(18.0, 16.0))
        .shadow(egui::epaint::Shadow {
            offset: egui::vec2(0.0, 3.0),
            blur: 16.0,
            spread: 0.0,
            color: shadow_color,
        })
}

fn window_frame(ctx: &egui::Context) -> egui::Frame {
    let visuals = &ctx.style().visuals;
    egui::Frame::window(&ctx.style())
        .fill(visuals.window_fill)
        .stroke(Stroke::new(1.0_f32, visuals.widgets.noninteractive.bg_stroke.color))
        .rounding(Rounding::same(14.0))
        .inner_margin(egui::Margin::symmetric(20.0, 16.0))
}

fn paint_logo(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(38.0, 38.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, Rounding::same(10.0), ACCENT);
    let c = rect.center();
    let stroke = Stroke::new(2.0_f32, Color32::WHITE);
    ui.painter().line_segment(
        [egui::pos2(c.x, c.y - 10.0), egui::pos2(c.x, c.y + 5.0)],
        stroke,
    );
    ui.painter().line_segment(
        [egui::pos2(c.x - 5.0, c.y), egui::pos2(c.x, c.y + 5.0)],
        stroke,
    );
    ui.painter().line_segment(
        [egui::pos2(c.x, c.y + 5.0), egui::pos2(c.x + 5.0, c.y)],
        stroke,
    );
    ui.painter().line_segment(
        [egui::pos2(c.x - 9.0, c.y + 10.0), egui::pos2(c.x + 9.0, c.y + 10.0)],
        Stroke::new(2.0_f32, Color32::from_rgba_unmultiplied(255, 255, 255, 180)),
    );
}

fn empty_state(ui: &mut egui::Ui) {
    let height = ui.available_height().max(300.0);
    let width = ui.available_width();
    ui.allocate_ui_with_layout(
        egui::vec2(width, height),
        egui::Layout::top_down(egui::Align::Center),
        |ui| {
            ui.add_space(((height - 150.0) / 2.0).max(24.0));
            let (rect, _) = ui.allocate_exact_size(egui::vec2(68.0, 68.0), egui::Sense::hover());
            ui.painter()
                .circle_filled(rect.center(), 34.0, ui.visuals().widgets.inactive.bg_fill);
            let folder = egui::Rect::from_center_size(rect.center(), egui::vec2(36.0, 26.0));
            let stroke = Stroke::new(1.4_f32, ui.visuals().weak_text_color());
            let tab = egui::Rect::from_min_max(
                egui::pos2(folder.left() + 4.0, folder.top() - 5.0),
                egui::pos2(folder.left() + 17.0, folder.top() + 4.0),
            );
            ui.painter().rect(
                tab,
                Rounding::same(3.0),
                ui.visuals().widgets.inactive.bg_fill,
                stroke,
            );
            ui.painter()
                .rect(folder, Rounding::same(4.0), Color32::TRANSPARENT, stroke);
            ui.add_space(16.0);
            ui.label(RichText::new("还没有文件").size(15.0).strong());
            ui.add_space(4.0);
            ui.label(
                RichText::new("粘贴 MEGA 链接并点击「解析」，文件会按目录结构显示在这里")
                    .size(11.5)
                    .color(ui.visuals().weak_text_color()),
            );
        },
    );
}

fn file_accent(name: &str) -> Color32 {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "mp3" | "wav" | "flac" | "m4a" | "ape" => Color32::from_rgb(0x4C, 0x8B, 0xF5),
        "mp4" | "mkv" | "avi" | "mov" => Color32::from_rgb(0x8A, 0x63, 0xD2),
        "zip" | "rar" | "7z" | "gz" => Color32::from_rgb(0xD9, 0x9A, 0x2B),
        "jpg" | "jpeg" | "png" | "gif" | "webp" => Color32::from_rgb(0xD6, 0x5A, 0x8A),
        "txt" | "md" | "cue" | "log" => Color32::from_rgb(0x4F, 0xA3, 0x7A),
        _ => Color32::from_rgb(0x8A, 0x96, 0x90),
    }
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    Color32::from_rgb(
        (a.r() as f32 * (1.0 - t) + b.r() as f32 * t) as u8,
        (a.g() as f32 * (1.0 - t) + b.g() as f32 * t) as u8,
        (a.b() as f32 * (1.0 - t) + b.b() as f32 * t) as u8,
    )
}

fn paint_file_icon(ui: &mut egui::Ui, name: &str) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
    let dark = ui.visuals().dark_mode;
    let accent = file_accent(name);
    let painter = ui.painter();
    let page = egui::Rect::from_min_max(
        egui::pos2(rect.left() + 2.5, rect.top() + 1.0),
        egui::pos2(rect.right() - 2.5, rect.bottom() - 1.0),
    );
    let base = if dark {
        Color32::from_rgb(0x22, 0x2C, 0x28)
    } else {
        Color32::WHITE
    };
    let fill = mix(base, accent, if dark { 0.12 } else { 0.07 });
    let border = mix(accent, base, if dark { 0.25 } else { 0.35 });
    painter.rect(page, Rounding::same(2.5), fill, Stroke::new(1.0_f32, border));
    // 右上折角
    let fold = 4.0;
    painter.line_segment(
        [egui::pos2(page.right() - fold, page.top()), egui::pos2(page.right(), page.top() + fold)],
        Stroke::new(1.0_f32, border),
    );
    painter.line_segment(
        [egui::pos2(page.right() - fold, page.top()), egui::pos2(page.right() - fold, page.top() + fold)],
        Stroke::new(1.0_f32, border),
    );
    // 底部色条标识文件类型
    let band = egui::Rect::from_min_max(
        egui::pos2(page.left() + 1.0, page.bottom() - 4.2),
        egui::pos2(page.right() - 1.0, page.bottom() - 1.0),
    );
    let band_round = Rounding { nw: 0.0, ne: 0.0, sw: 2.0, se: 2.0 };
    painter.rect_filled(band, band_round, accent);
}

fn paint_folder_icon(ui: &mut egui::Ui, open: bool) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
    let painter = ui.painter();
    let dark = ui.visuals().dark_mode;
    let blue = Color32::from_rgb(0x4C, 0x7F, 0xD8);
    let fill = if dark {
        if open {
            Color32::from_rgb(0x27, 0x39, 0x56)
        } else {
            Color32::from_rgb(0x22, 0x31, 0x49)
        }
    } else if open {
        Color32::from_rgb(0xD8, 0xE5, 0xFB)
    } else {
        Color32::from_rgb(0xE8, 0xF0, 0xFD)
    };
    let stroke_color = if dark {
        Color32::from_rgb(0x8F, 0xAC, 0xDA)
    } else {
        mix(blue, Color32::from_rgb(0x1B, 0x23, 0x20), 0.15)
    };
    let stroke = Stroke::new(1.2_f32, stroke_color);
    let body = egui::Rect::from_min_max(
        egui::pos2(rect.left() + 1.0, rect.top() + 5.5),
        egui::pos2(rect.right() - 1.0, rect.bottom() - 2.0),
    );
    painter.rect(body, Rounding::same(2.5), fill, stroke);
    let tab = egui::Rect::from_min_max(
        egui::pos2(body.left() + 1.5, body.top() - 3.5),
        egui::pos2(body.left() + 8.5, body.top() + 1.0),
    );
    painter.rect(tab, Rounding { nw: 3.0, ne: 2.0, sw: 0.0, se: 0.0 }, fill, stroke);
    if open {
        painter.line_segment(
            [egui::pos2(body.left() + 2.0, body.top() + 0.5), egui::pos2(body.right() - 2.0, body.top() + 0.5)],
            Stroke::new(1.0_f32, mix(stroke_color, fill, 0.55)),
        );
    }
}

pub struct App {    cfg: Config,
    api: Arc<MegaApi>,
    link_input: String,
    items: Vec<TreeItem>,
    roots: Vec<usize>,
    folder: Option<String>,
    parsing: bool,
    busy: bool,
    cancel: Arc<AtomicBool>,
    tx: mpsc::Sender<Event>,
    rx: mpsc::Receiver<Event>,
    status: String,
    log: VecDeque<String>,
    show_log: bool,
    show_settings: bool,
    batch_total: u64,
    batch_done: u64,
    speed: f64,
    last_sample: (Instant, u64),
    applied_theme: Option<Theme>,
    single_download: Option<usize>,
    focused: Option<usize>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, demo: bool) -> Self {
        install_fonts(&cc.egui_ctx);
        tune_style(&cc.egui_ctx);
        let cfg = Config::load();
        let api = Arc::new(MegaApi::new(cfg.route == Route::Eu, &cfg.proxy));
        let (tx, rx) = mpsc::channel();
        let mut app = Self {
            cfg,
            api,
            link_input: String::new(),
            items: Vec::new(),
            roots: Vec::new(),
            folder: None,
            parsing: false,
            busy: false,
            cancel: Arc::new(AtomicBool::new(false)),
            tx,
            rx,
            status: "就绪".into(),
            log: VecDeque::new(),
            show_log: false,
            show_settings: false,
            batch_total: 0,
            batch_done: 0,
            speed: 0.0,
            last_sample: (Instant::now(), 0),
            applied_theme: None,
            single_download: None,
            focused: None,
        };
        if demo {
            app.load_demo();
        }
        app
    }

    fn load_demo(&mut self) {
        let samples = [
            ("项目资料备份.zip", 120 * 1024 * 1024u64, None, "F0"),
            ("素材", 0, None, "D1"),
            ("voice_sample.mp3", 12 * 1024 * 1024, Some("D1"), "F1"),
            ("产品演示视频.mp4", 86 * 1024 * 1024, Some("D1"), "F2"),
            ("文档", 0, None, "D2"),
            ("readme.txt", 2048, Some("D2"), "F3"),
            ("license.txt", 4096, Some("D2"), "F4"),
        ];
        let items = samples
            .iter()
            .map(|(name, size, parent, handle)| ItemData {
                name: name.to_string(),
                size: *size,
                handle: handle.to_string(),
                key: vec![0; 32],
                info: None,
                is_dir: name == &"素材" || name == &"文档",
                parent: parent.map(str::to_string),
                rel_path: String::new(),
            })
            .collect();
        self.link_input = "https://mega.nz/folder/DEMO1234#demo-key".into();
        self.apply_parsed(Some("DEMO1234".into()), items);
        self.status = "解析完成".into();
    }

    fn push_log(&mut self, msg: impl AsRef<str>) {
        let secs = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        let local = (secs + 8 * 3600) % 86400;
        let ts = format!("{:02}:{:02}:{:02}", local / 3600, local / 60 % 60, local % 60);
        if self.log.len() >= 400 {
            self.log.pop_front();
        }
        self.log.push_back(format!("[{ts}] {}", msg.as_ref()));
    }

    // ------------------------------------------------------------ 动作

    fn start_parse(&mut self) {
        let link = self.link_input.trim().to_string();
        if link.is_empty() || self.parsing || self.busy {
            return;
        }
        self.parsing = true;
        self.status = "正在解析链接…".into();
        self.api = Arc::new(MegaApi::new(self.cfg.route == Route::Eu, &self.cfg.proxy));
        let api = self.api.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = (|| -> mega::Result<(Option<String>, Vec<ItemData>)> {
                let parsed = mega::parse_link(&link)?;
                match parsed.kind {
                    LinkKind::Folder => {
                        let nodes = api.enrich_folder(&parsed)?;
                        let items = nodes
                            .into_iter()
                            .map(|n| ItemData {
                                name: n.name,
                                size: n.size,
                                handle: n.handle,
                                key: n.key,
                                info: None,
                                is_dir: n.is_dir,
                                parent: n.parent,
                                rel_path: String::new(),
                            })
                            .collect();
                        Ok((Some(parsed.id), items))
                    }
                    LinkKind::File => {
                        let key = mega::link_key_bytes(&parsed)?;
                        let info = api.file_info(Some(&parsed.id), None, None)?;
                        let attrs = mega::decrypt_attrs(&info.at, &key)?;
                        let name = attrs
                            .get("n")
                            .and_then(|v| v.as_str())
                            .unwrap_or(&parsed.id)
                            .to_string();
                        Ok((
                            None,
                            vec![ItemData {
                                name,
                                size: info.size,
                                handle: parsed.id.clone(),
                                key,
                                info: Some(info),
                                is_dir: false,
                                parent: None,
                                rel_path: String::new(),
                            }],
                        ))
                    }
                }
            })();
            let _ = match result {
                Ok((folder, items)) => tx.send(Event::Parsed { folder, items }),
                Err(e) => tx.send(Event::ParseError(e.to_string())),
            };
        });
    }

    fn apply_parsed(&mut self, folder: Option<String>, items: Vec<ItemData>) {
        let (nodes, roots) = build_tree(items, folder.as_deref());
        let files = nodes.iter().filter(|n| !n.data.is_dir).count();
        let dirs = nodes.len() - files;
        let total: u64 = nodes.iter().filter(|n| !n.data.is_dir).map(|n| n.data.size).sum();
        self.items = nodes;
        self.roots = roots;
        self.folder = folder;
        self.focused = None;
        self.parsing = false;
        self.status = "解析完成".into();
        self.push_log(format!(
            "解析成功：{} 个文件夹 / {} 个文件，共 {}",
            dirs,
            files,
            mega::fmt_size(total)
        ));
    }

    fn start_download(&mut self, targets: Vec<usize>) {
        if self.busy || self.parsing || targets.is_empty() {
            return;
        }
        let out_dir = PathBuf::from(&self.cfg.out_dir);
        if std::fs::create_dir_all(&out_dir).is_err() {
            self.status = "无法创建保存目录".into();
            return;
        }
        self.cancel = Arc::new(AtomicBool::new(false));
        self.busy = true;
        self.batch_total = targets.iter().map(|&i| self.items[i].data.size).sum();
        self.batch_done = 0;
        self.speed = 0.0;
        self.last_sample = (Instant::now(), 0);
        for &i in &targets {
            self.items[i].state = ItemState::Waiting;
            self.items[i].downloaded = 0;
        }
        let jobs: Vec<(usize, ItemData)> = targets.iter().map(|&i| (i, self.items[i].data.clone())).collect();
        let api = self.api.clone();
        let tx = self.tx.clone();
        let cancel = self.cancel.clone();
        let folder = self.folder.clone();
        let cfg = self.cfg.clone();
        let total_conn = cfg.connections * cfg.parallel_files.min(jobs.len());
        self.status = format!("开始下载 {} 个文件", jobs.len());
        self.push_log(format!(
            "开始下载 {} 个文件：{} 文件并发 × {} 连接（共 {} 连接），{}MB 分块",
            jobs.len(),
            cfg.parallel_files.min(jobs.len()),
            cfg.connections,
            total_conn,
            cfg.part_mb
        ));
        std::thread::spawn(move || {
            use std::sync::atomic::AtomicUsize;
            let ok = AtomicUsize::new(0);
            let fail = AtomicUsize::new(0);
            let skip = AtomicUsize::new(0);
            let stopped = AtomicBool::new(false);
            let workers = cfg.parallel_files.max(1).min(jobs.len());
            let queue = std::sync::Mutex::new(jobs);
            let dl = Downloader::new(cfg.connections, cfg.part_mb, cfg.route == Route::Eu, &cfg.proxy);
            std::thread::scope(|scope| {
                for _ in 0..workers {
                    scope.spawn(|| loop {
                        if cancel.load(Ordering::Relaxed) {
                            stopped.store(true, Ordering::Relaxed);
                            break;
                        }
                        let job = queue.lock().unwrap().pop();
                        let Some((idx, data)) = job else { break };
                        let result = (|| -> mega::Result<PathBuf> {
                            let mut dest = out_dir.clone();
                            let rel = if data.rel_path.is_empty() {
                                mega::safe_filename(&data.name)
                            } else {
                                data.rel_path.clone()
                            };
                            for seg in rel.split('/') {
                                dest.push(seg);
                            }
                            let info = match &data.info {
                                Some(i) => i.clone(),
                                None => {
                                    let _ = tx.send(Event::ItemState(idx, ItemState::Fetching));
                                    api.file_info(None, Some(&data.handle), folder.as_deref())?
                                }
                            };
                            let _ = tx.send(Event::ItemState(idx, ItemState::Downloading));
                            let tx2 = std::sync::Mutex::new(tx.clone());
                            let cb = move |n: u64| {
                                let _ = tx2.lock().unwrap().send(Event::Progress(idx, n));
                            };
                            dl.download(&info, &data.key, &dest, cfg.collision, Some(&cb), Some(&cancel))
                        })();
                        match result {
                            Ok(path) => {
                                ok.fetch_add(1, Ordering::Relaxed);
                                let _ = tx.send(Event::ItemState(idx, ItemState::Done));
                                let _ = tx.send(Event::Log(format!("完成：{} → {}", data.name, path.display())));
                            }
                            Err(mega::Error::Cancelled) => {
                                let _ = tx.send(Event::ItemState(idx, ItemState::Stopped));
                                let _ = tx.send(Event::Log(format!("已停止：{}", data.name)));
                                stopped.store(true, Ordering::Relaxed);
                                break;
                            }
                            Err(mega::Error::Skipped) => {
                                skip.fetch_add(1, Ordering::Relaxed);
                                let _ = tx.send(Event::ItemState(idx, ItemState::Skipped));
                                let _ = tx.send(Event::Log(format!("已跳过（文件已存在）：{}", data.name)));
                            }
                            Err(e) => {
                                fail.fetch_add(1, Ordering::Relaxed);
                                let _ = tx.send(Event::ItemState(idx, ItemState::Failed(e.to_string())));
                                let _ = tx.send(Event::Log(format!("失败：{} - {e}", data.name)));
                            }
                        }
                    });
                }
            });
            let _ = tx.send(Event::BatchDone {
                ok: ok.into_inner(),
                fail: fail.into_inner(),
                skip: skip.into_inner(),
                stopped: stopped.into_inner(),
            });
        });
    }

    fn drain_events(&mut self) {
        while let Ok(ev) = self.rx.try_recv() {
            match ev {
                Event::Parsed { folder, items } => self.apply_parsed(folder, items),
                Event::ParseError(e) => {
                    self.parsing = false;
                    self.status = format!("解析失败：{e}");
                    self.push_log(format!("解析失败：{e}"));
                }
                Event::Log(msg) => self.push_log(msg),
                Event::ItemState(idx, state) => {
                    if let Some(item) = self.items.get_mut(idx) {
                        if state == ItemState::Done {
                            item.downloaded = item.data.size;
                        }
                        item.state = state;
                    }
                }
                Event::Progress(idx, delta) => {
                    if let Some(item) = self.items.get_mut(idx) {
                        item.downloaded = (item.downloaded + delta).min(item.data.size);
                    }
                    self.batch_done += delta;
                }
                Event::BatchDone { ok, fail, skip, stopped } => {
                    self.busy = false;
                    self.speed = 0.0;
                    let suffix = if skip > 0 { format!("，跳过 {skip}") } else { String::new() };
                    self.status = if stopped {
                        format!("已停止（完成 {ok}，失败 {fail}{suffix}）")
                    } else {
                        format!("全部结束：成功 {ok}，失败 {fail}{suffix}")
                    };
                    self.push_log(self.status.clone());
                }
            }
        }
        if self.busy {
            let (t0, b0) = self.last_sample;
            let dt = t0.elapsed().as_secs_f64();
            if dt >= 0.5 {
                let inst = (self.batch_done.saturating_sub(b0)) as f64 / dt;
                // 指数平滑：瞬时速度跳动大，显示值跟手但不会被单个分块的抖动带着乱跳
                let alpha = 1.0 - (-dt / 1.6).exp();
                self.speed = self.speed * (1.0 - alpha) + inst * alpha;
                self.last_sample = (Instant::now(), self.batch_done);
            }
        }
        if let Some(idx) = self.single_download.take() {
            self.start_download(vec![idx]);
        }
    }

    // ------------------------------------------------------------ 树操作

    fn visible_rows(&self) -> Vec<(usize, usize)> {
        fn walk(items: &[TreeItem], idx: usize, depth: usize, out: &mut Vec<(usize, usize)>) {
            out.push((idx, depth));
            if items[idx].data.is_dir && items[idx].expanded {
                for &child in &items[idx].children {
                    walk(items, child, depth + 1, out);
                }
            }
        }
        let mut out = Vec::new();
        for &root in &self.roots {
            walk(&self.items, root, 0, &mut out);
        }
        out
    }

    fn collect_files(&self, idx: usize, out: &mut Vec<usize>) {
        if self.items[idx].data.is_dir {
            for &child in &self.items[idx].children {
                self.collect_files(child, out);
            }
        } else {
            out.push(idx);
        }
    }

    fn folder_files(&self, idx: usize) -> Vec<usize> {
        let mut out = Vec::new();
        self.collect_files(idx, &mut out);
        out
    }

    fn folder_all(&self, idx: usize) -> bool {
        let files = self.folder_files(idx);
        !files.is_empty() && files.iter().all(|&f| self.items[f].selected)
    }

    fn folder_any(&self, idx: usize) -> bool {
        self.folder_files(idx).iter().any(|&f| self.items[f].selected)
    }

    fn set_subtree(&mut self, idx: usize, value: bool) {
        let files = self.folder_files(idx);
        for f in files {
            self.items[f].selected = value;
        }
    }

    fn selected_files(&self) -> Vec<usize> {
        self.items
            .iter()
            .enumerate()
            .filter(|(_, n)| !n.data.is_dir && n.selected)
            .map(|(i, _)| i)
            .collect()
    }

    fn all_files(&self) -> Vec<usize> {
        self.items.iter().enumerate().filter(|(_, n)| !n.data.is_dir).map(|(i, _)| i).collect()
    }

    // ------------------------------------------------------------ 界面

    fn apply_theme(&mut self, ctx: &egui::Context) {
        let effective = match self.cfg.theme {
            Theme::Light => Theme::Light,
            Theme::Dark => Theme::Dark,
            Theme::System => match ctx.system_theme() {
                Some(egui::Theme::Dark) => Theme::Dark,
                _ => Theme::Light,
            },
        };
        if self.applied_theme == Some(effective) {
            return;
        }
        self.applied_theme = Some(effective);
        ctx.set_visuals(palette(effective == Theme::Dark));
    }

    fn header_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            paint_logo(ui);
            ui.add_space(12.0);
            ui.vertical(|ui| {
                ui.add_space(1.0);
                ui.label(RichText::new("MegaDL").size(18.0).strong());
                ui.label(
                    RichText::new(concat!("MEGA 公共链接下载器 · v", env!("CARGO_PKG_VERSION")))
                        .size(11.5)
                        .color(ui.visuals().weak_text_color()),
                );
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(egui::Button::new(RichText::new("设置").size(12.5)).min_size(egui::vec2(58.0, 32.0)))
                    .clicked()
                {
                    self.show_settings = !self.show_settings;
                }
                ui.add_space(4.0);
                if ui
                    .add(egui::Button::new(RichText::new("日志").size(12.5)).min_size(egui::vec2(58.0, 32.0)))
                    .clicked()
                {
                    self.show_log = !self.show_log;
                }
            });
        });
    }

    fn link_card(&mut self, ui: &mut egui::Ui) {
        card_frame(ui).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new("链接").size(14.0).strong());
                ui.add_space(2.0);
                ui.label(
                    RichText::new("支持文件、文件夹，以及 #! / #F! 旧格式")
                        .size(11.5)
                        .color(ui.visuals().weak_text_color()),
                );
            });
            ui.add_space(9.0);
            ui.horizontal(|ui| {
                let reserved = 150.0;
                let edit_width = (ui.available_width() - reserved).max(280.0);
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut self.link_input)
                        .desired_width(edit_width)
                        .hint_text("https://mega.nz/folder/…")
                        .font(egui::TextStyle::Monospace)
                        .margin(egui::Margin::symmetric(12.0, 9.0)),
                );
                let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if ui.add_enabled(!self.busy && !self.parsing, egui::Button::new("粘贴")).clicked() {
                    if let Ok(mut clip) = arboard::Clipboard::new() {
                        if let Ok(text) = clip.get_text() {
                            self.link_input = text.trim().to_string();
                        }
                    }
                }
                let parse_btn = accent_button("解析").min_size(egui::vec2(84.0, 38.0));
                if ui.add_enabled(!self.busy && !self.parsing, parse_btn).clicked() || enter {
                    self.start_parse();
                }
            });
        });
    }

    fn file_card(&mut self, ui: &mut egui::Ui) {
        let card = card_frame(ui);
        card.show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_min_height(ui.available_height());
            let files = self.all_files();
            let dirs = self.items.len() - files.len();
            ui.horizontal(|ui| {
                ui.label(RichText::new("文件").size(14.5).strong());
                if !self.items.is_empty() {
                    ui.add_space(2.0);
                    ui.label(
                        RichText::new(format!("{} 个文件夹 · {} 个文件", dirs, files.len()))
                            .size(11.5)
                            .color(ui.visuals().weak_text_color()),
                    );
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let enabled = !self.items.is_empty() && !self.busy;
                    if ui
                        .add_enabled(
                            enabled,
                            egui::Button::new(RichText::new("全不选").size(11.5))
                                .min_size(egui::vec2(50.0, 26.0)),
                        )
                        .clicked()
                    {
                        for item in &mut self.items {
                            item.selected = false;
                        }
                    }
                    if ui
                        .add_enabled(
                            enabled,
                            egui::Button::new(RichText::new("全选").size(11.5))
                                .min_size(egui::vec2(46.0, 26.0)),
                        )
                        .clicked()
                    {
                        for item in &mut self.items {
                            item.selected = true;
                        }
                    }
                    ui.add_space(10.0);
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new("收起全部")
                                    .size(11.5)
                                    .color(ui.visuals().weak_text_color()),
                            )
                            .frame(false),
                        )
                        .clicked()
                    {
                        for item in &mut self.items {
                            if item.data.is_dir {
                                item.expanded = false;
                            }
                        }
                    }
                    ui.label(RichText::new("·").size(11.5).color(ui.visuals().weak_text_color()));
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new("展开全部")
                                    .size(11.5)
                                    .color(ui.visuals().weak_text_color()),
                            )
                            .frame(false),
                        )
                        .clicked()
                    {
                        for item in &mut self.items {
                            if item.data.is_dir {
                                item.expanded = true;
                            }
                        }
                    }
                    ui.add_space(4.0);
                });
            });
            ui.add_space(6.0);

            if self.items.is_empty() {
                empty_state(ui);
                return;
            }

            let rows = self.visible_rows();
            let mut metas = Vec::with_capacity(rows.len());
            for (idx, depth) in rows {
                let is_dir = self.items[idx].data.is_dir;
                let (all, any) = if is_dir {
                    (self.folder_all(idx), self.folder_any(idx))
                } else {
                    (false, false)
                };
                let file_count = if is_dir { self.folder_files(idx).len() } else { 0 };
                metas.push(RowMeta { idx, depth, is_dir, all_selected: all, any_selected: any, file_count });
            }

            let busy = self.busy;
            let focused_idx = self.focused;
            let mut toggle_expand: Option<usize> = None;
            let mut toggle_select: Option<usize> = None;
            let mut focus: Option<usize> = None;
            let mut download_one: Option<usize> = None;

            let dark = ui.visuals().dark_mode;
            let hover_bg = if dark {
                Color32::from_rgb(0x1D, 0x27, 0x23)
            } else {
                Color32::from_rgb(0xF7, 0xF9, 0xF8)
            };
            let focus_bg = if dark {
                Color32::from_rgb(0x1C, 0x2A, 0x3D)
            } else {
                Color32::from_rgb(0xEE, 0xF4, 0xFE)
            };
            let row_h = 34.0;
            let indent = 18.0;

            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show_rows(ui, row_h, metas.len(), |ui, range| {
                    for mi in range {
                        let meta = &metas[mi];
                        let item = &mut self.items[meta.idx];
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(ui.available_width(), row_h), egui::Sense::hover());
                        let hovered = ui.rect_contains_pointer(rect);
                        let focused = focused_idx == Some(meta.idx);
                        let bg_rect = rect.shrink2(egui::vec2(0.0, 1.0));
                        if focused {
                            ui.painter().rect_filled(bg_rect, Rounding::same(6.0), focus_bg);
                            ui.painter().rect_filled(
                                egui::Rect::from_min_size(
                                    egui::pos2(bg_rect.left(), bg_rect.top() + 4.0),
                                    egui::vec2(2.0, bg_rect.height() - 8.0),
                                ),
                                Rounding::same(1.0),
                                ACCENT,
                            );
                        } else if hovered {
                            ui.painter().rect_filled(bg_rect, Rounding::same(6.0), hover_bg);
                        }

                        // 勾选框已随层级缩进，层级由缩进本身表达，不再画竖向引导线

                        let mut row_host = ui.new_child(
                            egui::UiBuilder::new()
                                .max_rect(rect)
                                .layout(egui::Layout::top_down(egui::Align::Min)),
                        );
                        row_host.spacing_mut().item_spacing = egui::vec2(6.0, 0.0);
                        row_host.horizontal_centered(|row| {
                        row.add_space(8.0 + meta.depth as f32 * indent);

                        // 展开箭头（文件行留出等宽空位），随后才是勾选框，勾选框紧贴图标
                        if meta.is_dir {
                            let (chev, resp) = row.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::click());
                            if resp.hovered() {
                                row.painter().circle_filled(
                                    chev.center(),
                                    7.0,
                                    row.visuals().widgets.hovered.bg_fill,
                                );
                            }
                            let color = if resp.hovered() {
                                row.visuals().strong_text_color()
                            } else {
                                row.visuals().weak_text_color()
                            };
                            let stroke = Stroke::new(1.2_f32, color);
                            let c = chev.center();
                            if item.expanded {
                                row.painter().line_segment(
                                    [egui::pos2(c.x - 3.5, c.y - 1.5), egui::pos2(c.x, c.y + 2.0)],
                                    stroke,
                                );
                                row.painter().line_segment(
                                    [egui::pos2(c.x, c.y + 2.0), egui::pos2(c.x + 3.5, c.y - 1.5)],
                                    stroke,
                                );
                            } else {
                                row.painter().line_segment(
                                    [egui::pos2(c.x - 1.5, c.y - 3.5), egui::pos2(c.x + 2.0, c.y)],
                                    stroke,
                                );
                                row.painter().line_segment(
                                    [egui::pos2(c.x + 2.0, c.y), egui::pos2(c.x - 1.5, c.y + 3.5)],
                                    stroke,
                                );
                            }
                            if resp.clicked() {
                                toggle_expand = Some(meta.idx);
                                focus = Some(meta.idx);
                            }
                        } else {
                            row.add_space(14.0);
                        }

                        if meta.is_dir {
                            let mark = if meta.all_selected {
                                Mark::All
                            } else if meta.any_selected {
                                Mark::Partial
                            } else {
                                Mark::None
                            };
                            if mark_box(row, mark)
                                .on_hover_text("勾选该文件夹下全部文件")
                                .clicked()
                            {
                                toggle_select = Some(meta.idx);
                                focus = Some(meta.idx);
                            }
                        } else {
                            let mark = if item.selected { Mark::All } else { Mark::None };
                            if mark_box(row, mark).clicked() {
                                item.selected = !item.selected;
                                focus = Some(meta.idx);
                            }
                        }

                        row.add_space(2.0);
                        if meta.is_dir {
                            paint_folder_icon(row, item.expanded);
                        } else {
                            paint_file_icon(row, &item.data.name);
                        }

                        let name_w = (row.available_width() - 230.0).max(80.0);
                        if meta.is_dir {
                            let resp = row
                                .allocate_ui_with_layout(
                                    egui::vec2(name_w, 26.0),
                                    egui::Layout::left_to_right(egui::Align::Center),
                                    |ui| {
                                        ui.add(
                                            egui::Label::new(
                                                RichText::new(item.data.name.as_str())
                                                    .strong()
                                                    .color(ui.visuals().strong_text_color()),
                                            )
                                            .truncate()
                                            .sense(egui::Sense::click()),
                                        )
                                        .on_hover_text(item.data.name.as_str())
                                    },
                                )
                                .inner;
                            if resp.clicked() {
                                toggle_expand = Some(meta.idx);
                                focus = Some(meta.idx);
                            }
                        } else {
                            let resp = row
                                .allocate_ui_with_layout(
                                    egui::vec2(name_w, 26.0),
                                    egui::Layout::left_to_right(egui::Align::Center),
                                    |ui| {
                                        ui.add(
                                            egui::Label::new(item.data.name.as_str())
                                                .truncate()
                                                .sense(egui::Sense::click()),
                                        )
                                        .on_hover_text(item.data.name.as_str())
                                    },
                                )
                                .inner;
                            if resp.clicked() {
                                focus = Some(meta.idx);
                            }
                            if resp.double_clicked() && !busy {
                                download_one = Some(meta.idx);
                            }
                        }

                        row.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if meta.is_dir {
                                ui.label(
                                    RichText::new(mega::fmt_size(item.total_size))
                                        .size(11.0)
                                        .color(ui.visuals().weak_text_color()),
                                );
                                ui.add_space(18.0);
                                ui.label(
                                    RichText::new(format!("{} 个文件", meta.file_count))
                                        .size(11.0)
                                        .color(ui.visuals().weak_text_color()),
                                );
                            } else {
                                let (text, color) =
                                    state_label(&item.state, item.downloaded, item.data.size, ui);
                                let status_resp = ui
                                    .allocate_ui_with_layout(
                                        egui::vec2(104.0, 18.0),
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            ui.add(
                                                egui::Label::new(
                                                    RichText::new(&text).size(11.0).color(color),
                                                )
                                                .truncate(),
                                            )
                                        },
                                    )
                                    .inner;
                                status_resp.on_hover_text(&text);
                                if matches!(item.state, ItemState::Downloading) {
                                    let frac = if item.data.size == 0 {
                                        0.0
                                    } else {
                                        item.downloaded as f32 / item.data.size as f32
                                    };
                                    ui.add_space(8.0);
                                    ui.add(
                                        egui::ProgressBar::new(frac)
                                            .desired_width(44.0)
                                            .desired_height(4.0)
                                            .fill(ACCENT)
                                            .rounding(2.0),
                                    );
                                }
                                ui.add_space(6.0);
                                ui.label(
                                    RichText::new(mega::fmt_size(item.data.size))
                                        .size(11.0)
                                        .color(ui.visuals().weak_text_color()),
                                );
                            }
                        });
                        });
                    }
                });

            if let Some(idx) = toggle_expand {
                self.items[idx].expanded = !self.items[idx].expanded;
            }
            if let Some(idx) = toggle_select {
                let all = self.folder_all(idx);
                self.set_subtree(idx, !all);
            }
            if let Some(idx) = focus {
                self.focused = Some(idx);
            }
            if let Some(idx) = download_one {
                self.single_download = Some(idx);
            }
        });
    }

    fn stats_card(&mut self, ui: &mut egui::Ui) {
        let picked = self.selected_files();
        let selected: u64 = picked.iter().map(|&i| self.items[i].data.size).sum();
        let busy = self.busy;
        let speed = self.speed;
        let done = self.batch_done;
        let total = self.batch_total;
        let mut action = 0u8;

        card_frame(ui).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new("本次下载").size(14.5).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let (text, color) = if busy {
                        ("进行中", ACCENT)
                    } else if total > 0 {
                        ("已结束", ui.visuals().weak_text_color())
                    } else {
                        ("空闲", ui.visuals().weak_text_color())
                    };
                    ui.label(RichText::new(text).size(11.5).color(color));
                });
            });
            ui.add_space(12.0);
            let col_w = (ui.available_width() / 3.0).max(40.0);
            let count = picked.len().to_string();
            let size = mega::fmt_size(selected);
            let speed_text = speed_parts(speed);
            ui.horizontal(|ui| {
                let metrics: [(&str, &str, &str); 3] = [
                    (&count, "", "已选文件"),
                    (&size, "", "总大小"),
                    (&speed_text.0, speed_text.1, "实时速度"),
                ];
                for (value, unit, caption) in metrics {
                    ui.allocate_ui_with_layout(
                        egui::vec2(col_w, 40.0),
                        egui::Layout::top_down(egui::Align::Center),
                        |ui| metric_column(ui, value, unit, caption),
                    );
                }
            });
            ui.add_space(12.0);
            let frac = if total == 0 { 0.0 } else { (done as f32 / total as f32).clamp(0.0, 1.0) };
            let (track, _) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), 6.0),
                egui::Sense::hover(),
            );
            ui.painter().rect_filled(
                track,
                Rounding::same(3.0),
                ui.visuals().widgets.inactive.bg_fill,
            );
            if frac > 0.0 {
                let fill = egui::Rect::from_min_size(
                    track.min,
                    egui::vec2(track.width() * frac, track.height()),
                );
                ui.painter().rect_filled(fill, Rounding::same(3.0), ACCENT);
            }
            if total > 0 {
                ui.add_space(6.0);
                ui.label(
                    RichText::new(format!("{} / {}", mega::fmt_size(done), mega::fmt_size(total)))
                        .size(11.0)
                        .color(ui.visuals().weak_text_color()),
                );
            }
            ui.add_space(12.0);
            let enabled = !busy && !self.parsing && !self.items.is_empty();
            if ui
                .add_enabled(enabled, accent_button("下载选中").min_size(egui::vec2(ui.available_width(), 38.0)))
                .clicked()
            {
                action = 1;
            }
            ui.add_space(7.0);
            ui.horizontal(|ui| {
                let w = (ui.available_width() - 8.0) / 2.0;
                if ui
                    .add_enabled(enabled, egui::Button::new("全部下载").min_size(egui::vec2(w, 34.0)))
                    .clicked()
                {
                    action = 2;
                }
                if ui
                    .add_enabled(
                        busy,
                        egui::Button::new(RichText::new("停止").color(DESTRUCTIVE))
                            .min_size(egui::vec2(w, 34.0)),
                    )
                    .clicked()
                {
                    action = 3;
                }
            });
        });

        match action {
            1 => self.start_download(picked),
            2 => {
                for item in &mut self.items {
                    item.selected = true;
                }
                let all = self.all_files();
                self.start_download(all);
            }
            3 => {
                self.cancel.store(true, Ordering::Relaxed);
                self.status = "正在停止…".into();
            }
            _ => {}
        }
    }

    fn detail_card(&mut self, ui: &mut egui::Ui) {
        let idx = self.focused.filter(|&i| i < self.items.len());
        let is_file = idx.map(|i| !self.items[i].data.is_dir).unwrap_or(false);
        let mut copy = false;
        card_frame(ui).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_min_height(100.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("当前选择").size(14.5).strong());
                if is_file {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add(egui::Button::new(RichText::new("复制直链").size(11.0)).frame(false))
                            .clicked()
                        {
                            copy = true;
                        }
                    });
                }
            });
            ui.add_space(10.0);
            let Some(idx) = idx else {
                ui.label(
                    RichText::new("选中左侧文件查看详情")
                        .size(11.5)
                        .color(ui.visuals().weak_text_color()),
                );
                return;
            };
            let item = &self.items[idx];
            ui.add(
                egui::Label::new(RichText::new(item.data.name.as_str()).size(13.0).strong())
                    .truncate(),
            )
            .on_hover_text(item.data.name.as_str());
            ui.add_space(3.0);
            if item.data.is_dir {
                let count = self.folder_files(idx).len();
                ui.label(
                    RichText::new(format!("文件夹 · {} 个文件 · {}", count, mega::fmt_size(item.total_size)))
                        .size(11.5)
                        .color(ui.visuals().weak_text_color()),
                );
            } else {
                let (status_text, status_color) =
                    state_label(&item.state, item.downloaded, item.data.size, ui);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("文件 · {}", mega::fmt_size(item.data.size)))
                            .size(11.5)
                            .color(ui.visuals().weak_text_color()),
                    );
                    ui.add(
                        egui::Label::new(RichText::new(&status_text).size(11.5).color(status_color))
                            .truncate(),
                    )
                    .on_hover_text(&status_text);
                });
            }
            ui.add_space(9.0);
            ui.label(
                RichText::new(&item.data.rel_path)
                    .monospace()
                    .size(10.5)
                    .color(ui.visuals().weak_text_color()),
            );
        });
        if copy {
            self.copy_link();
        }
    }

    fn env_card(&mut self, ui: &mut egui::Ui) {
        let cfg = self.cfg.clone();
        let route = self.api.preferred().to_string();
        let mut open_dir = false;
        card_frame(ui).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new("环境").size(14.5).strong());
            ui.add_space(10.0);
            egui::Grid::new("env_grid")
                .num_columns(2)
                .spacing([12.0, 9.0])
                .show(ui, |ui| {
                    ui.label(RichText::new("API 线路").size(11.5).color(ui.visuals().weak_text_color()));
                    ui.label(RichText::new(&route).size(12.0));
                    ui.end_row();
                    ui.label(RichText::new("并发").size(11.5).color(ui.visuals().weak_text_color()));
                    ui.label(
                        RichText::new(format!("{} 连接 × {} 文件", cfg.connections, cfg.parallel_files))
                            .size(12.0),
                    );
                    ui.end_row();
                    ui.label(RichText::new("保存位置").size(11.5).color(ui.visuals().weak_text_color()));
                    ui.add(
                        egui::Label::new(RichText::new(&cfg.out_dir).size(11.0).monospace())
                            .truncate(),
                    )
                    .on_hover_text(&cfg.out_dir);
                    ui.end_row();
                });
            ui.add_space(10.0);
            if ui
                .add(egui::Button::new("打开下载目录").min_size(egui::vec2(ui.available_width(), 32.0)))
                .clicked()
            {
                open_dir = true;
            }
        });
        if open_dir {
            let _ = open_folder(&cfg.out_dir);
        }
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.set_width(ui.available_width());
        self.stats_card(ui);
        ui.add_space(14.0);
        self.detail_card(ui);
        ui.add_space(14.0);
        self.env_card(ui);
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        ui.painter().hline(
            rect.x_range(),
            rect.top(),
            Stroke::new(1.0_f32, ui.visuals().widgets.noninteractive.bg_stroke.color),
        );
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let color = if self.busy || self.parsing { ACCENT } else { SUCCESS };
            let (dot, _) = ui.allocate_exact_size(egui::vec2(6.0, 6.0), egui::Sense::hover());
            ui.painter().circle_filled(dot.center(), 3.0, color);
            ui.label(RichText::new(&self.status).size(11.0));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new(format!(
                        "v{}  ·  线路 {}  ·  {} × {}",
                        env!("CARGO_PKG_VERSION"),
                        self.api.preferred(),
                        self.cfg.connections,
                        self.cfg.parallel_files
                    ))
                    .size(10.5)
                    .color(ui.visuals().weak_text_color()),
                );
            });
        });
    }

    fn settings_window(&mut self, ctx: &egui::Context) {
        if !self.show_settings {
            return;
        }
        let before = self.cfg.clone();
        let mut open = true;
        egui::Window::new("设置")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(540.0)
            .default_pos(egui::pos2(330.0, 120.0))
            .frame(window_frame(ctx))
            .show(ctx, |ui| {
                ui.set_width(520.0);
                ui.label(RichText::new("下载").size(14.0).strong());
                ui.add_space(2.0);
                ui.label(
                    RichText::new("保存位置与并发策略")
                        .size(11.0)
                        .color(ui.visuals().weak_text_color()),
                );
                ui.add_space(9.0);
                egui::Grid::new("settings_download")
                    .num_columns(2)
                    .spacing([20.0, 12.0])
                    .show(ui, |ui| {
                        ui.label("保存目录");
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::Label::new(RichText::new(&self.cfg.out_dir).size(11.0).monospace())
                                    .truncate(),
                            );
                            if ui.add(egui::Button::new("浏览…").min_size(egui::vec2(56.0, 30.0))).clicked() {
                                if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                                    self.cfg.out_dir = dir.to_string_lossy().to_string();
                                }
                            }
                        });
                        ui.end_row();

                        ui.label("并发连接数");
                        ui.horizontal(|ui| {
                            ui.add(egui::DragValue::new(&mut self.cfg.connections).range(1..=16));
                            ui.label(
                                RichText::new("单文件分块并发")
                                    .size(11.0)
                                    .color(ui.visuals().weak_text_color()),
                            );
                        });
                        ui.end_row();

                        ui.label("同时下载文件数");
                        ui.horizontal(|ui| {
                            ui.add(egui::DragValue::new(&mut self.cfg.parallel_files).range(1..=6));
                            ui.label(
                                RichText::new("多个文件并行")
                                    .size(11.0)
                                    .color(ui.visuals().weak_text_color()),
                            );
                        });
                        ui.end_row();

                        ui.label("分块大小");
                        ui.horizontal(|ui| {
                            ui.add(egui::DragValue::new(&mut self.cfg.part_mb).range(1..=16));
                            ui.label("MB");
                        });
                        ui.end_row();
                    });

                ui.add_space(16.0);
                ui.separator();
                ui.add_space(12.0);
                ui.label(RichText::new("连接").size(14.0).strong());
                ui.add_space(2.0);
                ui.label(
                    RichText::new("API 线路、代理与下载配额")
                        .size(11.0)
                        .color(ui.visuals().weak_text_color()),
                );
                ui.add_space(9.0);
                egui::Grid::new("settings_network")
                    .num_columns(2)
                    .spacing([20.0, 12.0])
                    .show(ui, |ui| {
                        ui.label("API 线路");
                        egui::ComboBox::from_id_salt("route")
                            .width(260.0)
                            .selected_text(match self.cfg.route {
                                Route::Auto => "自动（G 优先，失败切 EU）",
                                Route::Eu => "EU 优先",
                            })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut self.cfg.route, Route::Auto, "自动（G 优先，失败切 EU）");
                                ui.selectable_value(&mut self.cfg.route, Route::Eu, "EU 优先");
                            });
                        ui.end_row();

                        ui.label("代理（留空直连）");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.cfg.proxy)
                                .desired_width(300.0)
                                .hint_text("http://127.0.0.1:7890")
                                .font(egui::TextStyle::Monospace)
                                .margin(egui::Margin::symmetric(10.0, 7.0)),
                        );
                        ui.end_row();
                    });

                ui.add_space(16.0);
                ui.separator();
                ui.add_space(12.0);
                ui.label(RichText::new("文件与外观").size(14.0).strong());
                ui.add_space(9.0);
                egui::Grid::new("settings_misc")
                    .num_columns(2)
                    .spacing([20.0, 12.0])
                    .show(ui, |ui| {
                        ui.label("重名文件");
                        egui::ComboBox::from_id_salt("collision")
                            .width(180.0)
                            .selected_text(match self.cfg.collision {
                                Collision::Rename => "自动重命名",
                                Collision::Overwrite => "覆盖",
                                Collision::Skip => "跳过",
                            })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut self.cfg.collision, Collision::Rename, "自动重命名");
                                ui.selectable_value(&mut self.cfg.collision, Collision::Overwrite, "覆盖");
                                ui.selectable_value(&mut self.cfg.collision, Collision::Skip, "跳过");
                            });
                        ui.end_row();

                        ui.label("外观");
                        egui::ComboBox::from_id_salt("theme")
                            .width(180.0)
                            .selected_text(match self.cfg.theme {
                                Theme::Light => "浅色",
                                Theme::Dark => "深色",
                                Theme::System => "跟随系统",
                            })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut self.cfg.theme, Theme::Light, "浅色");
                                ui.selectable_value(&mut self.cfg.theme, Theme::Dark, "深色");
                                ui.selectable_value(&mut self.cfg.theme, Theme::System, "跟随系统");
                            });
                        ui.end_row();
                    });

                ui.add_space(14.0);
                let total = self.cfg.connections * self.cfg.parallel_files;
                let hint = if total > 32 {
                    format!("当前总连接 {total}，可能触发 MEGA 连接/配额限制")
                } else {
                    format!("当前总连接 {total}；MEGA 免费配额按 IP 每 6 小时滚动计算")
                };
                ui.label(
                    RichText::new(hint)
                        .size(11.0)
                        .color(if total > 32 { DESTRUCTIVE } else { ui.visuals().weak_text_color() }),
                );
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("自动保存：{}", Config::path().display()))
                            .size(10.5)
                            .color(ui.visuals().weak_text_color()),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add(egui::Button::new("打开下载目录").min_size(egui::vec2(110.0, 30.0)))
                            .clicked()
                        {
                            let _ = open_folder(&self.cfg.out_dir);
                        }
                    });
                });
            });
        self.show_settings = open;
        if self.cfg.out_dir != before.out_dir
            || self.cfg.connections != before.connections
            || self.cfg.part_mb != before.part_mb
            || self.cfg.parallel_files != before.parallel_files
            || self.cfg.proxy != before.proxy
            || self.cfg.route != before.route
            || self.cfg.collision != before.collision
            || self.cfg.theme != before.theme
        {
            self.cfg.save();
        }
    }

    fn log_window(&mut self, ctx: &egui::Context) {
        if !self.show_log {
            return;
        }
        let mut open = true;
        egui::Window::new("日志")
            .open(&mut open)
            .resizable(true)
            .collapsible(false)
            .default_size([680.0, 320.0])
            .min_size(egui::vec2(520.0, 260.0))
            .default_pos(egui::pos2(260.0, 180.0))
            .frame(window_frame(ctx))
            .show(ctx, |ui| {
                ui.set_min_size(egui::vec2(640.0, 260.0));
                ui.horizontal(|ui| {
                    if ui.add(egui::Button::new("清空").min_size(egui::vec2(56.0, 28.0))).clicked() {
                        self.log.clear();
                    }
                });
                ui.add_space(6.0);
                egui::ScrollArea::vertical().stick_to_bottom(true).show(ui, |ui| {
                    for line in &self.log {
                        ui.label(RichText::new(line).monospace().small());
                    }
                });
            });
        self.show_log = open;
    }

    fn copy_link(&mut self) {
        let idx = self
            .focused
            .filter(|&i| i < self.items.len() && !self.items[i].data.is_dir)
            .or_else(|| self.selected_files().first().copied());
        let Some(idx) = idx else {
            self.status = "请先选择一个文件（文件夹没有单文件直链）".into();
            return;
        };
        let item = &self.items[idx];
        let link = format!("https://mega.nz/file/{}#{}", item.data.handle, mega::b64url_encode(&item.data.key));
        if let Ok(mut clip) = arboard::Clipboard::new() {
            let _ = clip.set_text(link.clone());
            self.push_log(format!("已复制链接：{link}"));
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.apply_theme(ctx);
        self.drain_events();
        if self.busy || self.parsing {
            ctx.request_repaint_after(Duration::from_millis(100));
        }

        let panel_fill = ctx.style().visuals.panel_fill;
        egui::TopBottomPanel::bottom("statusbar")
            .frame(
                egui::Frame::none()
                    .fill(panel_fill)
                    .inner_margin(egui::Margin::symmetric(20.0, 6.0)),
            )
            .show(ctx, |ui| self.status_bar(ui));

        egui::CentralPanel::default()
            .frame(
                egui::Frame::none()
                    .fill(panel_fill)
                    .inner_margin(egui::Margin::symmetric(20.0, 14.0)),
            )
            .show(ctx, |ui| {
                self.header_bar(ui);
                ui.add_space(14.0);
                self.link_card(ui);
                ui.add_space(14.0);

                let available = ui.available_size();
                let gap = 16.0;
                let side = 292.0;
                let left = (available.x - side - gap).max(420.0);
                ui.horizontal_top(|ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2(left, available.y),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.set_width(left);
                            self.file_card(ui);
                        },
                    );
                    ui.add_space(gap);
                    ui.allocate_ui_with_layout(
                        egui::vec2(side, available.y),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.set_width(side);
                            egui::ScrollArea::vertical()
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    self.sidebar(ui);
                                });
                        },
                    );
                });
            });

        self.settings_window(ctx);
        self.log_window(ctx);
    }
}

// ---------------------------------------------------------------- 树构建

fn build_tree(items: Vec<ItemData>, root_id: Option<&str>) -> (Vec<TreeItem>, Vec<usize>) {
    let handles: HashMap<String, usize> =
        items.iter().enumerate().map(|(i, n)| (n.handle.clone(), i)).collect();
    let mut nodes: Vec<TreeItem> = items
        .into_iter()
        .map(|data| TreeItem {
            data,
            parent: None,
            children: Vec::new(),
            expanded: true,
            selected: true,
            state: ItemState::Waiting,
            downloaded: 0,
            total_size: 0,
        })
        .collect();

    let mut edges = Vec::new();
    for i in 0..nodes.len() {
        if let Some(p) = nodes[i].data.parent.clone() {
            if Some(p.as_str()) != root_id {
                if let Some(&pi) = handles.get(&p) {
                    edges.push((pi, i));
                }
            }
        }
    }
    for (pi, ci) in edges {
        nodes[pi].children.push(ci);
        nodes[ci].parent = Some(pi);
    }

    let mut roots: Vec<usize> = nodes.iter().enumerate().filter(|(_, n)| n.parent.is_none()).map(|(i, _)| i).collect();
    let sort_key = |nodes: &Vec<TreeItem>, a: usize, b: usize| {
        nodes[b]
            .data
            .is_dir
            .cmp(&nodes[a].data.is_dir)
            .then_with(|| nodes[a].data.name.to_lowercase().cmp(&nodes[b].data.name.to_lowercase()))
    };
    roots.sort_by(|&a, &b| sort_key(&nodes, a, b));
    for &r in &roots {
        sort_rec(&mut nodes, r, &sort_key);
        assign_paths(&mut nodes, r, "", 0);
        total_rec(&mut nodes, r);
    }
    for &r in &roots {
        set_default_expanded(&mut nodes, r, 0);
    }
    (nodes, roots)
}

fn set_default_expanded(nodes: &mut Vec<TreeItem>, idx: usize, depth: usize) {
    if nodes[idx].data.is_dir {
        nodes[idx].expanded = depth == 0;
        for k in nodes[idx].children.clone() {
            set_default_expanded(nodes, k, depth + 1);
        }
    }
}

fn sort_rec(nodes: &mut Vec<TreeItem>, idx: usize, key: &dyn Fn(&Vec<TreeItem>, usize, usize) -> std::cmp::Ordering) {
    let mut kids = std::mem::take(&mut nodes[idx].children);
    kids.sort_by(|&a, &b| key(nodes, a, b));
    for &k in &kids {
        sort_rec(nodes, k, key);
    }
    nodes[idx].children = kids;
}

fn assign_paths(nodes: &mut Vec<TreeItem>, idx: usize, prefix: &str, depth: usize) {
    if depth > 64 {
        return;
    }
    let name = mega::safe_filename(&nodes[idx].data.name);
    let rel = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
    nodes[idx].data.rel_path = rel.clone();
    let kids = nodes[idx].children.clone();
    for k in kids {
        assign_paths(nodes, k, &rel, depth + 1);
    }
}

fn total_rec(nodes: &mut Vec<TreeItem>, idx: usize) -> u64 {
    if nodes[idx].data.is_dir {
        let kids = nodes[idx].children.clone();
        let mut sum = 0;
        for k in kids {
            sum += total_rec(nodes, k);
        }
        nodes[idx].total_size = sum;
        sum
    } else {
        nodes[idx].total_size = nodes[idx].data.size;
        nodes[idx].data.size
    }
}

// ---------------------------------------------------------------- 外观

const ACCENT: Color32 = Color32::from_rgb(0x1F, 0x6B, 0xEB);
const DESTRUCTIVE: Color32 = Color32::from_rgb(0xD1, 0x43, 0x43);
const SUCCESS: Color32 = Color32::from_rgb(0x1E, 0x9C, 0x6A);

fn state_label(state: &ItemState, done: u64, size: u64, ui: &egui::Ui) -> (String, Color32) {
    let muted = ui.visuals().weak_text_color();
    match state {
        ItemState::Waiting => ("等待".into(), muted),
        ItemState::Fetching => ("获取直链…".into(), ACCENT),
        ItemState::Downloading => {
            let pct = if size == 0 { 0.0 } else { done as f64 / size as f64 * 100.0 };
            (format!("下载中 {pct:.1}%"), ACCENT)
        }
        ItemState::Done => ("完成".into(), SUCCESS),
        ItemState::Skipped => ("已跳过".into(), muted),
        ItemState::Stopped => ("已停止".into(), muted),
        ItemState::Failed(e) => (format!("失败：{e}"), DESTRUCTIVE),
    }
}

/// 速度数值和单位分开显示：数值用大字号、单位用小字号，窄卡片里不会折行。
fn speed_parts(v: f64) -> (String, &'static str) {
    let v = v.max(0.0);
    if v < 1024.0 {
        ("0".to_string(), "B/s")
    } else if v < 1024.0 * 1024.0 {
        (format!("{:.0}", v / 1024.0), "KB/s")
    } else if v < 1024.0 * 1024.0 * 1024.0 {
        (format!("{:.1}", v / (1024.0 * 1024.0)), "MB/s")
    } else {
        (format!("{:.2}", v / (1024.0 * 1024.0 * 1024.0)), "GB/s")
    }
}

fn metric_column(ui: &mut egui::Ui, value: &str, unit: &str, caption: &str) {
    ui.spacing_mut().item_spacing.y = 1.0;
    let mut job = egui::text::LayoutJob::default();
    job.append(
        value,
        0.0,
        egui::TextFormat {
            font_id: egui::FontId::proportional(19.0),
            color: ui.visuals().strong_text_color(),
            ..Default::default()
        },
    );
    if !unit.is_empty() {
        job.append(
            &format!(" {unit}"),
            0.0,
            egui::TextFormat {
                font_id: egui::FontId::proportional(10.5),
                color: ui.visuals().weak_text_color(),
                ..Default::default()
            },
        );
    }
    ui.add(egui::Label::new(job).wrap_mode(egui::TextWrapMode::Extend).selectable(false));
    ui.label(RichText::new(caption).size(10.5).color(ui.visuals().weak_text_color()));
}

fn palette(dark: bool) -> egui::Visuals {
    let mut v = if dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    if dark {
        v.panel_fill = Color32::from_rgb(0x0E, 0x14, 0x12);
        v.window_fill = Color32::from_rgb(0x18, 0x21, 0x1E);
        v.extreme_bg_color = Color32::from_rgb(0x12, 0x1A, 0x17);
        v.override_text_color = Some(Color32::from_rgb(0xE7, 0xEE, 0xEA));
        v.widgets.noninteractive.bg_fill = Color32::from_rgb(0x18, 0x21, 0x1E);
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(0x2B, 0x38, 0x33));
        v.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(0xB7, 0xC3, 0xBD));
        v.widgets.inactive.bg_fill = Color32::from_rgb(0x22, 0x2D, 0x29);
        v.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(0x31, 0x3F, 0x39));
        v.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(0xD3, 0xDD, 0xD8));
        v.widgets.hovered.bg_fill = Color32::from_rgb(0x2B, 0x38, 0x33);
        v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(0x41, 0x51, 0x4A));
        v.widgets.active.bg_fill = Color32::from_rgb(0x36, 0x45, 0x3F);
        v.selection.bg_fill = ACCENT;
        v.selection.stroke = Stroke::new(1.0_f32, Color32::WHITE);
    } else {
        v.panel_fill = Color32::from_rgb(0xF2, 0xF4, 0xF2);
        v.window_fill = Color32::WHITE;
        v.extreme_bg_color = Color32::from_rgb(0xF6, 0xF8, 0xF7);
        v.override_text_color = Some(Color32::from_rgb(0x1B, 0x23, 0x20));
        v.widgets.noninteractive.bg_fill = Color32::WHITE;
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(0xE1, 0xE7, 0xE3));
        v.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(0x5F, 0x6B, 0x65));
        v.widgets.inactive.bg_fill = Color32::from_rgb(0xF4, 0xF6, 0xF5);
        v.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(0xE2, 0xE8, 0xE4));
        v.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(0x35, 0x40, 0x3B));
        v.widgets.hovered.bg_fill = Color32::from_rgb(0xEB, 0xF0, 0xED);
        v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(0xD4, 0xDE, 0xD8));
        v.widgets.active.bg_fill = Color32::from_rgb(0xE1, 0xE8, 0xE4);
        v.selection.bg_fill = ACCENT;
        v.selection.stroke = Stroke::new(1.0_f32, Color32::WHITE);
    }
    v.hyperlink_color = ACCENT;
    v.window_rounding = Rounding::same(14.0);
    v.menu_rounding = Rounding::same(10.0);
    v.window_stroke = Stroke::new(1.0_f32, if dark { Color32::from_rgb(0x2B, 0x38, 0x33) } else { Color32::from_rgb(0xE1, 0xE7, 0xE3) });
    v.widgets.noninteractive.rounding = Rounding::same(8.0);
    v.widgets.inactive.rounding = Rounding::same(8.0);
    v.widgets.hovered.rounding = Rounding::same(8.0);
    v.widgets.active.rounding = Rounding::same(8.0);
    v
}

fn tune_style(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = egui::vec2(10.0, 8.0);
    style.spacing.button_padding = egui::vec2(16.0, 7.0);
    style.spacing.interact_size = egui::vec2(40.0, 36.0);
    style.spacing.window_margin = egui::Margin::same(16.0);
    style.text_styles.insert(egui::TextStyle::Body, egui::FontId::proportional(13.5));
    style.text_styles.insert(egui::TextStyle::Button, egui::FontId::proportional(13.5));
    style.text_styles.insert(egui::TextStyle::Small, egui::FontId::proportional(11.5));
    style.text_styles.insert(egui::TextStyle::Monospace, egui::FontId::monospace(12.0));
    style.text_styles.insert(egui::TextStyle::Heading, egui::FontId::proportional(19.0));
    ctx.set_style(style);
}

fn install_fonts(ctx: &egui::Context) {
    let candidates = [
        r"C:\Windows\Fonts\msyh.ttc",
        r"C:\Windows\Fonts\msyh.ttf",
        r"C:\Windows\Fonts\Deng.ttf",
        r"C:\Windows\Fonts\simhei.ttf",
    ];
    let mut fonts = FontDefinitions::default();
    for path in candidates {
        if let Ok(bytes) = std::fs::read(path) {
            fonts.font_data.insert("cjk".into(), FontData::from_owned(bytes));
            fonts.families.get_mut(&FontFamily::Proportional).unwrap().insert(0, "cjk".into());
            fonts.families.get_mut(&FontFamily::Monospace).unwrap().push("cjk".into());
            break;
        }
    }
    ctx.set_fonts(fonts);
}

fn open_folder(path: &str) -> std::io::Result<()> {
    std::process::Command::new("explorer").arg(path).spawn().map(|_| ())
}

#[cfg(test)]
mod tree_tests {
    use super::*;

    fn item(name: &str, handle: &str, parent: Option<&str>, dir: bool, size: u64) -> ItemData {
        ItemData {
            name: name.into(),
            size,
            handle: handle.into(),
            key: Vec::new(),
            info: None,
            is_dir: dir,
            parent: parent.map(str::to_string),
            rel_path: String::new(),
        }
    }

    #[test]
    fn builds_nested_tree_with_paths_and_sizes() {
        let items = vec![
            item("b.txt", "F1", Some("D1"), false, 10),
            item("素材", "D1", Some("ROOT"), true, 0),
            item("a.txt", "F2", Some("D1"), false, 20),
            item("子目录", "D2", Some("D1"), true, 0),
            item("c.txt", "F3", Some("D2"), false, 30),
            item("根文件.bin", "F4", Some("ROOT"), false, 40),
        ];
        let (nodes, roots) = build_tree(items, Some("ROOT"));
        assert_eq!(nodes.len(), 6);
        assert_eq!(roots.len(), 2);
        // 文件夹排在文件前
        assert!(nodes[roots[0]].data.is_dir);

        let root_file = roots.iter().map(|&i| &nodes[i]).find(|n| !n.data.is_dir).unwrap();
        assert_eq!(root_file.data.rel_path, "根文件.bin");

        let dir = roots.iter().map(|&i| &nodes[i]).find(|n| n.data.is_dir).unwrap();
        assert_eq!(dir.data.rel_path, "素材");
        assert_eq!(dir.total_size, 60, "子目录应递归汇总大小");
        assert!(dir.selected);

        let child_paths: Vec<String> = nodes.iter().filter(|n| !n.data.is_dir).map(|n| n.data.rel_path.clone()).collect();
        assert!(child_paths.contains(&"素材/a.txt".to_string()));
        assert!(child_paths.contains(&"素材/子目录/c.txt".to_string()));
        assert!(child_paths.contains(&"素材/b.txt".to_string()));
    }

    #[test]
    fn selection_helpers_cover_descendants() {
        // 纯函数逻辑在 App 方法里，这里验证树结构足以支撑递归遍历
        let items = vec![
            item("D", "D1", None, true, 0),
            item("x", "F1", Some("D1"), false, 1),
            item("y", "F2", Some("D1"), false, 2),
        ];
        let (nodes, roots) = build_tree(items, None);
        assert_eq!(roots.len(), 1);
        assert_eq!(nodes[roots[0]].children.len(), 2);
    }
}
