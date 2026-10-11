//! The speech hold: while an NPC line or a bark is being spoken, a command waits
//! for it to finish, instead of running over it or being refused. remote.rs reads
//! the speech state and passes it in; the rules here are plain data, tested
//! without a window.
//!
//! A held call is answered once the speech ends, and its reply says how long it
//! waited. A call still held past the line's own length plus [`HOLD_PAD`] is
//! refused, so no call waits forever. Only stop (it must still interrupt), quit and
//! game_status run during speech; every other call waits, reads included.
//!
//! Waiting calls queue in order behind the one ahead of them. A dialog call
//! (choose_reply, continue_dialog) answers once the next line has been spoken and its
//! replies are up, or the conversation has ended; its wait is the same kind of [`Wait`].

use k2_world::control::Command;

#[cfg(test)]
mod tests;

/// Seconds a held call may wait past the line's own length.
pub const HOLD_PAD: f64 = 2.0;

/// The speech in progress: who is speaking, the seconds left in the line, and
/// the line's measured length (its decoded voice-over, or its text time).
#[derive(Debug, Clone, PartialEq)]
pub struct Speech {
    pub speaker: String,
    pub left: f32,
    pub total: f32,
}

/// Seconds left in a line `total` long that has been spoken for `elapsed`
/// seconds; None once it is over.
pub fn left_of(total: f32, elapsed: f32) -> Option<f32> {
    (elapsed < total).then(|| total - elapsed)
}

/// The interim frame's message for a held call: it runs when the speech ends,
/// and waits at most `wait_s` seconds.
pub fn notice(speaker: &str, wait_s: f64) -> String {
    format!("held while {speaker} speaks; the call runs when it ends (up to {wait_s:.1} s)")
}

/// Whether a command runs while someone speaks: stop (it interrupts), quit and
/// game_status. Every other call waits for the speech to end, reads included.
pub fn exempt(cmd: &Command) -> bool {
    matches!(cmd, Command::Stop | Command::Quit | Command::Info)
}

/// Whether a command waits for the speech to end.
pub fn must_wait(cmd: &Command, speech: Option<&Speech>) -> bool {
    speech.is_some() && !exempt(cmd)
}

/// The real-time instant (seconds) past which a call held on this speech is refused.
pub fn cap(now: f64, speech: &Speech) -> f64 {
    now + f64::from(speech.left) + HOLD_PAD
}

/// The start of a reply to a call that waited, for example "waited 3.2 s for Kreia to finish speaking".
pub fn wait_note(speaker: &str, waited: f64) -> String {
    format!("waited {waited:.1} s for {speaker} to finish speaking")
}

/// The refusal for a call held past its cap.
pub fn overran(speaker: &str, waited: f64) -> String {
    format!("refused: {speaker} is still speaking after {waited:.1} s; the call was not run")
}

/// The interim frame's message for a dialog call: its answer follows once the next
/// line has been spoken and its replies are up, and waits at most `wait_s` seconds.
pub fn dialog_notice(speaker: &str, wait_s: f64) -> String {
    format!("held until {speaker} finishes and the replies are up; the answer follows (up to {wait_s:.1} s)")
}

/// The note on a dialog answer that did not wait for the conversation to settle.
pub fn unsettled(waited: f64) -> String {
    format!("stopped waiting after {waited:.1} s; the conversation may still be moving")
}

/// A call waiting in order: who it waits for, and until when at most.
/// `speaking` is set while the cap follows the line on screen (see [`Wait::track`]).
#[derive(Debug, Clone, PartialEq)]
pub struct Wait {
    pub started: f64,
    pub cap: f64,
    pub speaker: String,
    speaking: bool,
}

impl Wait {
    /// A wait that starts now, for `speaker`, and ends at `cap` unless a line follows.
    pub fn new(now: f64, speaker: &str, cap: f64) -> Self {
        Self { started: now, cap, speaker: speaker.to_owned(), speaking: false }
    }

    /// Follows the speech on screen. A line that starts sets the cap to its own length
    /// plus [`HOLD_PAD`] in real time, and the cap stays while that line is spoken, so
    /// a paused game (which freezes the line) runs past it and is refused.
    pub fn track(&mut self, now: f64, speech: Option<&Speech>) {
        match speech {
            Some(s) if !self.speaking => {
                self.cap = cap(now, s);
                self.speaking = true;
            }
            Some(_) => {}
            None => self.speaking = false,
        }
    }

    /// Whether the cap has passed.
    pub fn overdue(&self, now: f64) -> bool {
        now > self.cap
    }
}
