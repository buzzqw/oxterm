//! Load the user's `extensions.json` once per process.
//!
//! The parsed configuration is shared by the status bar and the command
//! palette. A malformed file is reported in the log and ignored: a typo in an
//! optional extension must never stop the terminal from starting.

use once_cell::sync::Lazy;

use crate::headless::extensions::UiConfig;
use crate::logging::LOGGER;

static CONFIG: Lazy<UiConfig> = Lazy::new(|| {
    let path = crate::headless::paths::config_dir().join("extensions.json");
    match UiConfig::load(&path) {
        Ok(config) => {
            if !config.is_empty() {
                LOGGER.info(&format!(
                    "extensions_loaded widgets={} palette={}",
                    config.widgets.len(),
                    config.palette.len()
                ));
            }
            config
        }
        Err(error) => {
            LOGGER.warning(&format!("extensions_failed error={}", error));
            UiConfig::default()
        }
    }
});

pub fn config() -> &'static UiConfig {
    &CONFIG
}
