//! 给 Windows 可执行文件嵌入图标与版本信息（找不到 SDK 的 rc.exe 时跳过着色）。
#[cfg(windows)]
fn find_rc_dir() -> Option<String> {
    let base = r"C:\Program Files (x86)\Windows Kits\10\bin";
    let mut versions: Vec<String> = std::fs::read_dir(base)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|v| std::path::Path::new(base).join(v).join("x64").join("rc.exe").exists())
        .collect();
    versions.sort();
    versions.pop().map(|v| std::path::Path::new(base).join(v).join("x64").to_string_lossy().into_owned())
}

#[cfg(windows)]
fn main() {
    println!("cargo:rerun-if-changed=assets/megadl.ico");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=Cargo.toml");
    let Some(rc_dir) = find_rc_dir() else {
        println!("cargo:warning=未找到 Windows SDK 的 rc.exe，跳过图标资源");
        return;
    };
    let path = std::env::var("PATH").unwrap_or_default();
    std::env::set_var("PATH", format!("{rc_dir};{path}"));
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.1.0".into());
    let mut res = winres::WindowsResource::new();
    res.set_icon("assets/megadl.ico");
    res.set("FileDescription", "MegaDL - MEGA 公共链接下载器");
    res.set("ProductName", "MegaDL");
    res.set("OriginalFilename", "megadl.exe");
    res.set("FileVersion", &format!("{version}.0"));
    res.set("ProductVersion", &format!("{version}.0"));
    res.compile().expect("嵌入图标资源失败");
}

#[cfg(not(windows))]
fn main() {}
