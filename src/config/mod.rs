use std::path::PathBuf;

pub mod app_config;
pub mod errors;
pub mod theme_config;

pub fn config_root() -> Option<PathBuf> {
    dirs::config_dir().map(|path| path.join("oc-stats"))
}

pub fn config_path() -> Option<PathBuf> {
    config_root().map(|path| path.join("config.toml"))
}

pub fn themes_dir_path() -> Option<PathBuf> {
    config_root().map(|path| path.join("themes"))
}
