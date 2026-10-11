//! Human-equivalent input for the windowed game: key presses, typed text,
//! pointer moves, clicks, drags and wheel scrolls. They are injected as the
//! same Bevy input messages a window backend writes, so they reach the same
//! input systems, picking, GUI hit-testing and egui as real input. Nothing
//! leaves the game process: no desktop-level input injection.
//!
//! A request becomes a plan of timed steps (plan_*); the delivery system
//! writes each step when its time comes. The remote server (remote.rs) answers
//! the request when the queue has drained.

use std::collections::VecDeque;

use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyCode, KeyboardInput, NativeKey};
use bevy::input::mouse::{MouseButton, MouseButtonInput, MouseScrollUnit, MouseWheel};
use bevy::input::touch::TouchPhase;
use bevy::prelude::*;
use bevy::window::CursorMoved;
use k2_world::control::MouseButtonName;

/// Gap between a click's release and the next click of a double click (ms).
const CLICK_GAP_MS: f64 = 60.0;
/// Time from moving the pointer to pressing the button (ms).
const SETTLE_MS: f64 = 12.0;
/// Time between two drag positions (ms).
const DRAG_STEP_MS: f64 = 16.0;
const DEFAULT_HOLD_MS: u32 = 60;
const DEFAULT_CLICK_HOLD_MS: u32 = 50;
const DEFAULT_TYPE_DELAY_MS: u32 = 30;
/// Longest text one type_text request may carry (characters).
const MAX_TYPE_CHARS: usize = 200;
/// Longest hold of a key or button, and longest wait between two typed
/// characters (ms).
const MAX_HOLD_MS: u32 = 10_000;
const MAX_TYPE_DELAY_MS: u32 = 1_000;
/// Most positions one drag passes through.
const MAX_DRAG_STEPS: u32 = 300;

/// One event to write into the app.
#[derive(Debug, Clone, PartialEq)]
pub enum Inject {
    Key { code: KeyCode, logical: Key, text: Option<String>, state: ButtonState },
    Cursor(Vec2),
    Button { button: MouseButton, state: ButtonState },
    Wheel { x: f32, y: f32 },
}

/// An event with the time it is due, in ms after the request started.
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub at_ms: f64,
    pub event: Inject,
}

/// Steps waiting to be written. The request is done when this is empty. It
/// also knows which keys and buttons the window was last given pressed, so an
/// abort can release them instead of leaving them down.
#[derive(Resource, Default)]
pub struct InputQueue {
    /// (due time in real seconds, event), oldest first.
    pending: VecDeque<(f64, Inject)>,
    held_keys: Vec<KeyCode>,
    held_buttons: Vec<MouseButton>,
}

impl InputQueue {
    /// Queues a plan that starts at real time `now` (seconds).
    pub fn push(&mut self, now: f64, steps: Vec<Step>) {
        let mut steps = steps;
        steps.sort_by(|a, b| a.at_ms.total_cmp(&b.at_ms));
        self.pending.extend(steps.into_iter().map(|s| (now + s.at_ms / 1000.0, s.event)));
    }

    /// Nothing left to write.
    pub fn is_idle(&self) -> bool {
        self.pending.is_empty()
    }

    /// Drops everything not yet written, and releases every key and button the
    /// window holds, due at `now`. Used when a request is stopped or times out,
    /// so no key or button stays down.
    pub fn abort(&mut self, now: f64) {
        self.pending.clear();
        for code in std::mem::take(&mut self.held_keys) {
            self.pending.push_back((now, key_event(code, ButtonState::Released, None)));
        }
        for button in std::mem::take(&mut self.held_buttons) {
            self.pending.push_back((now, Inject::Button { button, state: ButtonState::Released }));
        }
    }

    /// Takes the events due at `now`, in order, and records what they press.
    pub fn due(&mut self, now: f64) -> Vec<Inject> {
        let mut out = Vec::new();
        while self.pending.front().is_some_and(|(at, _)| *at <= now) {
            if let Some((_, event)) = self.pending.pop_front() {
                self.track(&event);
                out.push(event);
            }
        }
        out
    }

