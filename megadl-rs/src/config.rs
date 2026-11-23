//! 设置持久化：%APPDATA%\megadl\config.json

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::download::Collision;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Route {
    Auto,
    Eu,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Theme {
    Light,
    Dark,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub out_dir: String,
    pub connections: usize,
    pub part_mb: u64,
    pub parallel_files: usize,
    pub proxy: String,
    pub route: Route,
    pub collision: Collision,
    pub theme: Theme,
}

impl Default for Config {
    fn default() -> Self {
        let out = dirs::download_dir()
            .or_else(dirs::home_dir)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| ".".into());
        Self {
            out_dir: out,
            connections: 8,
            part_mb: 4,
            parallel_files: 3,
            proxy: String::new(),
            route: Route::Auto,
            collision: Collision::Rename,
            theme: Theme::Light,
        }
    }
}

impl Config {
    pub fn path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("megadl")
            .join("config.json")
    }

    pub fn load() -> Self {
        let Ok(text) = fs::read_to_string(Self::path()) else { return Self::default() };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else { return Self::default() };
        // 旧版本配置没有 parallel_files；升级时把速度相关项迁移到新默认值
        let legacy = value.get("parallel_files").is_none();
        let mut cfg: Self = serde_json::from_value(value).unwrap_or_default();
        if legacy {
            cfg.connections = 8;
            cfg.part_mb = 4;
            cfg.parallel_files = 3;
        }
        cfg.connections = cfg.connections.clamp(1, 16);
        cfg.parallel_files = cfg.parallel_files.clamp(1, 6);
        cfg.part_mb = cfg.part_mb.clamp(1, 16);
        cfg
    }

    pub fn save(&self) {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let _ = fs::write(path, s);
        }
    }
}
