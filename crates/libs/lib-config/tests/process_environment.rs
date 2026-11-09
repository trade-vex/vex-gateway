//! `Config::load` and `Config::load_auto` read the process environment, which this test changes.
//! It must stay the only test in this binary, so that no other test reads the environment while it
//! does.
#![cfg(unix)]

use lib_config::{Config, ConfigError, Environment};
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;

#[test]
fn test_only_app_variables_are_read_and_one_that_is_not_utf8_is_an_error() {
    let config_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        config_dir.path().join("development.toml"),
        include_str!("../../../../config/development.toml.example"),
    )
    .unwrap();
    let load = || {
        Config::load(
            Environment::Development,
            Some(config_dir.path().to_path_buf()),
        )
    };
    let secret = "Sup3r-S3cret";
    let not_utf8 = [secret.as_bytes(), b"\xff"].concat();

    // Start without `APP__` variables, whatever the shell running the tests set.
    for (name, _) in std::env::vars_os() {
        let prefix = name.as_bytes().get(..5);
        if prefix.is_some_and(|prefix| prefix.eq_ignore_ascii_case(b"APP__")) {
            std::env::remove_var(name);
        }
    }

    // Another program's variables are not read, whatever they hold. Before, the config library
    // read them all and panicked on these, printing the variable.
    std::env::set_var("NOT_THE_GATEWAYS", OsStr::from_bytes(&not_utf8));
    std::env::set_var(OsStr::from_bytes(b"NOT_THE_GATEWAYS_\xff"), "1");
    assert!(load().is_ok());

    // `APP__` variables are, the prefix in any case, as the config library matches it.
    std::env::set_var("App__Server__Port", "9090");
    assert_eq!(load().unwrap().server.port, 9090);

    // One whose value is not UTF-8 is an error that names it and never shows the value.
    std::env::set_var(
        "APP__AUTH__EMAIL__SMTP_PASSWORD",
        OsStr::from_bytes(&not_utf8),
    );
    let error = load().unwrap_err();
    assert!(matches!(error, ConfigError::LoadError(_)));
    assert_eq!(
        error.to_string(),
        "Failed to load configuration: APP__AUTH__EMAIL__SMTP_PASSWORD is not valid UTF-8"
    );
    assert!(!format!("{error:?}").contains(secret));
    std::env::remove_var("APP__AUTH__EMAIL__SMTP_PASSWORD");

    // So is one whose name is not UTF-8, named lossily.
    std::env::set_var(OsStr::from_bytes(b"APP__SERVER__\xffPORT"), "9090");
    assert_eq!(
        load().unwrap_err().to_string(),
        "Failed to load configuration: APP__SERVER__\u{FFFD}PORT is not valid UTF-8"
    );
    std::env::remove_var(OsStr::from_bytes(b"APP__SERVER__\xffPORT"));

    // `APP_ENV` selects the file that `APP_CONFIG_DIR` holds.
    std::env::set_var("APP_CONFIG_DIR", config_dir.path());
    std::env::set_var("APP_ENV", "development");
    assert_eq!(Config::load_auto().unwrap().server.port, 9090);

    // One that is not UTF-8 is an error that says so, not a missing `APP_ENV`, and never shows the
    // value.
    std::env::set_var("APP_ENV", OsStr::from_bytes(&not_utf8));
    let error = Config::load_auto().unwrap_err();
    assert!(matches!(error, ConfigError::LoadError(_)));
    assert_eq!(
        error.to_string(),
        "Failed to load configuration: APP_ENV is not valid UTF-8"
    );
    assert!(!format!("{error:?}").contains(secret));

    std::env::remove_var("APP_ENV");
    assert!(matches!(
        Config::load_auto(),
        Err(ConfigError::MissingEnvironment)
    ));
}