    /// Notes a key or button going down or up.
    fn track(&mut self, event: &Inject) {
        match event {
            Inject::Key { code, state, .. } => {
                self.held_keys.retain(|k| k != code);
                if *state == ButtonState::Pressed {
                    self.held_keys.push(*code);
                }
            }
            Inject::Button { button, state } => {
                self.held_buttons.retain(|b| b != button);
                if *state == ButtonState::Pressed {
                    self.held_buttons.push(*button);
                }
            }
            Inject::Cursor(_) | Inject::Wheel { .. } => {}
        }
    }
}

/// Key names accepted by the input verbs (see [`key_code`]).
pub fn key_code(name: &str) -> Option<KeyCode> {
    let n = name.trim();
    let lower = n.to_ascii_lowercase();
    if lower.len() == 1 {
        let c = lower.chars().next()?;
        return letter_or_digit(c);
    }
    if let Some(rest) = n.strip_prefix("Key") {
        if rest.len() == 1 {
            return letter_or_digit(rest.chars().next()?.to_ascii_lowercase());
        }
    }
    if let Some(rest) = n.strip_prefix("Digit") {
        if rest.len() == 1 {
            return letter_or_digit(rest.chars().next()?);
        }
    }
    if let Some(n) = lower.strip_prefix('f').and_then(|d| d.parse::<u8>().ok()) {
        return Some(match n {
            1 => KeyCode::F1,
            2 => KeyCode::F2,
            3 => KeyCode::F3,
            4 => KeyCode::F4,
            5 => KeyCode::F5,
            6 => KeyCode::F6,
            7 => KeyCode::F7,
            8 => KeyCode::F8,
            9 => KeyCode::F9,
            10 => KeyCode::F10,
            11 => KeyCode::F11,
            12 => KeyCode::F12,
            _ => return None,
        });
    }
    Some(match lower.as_str() {
        "enter" | "return" => KeyCode::Enter,
        "space" | "spacebar" => KeyCode::Space,
        "escape" | "esc" => KeyCode::Escape,
        "tab" => KeyCode::Tab,
        "backspace" => KeyCode::Backspace,
        "delete" | "del" => KeyCode::Delete,
        "insert" => KeyCode::Insert,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" => KeyCode::PageUp,
        "pagedown" => KeyCode::PageDown,
        "arrowup" | "up" => KeyCode::ArrowUp,
        "arrowdown" | "down" => KeyCode::ArrowDown,
        "arrowleft" | "left" => KeyCode::ArrowLeft,
        "arrowright" | "right" => KeyCode::ArrowRight,
        "shift" | "shiftleft" => KeyCode::ShiftLeft,
        "shiftright" => KeyCode::ShiftRight,
        "ctrl" | "control" | "controlleft" => KeyCode::ControlLeft,
        "controlright" => KeyCode::ControlRight,
        "alt" | "altleft" => KeyCode::AltLeft,
        "altright" => KeyCode::AltRight,
        "super" | "meta" | "superleft" => KeyCode::SuperLeft,
        "superright" => KeyCode::SuperRight,
        "capslock" => KeyCode::CapsLock,
        "minus" => KeyCode::Minus,
        "equal" => KeyCode::Equal,
        "bracketleft" => KeyCode::BracketLeft,
        "bracketright" => KeyCode::BracketRight,
        "backslash" => KeyCode::Backslash,
        "semicolon" => KeyCode::Semicolon,
        "quote" => KeyCode::Quote,
        "backquote" => KeyCode::Backquote,
        "comma" => KeyCode::Comma,
        "period" => KeyCode::Period,
        "slash" => KeyCode::Slash,
        _ => return None,
    })
}

fn letter_or_digit(c: char) -> Option<KeyCode> {
    Some(match c.to_ascii_lowercase() {
        'a' => KeyCode::KeyA,
        'b' => KeyCode::KeyB,
        'c' => KeyCode::KeyC,
        'd' => KeyCode::KeyD,
        'e' => KeyCode::KeyE,
        'f' => KeyCode::KeyF,
        'g' => KeyCode::KeyG,
        'h' => KeyCode::KeyH,
        'i' => KeyCode::KeyI,
        'j' => KeyCode::KeyJ,
        'k' => KeyCode::KeyK,
        'l' => KeyCode::KeyL,
        'm' => KeyCode::KeyM,
        'n' => KeyCode::KeyN,
        'o' => KeyCode::KeyO,
        'p' => KeyCode::KeyP,
        'q' => KeyCode::KeyQ,
        'r' => KeyCode::KeyR,
        's' => KeyCode::KeyS,
        't' => KeyCode::KeyT,
        'u' => KeyCode::KeyU,
        'v' => KeyCode::KeyV,
        'w' => KeyCode::KeyW,
        'x' => KeyCode::KeyX,
        'y' => KeyCode::KeyY,
        'z' => KeyCode::KeyZ,
        '0' => KeyCode::Digit0,
        '1' => KeyCode::Digit1,
        '2' => KeyCode::Digit2,
        '3' => KeyCode::Digit3,
        '4' => KeyCode::Digit4,
        '5' => KeyCode::Digit5,
        '6' => KeyCode::Digit6,
        '7' => KeyCode::Digit7,
        '8' => KeyCode::Digit8,
        '9' => KeyCode::Digit9,
        _ => return None,
    })
}

