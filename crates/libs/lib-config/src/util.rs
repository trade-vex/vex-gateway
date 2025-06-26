//! lib-config/src/util.rs

use std::path::PathBuf;
use tracing::{info, warn};

/// Find the configuration directory by searching upwards from the current directory.
///
/// This function implements a robust config discovery mechanism that works
/// regardless of where the application is run from.
///
/// # Priority Order
///
/// 1. `APP_CONFIG_DIR` environment variable (if set and valid)
/// 2. `config/` directory in current directory or any parent directory
/// 3. Current directory as fallback
///
/// # Returns
///
/// Returns the path to the directory containing the `config/` subdirectory.
pub fn find_config_dir() -> PathBuf {
    // 1. Check APP_CONFIG_DIR env var
    if let Ok(dir) = std::env::var("APP_CONFIG_DIR") {
        let path = PathBuf::from(&dir);
        if path.exists() && path.is_dir() {
            info!("Using config directory from APP_CONFIG_DIR: {:?}", path);
            return path;
        } else {
            warn!(
                "APP_CONFIG_DIR set to {:?} but directory doesn't exist, falling back to search",
                path
            );
        }
    }

    // 2. Search for config/ upwards
    let mut dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    loop {
        let candidate = dir.join("config");
        if candidate.exists() && candidate.is_dir() {
            info!("Found config directory: {:?}", dir);
            return dir;
        }
        if !dir.pop() {
            break;
        }
    }

    // 3. Fallback: current directory
    warn!("No config directory found, using current directory");
    PathBuf::from(".")
}
