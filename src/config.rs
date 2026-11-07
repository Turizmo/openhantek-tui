//! Load and save settings as TOML in the user's config folder.

use std::path::{Path, PathBuf};

use serde::{de::DeserializeOwned, Serialize};

/// `$XDG_CONFIG_HOME/openhantek-tui`, or `~/.config/openhantek-tui`.
pub fn config_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(dir).join("openhantek-tui"));
    }
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config/openhantek-tui"))
}

/// Default settings file.
pub fn settings_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("settings.toml"))
}

/// Read settings. A missing file gives the defaults. A broken file gives
/// the defaults plus an error text to show to the user.
pub fn load<T: DeserializeOwned + Default>(path: &Path) -> (T, Option<String>) {
    match std::fs::read_to_string(path) {
        Ok(text) => match toml::from_str(&text) {
            Ok(value) => (value, None),
            Err(e) => (
                T::default(),
                Some(format!("Bad settings file {}: {e}", path.display())),
            ),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (T::default(), None),
        Err(e) => (
            T::default(),
            Some(format!("Cannot read {}: {e}", path.display())),
        ),
    }
}

/// Write settings, and make the folder if needed.
pub fn save<T: Serialize>(path: &Path, value: &T) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text = toml::to_string_pretty(value)?;
    // Write to a temp file first, so a crash cannot leave half a file.
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Serialize, Deserialize, Default, Debug, PartialEq)]
    #[serde(default)]
    struct Demo {
        a: u32,
        b: String,
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("oht-cfg-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn round_trip() {
        let dir = temp_dir("rt");
        let path = dir.join("sub/settings.toml");
        let v = Demo {
            a: 7,
            b: "x".into(),
        };
        save(&path, &v).unwrap();
        let (back, err): (Demo, _) = load(&path);
        assert_eq!(back, v);
        assert!(err.is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_file_gives_defaults_without_error() {
        let (v, err): (Demo, _) = load(Path::new("/nonexistent/oht/settings.toml"));
        assert_eq!(v, Demo::default());
        assert!(err.is_none());
    }

    #[test]
    fn broken_file_gives_defaults_and_error() {
        let dir = temp_dir("bad");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.toml");
        std::fs::write(&path, "a = \"not a number\"").unwrap();
        let (v, err): (Demo, _) = load(&path);
        assert_eq!(v, Demo::default());
        assert!(err.unwrap().contains("Bad settings file"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_fields_use_defaults() {
        let dir = temp_dir("partial");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.toml");
        std::fs::write(&path, "a = 3").unwrap();
        let (v, _): (Demo, _) = load(&path);
        assert_eq!(v, Demo { a: 3, b: String::new() });
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
