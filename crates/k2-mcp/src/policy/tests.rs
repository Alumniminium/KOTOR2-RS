use super::*;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

fn env(key: &str, value: &str) -> Vec<(String, String)> {
    vec![(key.to_string(), value.to_string())]
}

#[test]
fn allowlisted_flags_pass_with_their_values() {
    assert!(check_args(&args(&["--debug", "--module", "001ebo", "--autowalk", "3,90", "--quickstart"])).is_ok());
    assert!(check_args(&args(&["--cam", "1,2,3,0,0", "--autotalk", "npc_tag"])).is_ok());
    assert!(check_args(&args(&["--mute", "--skip-movies", "--no-logos", "--volume", "0.3"])).is_ok());
    assert!(check_args(&[]).is_ok());
}

#[test]
fn headless_entry_points_and_unknown_flags_are_refused() {
    for flag in [
        "--sfx-survey", "--audio-survey", "--audio-test", "--edit-selftest", "--edit", "--mod-dir", "--mod-name", "--screenshot", "--install",
        "--frames", "--fly", "--autoreply", "--autotransition", "--bogus",
    ] {
        let err = check_args(&args(&[flag, "x"])).unwrap_err();
        assert!(err.contains("not allowed"), "{flag}: {err}");
    }
}

#[test]
fn a_valued_flag_needs_a_value_that_is_not_a_flag() {
    assert!(check_args(&args(&["--module"])).is_err());
    assert!(check_args(&args(&["--module", "--debug"])).is_err());
    assert!(check_args(&args(&["--module", "--screenshot"])).is_err());
}

#[test]
fn a_nul_in_an_argument_is_refused() {
    assert!(check_args(&args(&["--module", "001ebo\0x"])).is_err());
}

#[test]
fn k2_variables_and_the_window_cannot_be_set() {
    for key in ["K2_SAVE_DIR", "K2_SETTINGS_DIR", "K2_CTL_DIR", "K2_SHOT_DIR", "K2_TRACE_DIR", "K2_INSTALL", "K2_MCP_GAME_BIN", "K2_TEST_WINDOW", "K2_CONTROL", "K2_MOD_DIR", "WAYLAND_DISPLAY", "HOME", "XDG_DATA_HOME", "LD_PRELOAD"] {
        assert!(check_env(&env(key, "x")).is_err(), "{key} must be refused");
    }
    assert!(check_env(&env("RUST_LOG", "debug")).is_ok());
    assert!(check_launch(&[], &env("RUST_LOG", "debug")).is_ok());
}

#[test]
fn env_names_are_refused_whatever_their_case() {
    for key in ["home", "Home", "k2_save_dir", "K2_save_dir", "xdg_data_home", "ld_preload", "wayland_display"] {
        assert!(check_env(&env(key, "x")).is_err(), "{key} must be refused");
    }
}

#[test]
fn an_entry_with_equals_or_nul_is_refused() {
    assert!(check_env(&env("HOME=/x", "")).is_err(), "a name with '=' is not an entry");
    assert!(check_env(&env("", "x")).is_err(), "an empty name is refused");
    assert!(check_env(&env("RUST_LOG", "debug\0")).is_err(), "a NUL in a value is refused");
    assert!(check_env(&env("RUST_L\0G", "debug")).is_err(), "a NUL in a name is refused");
}

#[test]
fn display_and_path_pass_through() {
    assert!(check_env(&env("DISPLAY", ":0")).is_ok());
    assert!(check_env(&env("XAUTHORITY", "/run/user/1000/xauth")).is_ok());
    assert!(check_env(&env("PATH", "/usr/bin")).is_ok());
}

#[test]
fn the_test_mode_is_on_in_unit_tests() {
    assert!(test_mode(), "unit tests are test mode, so K2_MCP_GAME_BIN would be honoured here");
}
