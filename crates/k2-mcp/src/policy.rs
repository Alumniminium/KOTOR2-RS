//! What a caller may pass to the game through game_start: extra command-line
//! arguments and environment. Only the allowlisted flags reach the game, so
//! the headless entry points (the audio and sfx surveys, the editor, screenshot
//! runs) and the debug switches are not reachable through the MCP. The
//! isolation variables are set by the supervisor, so a caller may not set them.

use std::path::PathBuf;

/// Switches the game takes with no value. The presentation ones (--mute,
/// --skip-movies, --no-logos) only change sound and the intro.
const SWITCHES: &[&str] = &[
    "--control", "--newgame", "--continue", "--chargen", "--quickstart", "--debug", "--autoattack", "--mute", "--skip-movies", "--no-logos",
];

/// Options the game takes one value for. The value may not start with "--".
const VALUED: &[&str] = &[
    "--load-slot", "--module", "--autowalk", "--autouse", "--autotalk", "--autoattack-door", "--cam", "--player-appearance", "--volume",
];

/// Checks game_start's extra arguments: each one is an allowlisted flag, and
/// each valued flag has its value.
pub fn check_args(args: &[String]) -> Result<(), String> {
    if let Some(arg) = args.iter().find(|a| a.contains('\0')) {
        return Err(format!("argument {arg:?} contains a NUL byte"));
    }
    let mut i = 0;
    while i < args.len() {
        let flag = args[i].as_str();
        if SWITCHES.contains(&flag) {
            i += 1;
        } else if VALUED.contains(&flag) {
            match args.get(i + 1) {
                Some(value) if !value.starts_with("--") => i += 2,
                _ => return Err(format!("{flag} needs a value")),
            }
        } else {
            let allowed: Vec<&str> = SWITCHES.iter().chain(VALUED.iter()).copied().collect();
            return Err(format!("argument {flag:?} is not allowed through the MCP. Allowed: {}", allowed.join(" ")));
        }
    }
    Ok(())
}

/// Checks game_start's extra environment. Names are compared without regard to
/// case, since the game is not case-sensitive about its folders. The run's
/// folders and window are the supervisor's, and no K2_ variable is accepted:
/// the game's K2_ knobs are debug and test switches. Variables that would move
/// the game's data folders or load code into it are refused too. A name with
/// '=' or NUL, or a value with NUL, cannot be an environment entry at all.
pub fn check_env(env: &[(String, String)]) -> Result<(), String> {
    for (key, value) in env {
        if key.is_empty() || key.contains('=') || key.contains('\0') {
            return Err(format!("environment name {key:?} is not valid: it is empty or contains '=' or NUL"));
        }
        if value.contains('\0') {
            return Err(format!("environment {key} has a value with a NUL byte"));
        }
        let upper = key.to_ascii_uppercase();
        if upper.starts_with("K2_") {
            return Err(format!("environment {key} is not allowed: the supervisor sets the run's folders and window, and no K2_ variable can be passed"));
        }
        if upper == "WAYLAND_DISPLAY" || upper == "HOME" || upper.starts_with("XDG_") || upper.starts_with("LD_") {
            return Err(format!("environment {key} is not allowed: it would move the game's data or window or load code into the game"));
        }
    }
    Ok(())
}

/// Both checks, for a game_start.
pub fn check_launch(args: &[String], env: &[(String, String)]) -> Result<(), String> {
    check_args(args)?;
    check_env(env)
}

/// Whether this build may run a game binary other than a fresh build: unit
/// tests, or any run with K2_MCP_TEST=1 (the integration tests set it).
pub fn test_mode() -> bool {
    cfg!(test) || std::env::var("K2_MCP_TEST").is_ok_and(|v| v == "1")
}

/// K2_MCP_GAME_BIN: the game to run instead of a fresh build. Honoured in test
/// mode only, so a stray variable cannot swap the game.
pub fn game_bin_override() -> Option<PathBuf> {
    if !test_mode() {
        return None;
    }
    std::env::var_os("K2_MCP_GAME_BIN").filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// Whether the debug_* verbs are offered. They skip the rules, so they are
/// listed only when the server itself was started with K2_MCP_DEBUG=1.
pub fn debug_tools_enabled() -> bool {
    std::env::var("K2_MCP_DEBUG").is_ok_and(|v| v == "1")
}

#[cfg(test)]
mod tests;
