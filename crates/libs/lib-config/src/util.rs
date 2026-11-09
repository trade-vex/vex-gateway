//! lib-config/src/util.rs

use crate::ConfigError;
use std::ffi::OsString;
use std::path::PathBuf;
use tracing::{info, warn};

/// Find the configuration directory by searching upwards from the current directory.
///
/// This function implements a robust config discovery mechanism that works
/// regardless of where the application is run from.
///
/// # Priority Order
///
/// 1. `APP_CONFIG_DIR` environment variable, which must name a directory: any other value is an
///    error rather than a reason to search elsewhere
/// 2. `config/` directory in current directory or any parent directory
/// 3. Current directory as fallback
///
/// # Returns
///
/// Returns the path to the `config/` directory.
pub fn find_config_dir() -> Result<PathBuf, ConfigError> {
    config_dir_from(std::env::var_os("APP_CONFIG_DIR"))
}

fn config_dir_from(app_config_dir: Option<OsString>) -> Result<PathBuf, ConfigError> {
    // 1. Check APP_CONFIG_DIR env var
    if let Some(dir) = app_config_dir {
        let path = PathBuf::from(dir);
        if !path.is_dir() {
            return Err(ConfigError::ConfigDirNotFound(path));
        }
        info!("Using config directory from APP_CONFIG_DIR: {:?}", path);
        return Ok(path);
    }

    // 2. Search for config/ upwards
    let mut dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    loop {
        let candidate = dir.join("config");
        if candidate.exists() && candidate.is_dir() {
            info!("Found config directory: {:?}", candidate);
            return Ok(candidate);
        }
        if !dir.pop() {
            break;
        }
    }

    // 3. Fallback: current directory
    warn!("No config directory found, using current directory");
    Ok(PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_config_dir_must_name_a_directory() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            config_dir_from(Some(dir.path().into())).unwrap(),
            dir.path()
        );

        let file = dir.path().join("production.toml");
        std::fs::write(&file, "").unwrap();
        for wrong in [dir.path().join("missing"), file, PathBuf::new()] {
            assert!(
                matches!(
                    config_dir_from(Some(wrong.clone().into())),
                    Err(ConfigError::ConfigDirNotFound(path)) if path == wrong
                ),
                "{wrong:?} must be rejected"
            );
        }
    }
}
