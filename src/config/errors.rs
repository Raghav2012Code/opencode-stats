use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("failed to read config file '{path}': {source}")]
    ConfigRead {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to parse config file '{path}': {source}")]
    ConfigParse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    #[error("failed to read theme file '{path}': {source}")]
    ThemeRead {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to parse theme file '{path}': {source}")]
    ThemeParse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    #[error("invalid color '{value}', expected #RRGGBB")]
    InvalidColor { value: String },

    #[error("invalid value '{value}' for config key {key}; expected one of {expected}")]
    InvalidAppValue {
        key: &'static str,
        value: String,
        expected: String,
    },

    #[error("theme name cannot be empty")]
    EmptyThemeName,

    #[error("invalid theme filename '{path}'")]
    InvalidThemeFilename { path: PathBuf },

    #[error("expected 12 model colors, got {0}")]
    ModelColorNum(usize),

    #[error("failed to parse model color array")]
    ModelColorParse,
}

impl Error {
    pub fn config_read(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::ConfigRead {
            path: path.into(),
            source,
        }
    }

    pub fn config_parse(path: impl Into<PathBuf>, source: toml::de::Error) -> Self {
        Self::ConfigParse {
            path: path.into(),
            source,
        }
    }

    pub fn theme_read(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::ThemeRead {
            path: path.into(),
            source,
        }
    }

    pub fn theme_parse(path: impl Into<PathBuf>, source: toml::de::Error) -> Self {
        Self::ThemeParse {
            path: path.into(),
            source,
        }
    }

    pub fn invalid_color(value: impl Into<String>) -> Self {
        Self::InvalidColor {
            value: value.into(),
        }
    }

    pub fn invalid_app_value(
        key: &'static str,
        value: impl Into<String>,
        expected: &[&str],
    ) -> Self {
        Self::InvalidAppValue {
            key,
            value: value.into(),
            expected: expected.join(", "),
        }
    }

    pub fn invalid_theme_filename(path: impl Into<PathBuf>) -> Self {
        Self::InvalidThemeFilename { path: path.into() }
    }
}

pub type Result<T> = std::result::Result<T, Error>;