/// The key a character is typed with (shifted characters map to their key).
fn char_key(c: char) -> Option<KeyCode> {
    match c {
        ' ' => Some(KeyCode::Space),
        '\n' | '\r' => Some(KeyCode::Enter),
        '\t' => Some(KeyCode::Tab),
        c if c.is_ascii_alphanumeric() => letter_or_digit(c),
        _ => None,
    }
}

/// The character a key produces, as a logical key (letters and digits only).
fn logical_for(code: KeyCode) -> Key {
    let c = match code {
        KeyCode::Space => return Key::Space,
        KeyCode::Enter => return Key::Enter,
        KeyCode::Tab => return Key::Tab,
        KeyCode::Escape => return Key::Escape,
        KeyCode::Backspace => return Key::Backspace,
        KeyCode::Delete => return Key::Delete,
        KeyCode::ArrowUp => return Key::ArrowUp,
        KeyCode::ArrowDown => return Key::ArrowDown,
        KeyCode::ArrowLeft => return Key::ArrowLeft,
        KeyCode::ArrowRight => return Key::ArrowRight,
        _ => char_of(code),
    };
    match c {
        Some(c) => Key::Character(c.to_string().into()),
        None => Key::Unidentified(NativeKey::Unidentified),
    }
}

fn char_of(code: KeyCode) -> Option<char> {
    let name = format!("{code:?}");
    if let Some(l) = name.strip_prefix("Key") {
        return l.chars().next().map(|c| c.to_ascii_lowercase());
    }
    if let Some(d) = name.strip_prefix("Digit") {
        return d.chars().next();
    }
    None
}

fn mouse_button(name: Option<MouseButtonName>) -> MouseButton {
    match name.unwrap_or_default() {
        MouseButtonName::Left => MouseButton::Left,
        MouseButtonName::Right => MouseButton::Right,
        MouseButtonName::Middle => MouseButton::Middle,
    }
}

fn parse_modifiers(names: &[String]) -> Result<Vec<KeyCode>, String> {
    names
        .iter()
        .map(|m| match m.to_ascii_lowercase().as_str() {
            "shift" => Ok(KeyCode::ShiftLeft),
            "ctrl" | "control" => Ok(KeyCode::ControlLeft),
            "alt" => Ok(KeyCode::AltLeft),
            "super" | "meta" => Ok(KeyCode::SuperLeft),
            other => Err(format!("unknown modifier {other:?}: use shift, ctrl, alt or super")),
        })
        .collect()
}

fn key_event(code: KeyCode, state: ButtonState, text: Option<String>) -> Inject {
    let logical = match &text {
        Some(t) => Key::Character(t.clone().into()),
        None => logical_for(code),
    };
    Inject::Key { code, logical, text, state }
}

/// The text a key press types, for a key pressed with `modifiers`: letters,
/// digits and space; shift makes letters capitals. Ctrl and super are
/// shortcuts, not text. Shifted digits are symbols that depend on the layout,
/// so they type nothing.
fn key_text(code: KeyCode, modifiers: &[KeyCode]) -> Option<String> {
    if modifiers.iter().any(|m| matches!(m, KeyCode::ControlLeft | KeyCode::SuperLeft)) {
        return None;
    }
    if code == KeyCode::Space {
        return Some(" ".to_owned());
    }
    let c = char_of(code)?;
    let shifted = modifiers.contains(&KeyCode::ShiftLeft);
    if c.is_ascii_digit() {
        return (!shifted).then(|| c.to_string());
    }
    Some(if shifted { c.to_ascii_uppercase() } else { c }.to_string())
}

fn named_key(name: &str) -> Result<KeyCode, String> {
    key_code(name).ok_or_else(|| format!("unknown key {name:?} (try KeyA, Digit1, Enter, Space, ArrowUp, F5, ShiftLeft)"))
}

/// How long a key_press holds its key, in game seconds: `hold_ms` (default 60),
/// capped at MAX_HOLD_MS. The hold runs on the game's clock (see remote.rs), not
/// on wall-clock time, so it is measured here in seconds only.
pub fn hold_seconds(hold_ms: Option<u32>) -> f64 {
    f64::from(hold_ms.unwrap_or(DEFAULT_HOLD_MS).min(MAX_HOLD_MS)) / 1000.0
}

/// Presses a key (and modifiers) and keeps them down; key_up releases them.
pub fn plan_key_down(key: &str, modifiers: &[String]) -> Result<Vec<Step>, String> {
    let code = named_key(key)?;
    let mods = parse_modifiers(modifiers)?;
    let mut steps: Vec<Step> = mods.iter().map(|m| Step { at_ms: 0.0, event: key_event(*m, ButtonState::Pressed, None) }).collect();
    steps.push(Step { at_ms: 0.0, event: key_event(code, ButtonState::Pressed, key_text(code, &mods)) });
    Ok(steps)
}

/// Releases a key, then the modifiers.
pub fn plan_key_up(key: &str, modifiers: &[String]) -> Result<Vec<Step>, String> {
    let code = named_key(key)?;
    let mut steps = vec![Step { at_ms: 0.0, event: key_event(code, ButtonState::Released, None) }];
    for m in parse_modifiers(modifiers)?.into_iter().rev() {
        steps.push(Step { at_ms: 0.0, event: key_event(m, ButtonState::Released, None) });
    }
    Ok(steps)
}

/// Types text: each character is a key press with its text, then a release.
pub fn plan_type_text(text: &str, delay_ms: Option<u32>) -> Result<Vec<Step>, String> {
    if text.chars().count() > MAX_TYPE_CHARS {
        return Err(format!("type_text takes at most {MAX_TYPE_CHARS} characters"));
    }
    let delay = f64::from(delay_ms.unwrap_or(DEFAULT_TYPE_DELAY_MS).min(MAX_TYPE_DELAY_MS));
    let mut steps = Vec::new();
    for (i, c) in text.chars().enumerate() {
        let at = i as f64 * delay;
        let code = char_key(c).ok_or_else(|| format!("cannot type {c:?}: only letters, digits, space, tab and newline"))?;
        let shifted = c.is_ascii_uppercase();
        let mods: &[KeyCode] = if shifted { &[KeyCode::ShiftLeft] } else { &[] };
        for m in mods {
            steps.push(Step { at_ms: at, event: key_event(*m, ButtonState::Pressed, None) });
        }
        let typed = if c == '\n' || c == '\r' || c == '\t' { None } else { Some(c.to_string()) };
        steps.push(Step { at_ms: at, event: key_event(code, ButtonState::Pressed, typed) });
        steps.push(Step { at_ms: at + delay.min(10.0), event: key_event(code, ButtonState::Released, None) });
        for m in mods {
            steps.push(Step { at_ms: at + delay.min(10.0), event: key_event(*m, ButtonState::Released, None) });
        }
    }
    Ok(steps)
}

/// A pointer position must be a number inside the window (logical pixels,
/// origin top left). `size` is the window's logical size, None without a window.
pub fn check_point(p: [f32; 2], size: Option<Vec2>) -> Result<(), String> {
    let Some(size) = size.filter(|s| s.x > 0.0 && s.y > 0.0) else {
        return Err("the game window has no size yet".to_owned());
    };
    if !p[0].is_finite() || !p[1].is_finite() {
        return Err(format!("position ({}, {}) is not a number", p[0], p[1]));
    }
    if p[0] < 0.0 || p[1] < 0.0 || p[0] >= size.x || p[1] >= size.y {
        return Err(format!("position ({}, {}) is outside the {}x{} window", p[0], p[1], size.x, size.y));
    }
    Ok(())
}

/// Moves the pointer to a window position (logical pixels).
pub fn plan_mouse_move(x: f32, y: f32) -> Vec<Step> {
    vec![Step { at_ms: 0.0, event: Inject::Cursor(Vec2::new(x, y)) }]
}

/// Moves to a position, then clicks `clicks` times in a row.
pub fn plan_mouse_click(x: f32, y: f32, button: Option<MouseButtonName>, clicks: Option<u32>, hold_ms: Option<u32>) -> Vec<Step> {
    let button = mouse_button(button);
    let hold = f64::from(hold_ms.unwrap_or(DEFAULT_CLICK_HOLD_MS).min(MAX_HOLD_MS));
    let mut steps = vec![Step { at_ms: 0.0, event: Inject::Cursor(Vec2::new(x, y)) }];
    let mut at = SETTLE_MS;
    for _ in 0..clicks.unwrap_or(1).clamp(1, 2) {
        steps.push(Step { at_ms: at, event: Inject::Button { button, state: ButtonState::Pressed } });
        steps.push(Step { at_ms: at + hold, event: Inject::Button { button, state: ButtonState::Released } });
        at += hold + CLICK_GAP_MS;
    }
    steps
}

/// Presses at `from`, moves in `steps` increments to `to`, releases.
pub fn plan_mouse_drag(from: [f32; 2], to: [f32; 2], button: Option<MouseButtonName>, steps: Option<u32>) -> Vec<Step> {
    let button = mouse_button(button);
    let n = steps.unwrap_or(12).clamp(1, MAX_DRAG_STEPS);
    let (a, b) = (Vec2::from(from), Vec2::from(to));
    let mut out = vec![
        Step { at_ms: 0.0, event: Inject::Cursor(a) },
        Step { at_ms: SETTLE_MS, event: Inject::Button { button, state: ButtonState::Pressed } },
    ];
    for k in 1..=n {
        let t = f64::from(k) / f64::from(n);
        out.push(Step {
            at_ms: SETTLE_MS + DRAG_STEP_MS * k as f64,
            event: Inject::Cursor(a.lerp(b, t as f32)),
        });
    }
    let end = SETTLE_MS + DRAG_STEP_MS * (f64::from(n) + 1.0);
    out.push(Step { at_ms: end, event: Inject::Button { button, state: ButtonState::Released } });
    out
}

/// Wheel scroll, after moving to an optional position. Positive `y` scrolls up.
pub fn plan_scroll(at: Option<[f32; 2]>, x: Option<f32>, y: f32) -> Vec<Step> {
    let mut steps = Vec::new();
    if let Some(p) = at {
        steps.push(Step { at_ms: 0.0, event: Inject::Cursor(Vec2::from(p)) });
    }
    steps.push(Step { at_ms: SETTLE_MS, event: Inject::Wheel { x: x.unwrap_or(0.0), y } });
    steps
}

/// Writes the events that are due. Keeps the window's cursor position in step
/// with pointer events: the game reads Window::cursor_position.
pub fn deliver_input(
    time: Res<Time<Real>>,
    mut queue: ResMut<InputQueue>,
    mut windows: Query<(Entity, &mut Window), With<bevy::window::PrimaryWindow>>,
    mut keys: MessageWriter<KeyboardInput>,
    mut buttons: MessageWriter<MouseButtonInput>,
    mut cursor: MessageWriter<CursorMoved>,
    mut wheel: MessageWriter<MouseWheel>,
) {
    if queue.is_idle() {
        return;
    }
    let Ok((entity, mut window)) = windows.single_mut() else { return };
    let due = queue.due(time.elapsed_secs_f64());
    if !due.is_empty() {
        debug!("input: delivering {} event(s)", due.len());
    }
    for event in due {
        match event {
            Inject::Key { code, logical, text, state } => {
                keys.write(KeyboardInput {
                    key_code: code,
                    logical_key: logical,
                    state,
                    text: text.map(Into::into),
                    repeat: false,
                    window: entity,
                });
            }
            Inject::Cursor(pos) => {
                let delta = window.cursor_position().map(|old| pos - old);
                window.set_cursor_position(Some(pos));
                cursor.write(CursorMoved { window: entity, position: pos, delta });
            }
            Inject::Button { button, state } => {
                buttons.write(MouseButtonInput { button, state, window: entity });
            }
            Inject::Wheel { x, y } => {
                wheel.write(MouseWheel { unit: MouseScrollUnit::Line, x, y, window: entity, phase: TouchPhase::Moved });
            }
        }
    }
}

#[cfg(test)]
mod tests;
