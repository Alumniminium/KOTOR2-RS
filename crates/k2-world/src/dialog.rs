//! Dialog runtime: a state machine over a k2-data [`Dialog`].
//!
//! Flow: start -> first starting link whose conditions pass -> entry (NPC
//! line; its scripts run on entry) -> the passing reply links are offered ->
//! [`World::choose_reply`] runs the reply's scripts and follows its first
//! passing link to the next entry, or ends the conversation.

use std::sync::Arc;

use k2_data::dlg::{Condition, Node, ScriptCall};
use k2_data::Dialog;
use k2_script::{ObjectId, OBJECT_INVALID};

use crate::object::bearing_towards;
use crate::{ObjectKind, ScriptCtx, World, WorldEvent};

/// `local_bools` slot holding SetLockOrientationInDialog. Negative, so no
/// script's local index can collide with it, and it is saved with the object.
pub const LOCK_ORIENTATION_LOCAL: i32 = -1001;

#[derive(Debug, Clone)]
pub struct Conversation {
    pub dialog: Arc<Dialog>,
    pub owner: ObjectId,
    pub pc: ObjectId,
    /// Current entry (NPC line).
    pub entry: Option<usize>,
    /// Indices into `dialog.replies` currently offered.
    pub replies: Vec<usize>,
    /// Set by ActionPauseConversation until ActionResumeConversation.
    pub paused: bool,
    /// Seconds the current line has been up.
    pub shown_for: f32,
}

/// What the UI shows.
#[derive(Debug, Clone, PartialEq)]
pub struct ConversationView {
    pub dialog: String,
    pub owner: ObjectId,
    pub speaker: ObjectId,
    pub listener: ObjectId,
    pub entry: usize,
    pub text: String,
    /// Voice-over resref (streamvoice), then sound resref.
    pub vo: Option<String>,
    pub sound: Option<String>,
    /// Alien voice set (alienvo.2da row) for a line with no voice file of
    /// its own; 0 for humans. The node's AlienRaceNode, else the owner's
    /// AlienRaceOwner when the owner is the speaker.
    pub alien_race: u32,
    /// Node Emotion: emotion.2da row, which names the alienvo column
    /// (Comment_Generic_Short, Silence, ...).
    pub emotion: i32,
    /// Raw node Delay; None = default timing.
    pub delay: Option<u32>,
    /// Node FadeType: 3 fades in from black at the start of the line, 4
    /// fades out to black at its end (0 none).
    pub fade_type: u32,
    /// Node NodeUnskippable: Space/Enter must not skip the line.
    pub unskippable: bool,
    pub paused: bool,
    pub replies: Vec<ReplyView>,
    /// A door's own line with no real choice: the client shows it as a modal
    /// message box (text and OK), with no camera change and no reply list. See
    /// [`is_message_box`].
    pub boxed: bool,
}

/// Whether a line is a door's message box: the owner is a door, the line is
/// the door's own (no speaker, or the door itself), it has text, and no reply
/// is a real choice (each reply is empty, the "[Continue]" placeholder). A line
/// by a named speaker stays a conversation, whatever its replies.
pub fn is_message_box(owner_is_door: bool, own_line: bool, text: &str, replies: &[ReplyView]) -> bool {
    owner_is_door && own_line && !text.trim().is_empty() && replies.iter().all(|r| r.text.trim().is_empty())
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReplyView {
    /// Index into `dialog.replies`.
    pub reply: usize,
    /// Empty text means "[Continue]".
    pub text: String,
}

impl World {
    /// Starts a conversation owned by `owner` with `pc`. Returns false when
    /// the dialog does not exist, no starting link passes, or a conversation
    /// is already running. A bare line (no reply links) returns true without
    /// starting a conversation: it is spoken as a bark instead.
    pub fn start_conversation(&mut self, owner: ObjectId, pc: ObjectId, dialog: &str) -> bool {
        self.start_conversation_at(owner, pc, dialog, None)
    }

    /// `start_conversation` with ActionStartConversation's bark position
    /// (nBarkX / nBarkY), used if the conversation is a bark.
    pub fn start_conversation_at(&mut self, owner: ObjectId, pc: ObjectId, dialog: &str, bark: Option<(i32, i32)>) -> bool {
        if self.conversation.is_some() || dialog.is_empty() {
            return false;
        }
        let Ok(dlg) = Dialog::load(&self.res, dialog) else {
            self.warnings.push(format!("dialog {dialog} not found"));
            return false;
        };
        let dlg = Arc::new(dlg);
        let start = dlg.start(&mut |c: &Condition| self.eval_condition(c, owner, pc));
        let Some(entry) = start else { return false };
        // A door's own line with text is a message box, which only a conversation can
        // be: so a bare door line is not a bark (see is_message_box).
        let door_line = self.get(owner).is_some_and(|o| o.kind == ObjectKind::Door)
            && dlg.entries.get(entry).is_some_and(|n| !self.resolve(&n.text).trim().is_empty());
        if dlg.is_one_liner(entry) && !door_line {
            self.speak_one_liner(owner, pc, &dlg, entry, bark);
            return true;
        }
        if self.trace {
            println!("  conversation {dialog} owner {} pc {}", self.describe(owner), self.describe(pc));
        }
        let staged = dlg.entries.get(entry).is_some_and(|n| Self::scripted_staging(&dlg, n));
        self.conversation =
            Some(Conversation { dialog: dlg, owner, pc, entry: None, replies: Vec::new(), paused: false, shown_for: 0.0 });
        self.emit(WorldEvent::ConversationStarted { owner, dialog: dialog.to_ascii_lowercase() });
        if !staged {
            self.face_each_other(pc, owner);
        }
        self.enter_entry(entry);
        true
    }

    /// Animated cutscenes stage their actors through scripts; the engine
    /// does not turn anyone: whole dialogs flagged AnimatedCut, and the
    /// lines of a dialog with a camera model that run its animated camera
    /// (CameraAngle 4). The rest of such a dialog (101kreia after Kreia is
    /// up) faces the speakers as usual.
    fn scripted_staging(dlg: &Dialog, node: &Node) -> bool {
        dlg.animated_cut || (dlg.camera_model.is_some() && node.camera_angle == 4)
    }

    /// A cutscene line for stealth (nwscript 819): whole AnimatedCut dialogs,
    /// and any line that runs an animated camera (CameraAngle 4), with or
    /// without a camera model. This does not decide facing (scripted_staging).
    fn cutscene_line(dlg: &Dialog, node: &Node) -> bool {
        dlg.animated_cut || node.camera_angle == 4
    }

    /// SpeakOneLinerConversation: `dialog` (default: the speaker's own
    /// conversation) says its first passing line now, as a bark.
    pub(crate) fn speak_one_liner_conversation(&mut self, speaker: ObjectId, dialog: &str) {
        let Ok(dlg) = Dialog::load(&self.res, dialog) else {
            self.warnings.push(format!("dialog {dialog} not found"));
            return;
        };
        let dlg = Arc::new(dlg);
        let pc = self.party_leader();
        if let Some(entry) = dlg.start(&mut |c: &Condition| self.eval_condition(c, speaker, pc)) {
            self.speak_line(speaker, pc, &dlg, entry, false, None);
        }
    }

    /// Why `dialog` does not start for `owner` and `pc` right now, for the
    /// diagnostics (talkall): the open conversation, or the starting links in
    /// order with each condition evaluated once, as `start` picks them (the
    /// first passing link; its index must be in range).
    pub fn explain_start(&mut self, owner: ObjectId, pc: ObjectId, dialog: &str) -> String {
        if let Some(c) = &self.conversation {
            return format!("conversation open (owner {} dialog {})", c.owner, c.dialog.resref);
        }
        let Ok(dlg) = Dialog::load(&self.res, dialog) else {
            return format!("dialog {dialog} not found");
        };
        if dlg.starting.is_empty() {
            return format!("dialog {dialog} has no starting links");
        }
        let label = |c: &Condition, v: bool| format!("{}{}={v}", if c.not { "!" } else { "" }, c.resref);
        let mut parts = Vec::new();
        let mut verdict = "none passes".to_owned();
        for link in &dlg.starting {
            let mut conds = Vec::new();
            let a = link.active.as_ref().map(|c| {
                let v = self.eval_condition(c, owner, pc) != c.not;
                conds.push(label(c, v));
                v
            });
            // active2 runs only when active does not decide the link, as `passes` does.
            let decided = link.active2.is_some() && a.is_some_and(|a| a == (link.logic == 1));
            let b = match &link.active2 {
                Some(c) if decided => {
                    conds.push(format!("{}{}=not evaluated", if c.not { "!" } else { "" }, c.resref));
                    None
                }
                Some(c) => {
                    let v = self.eval_condition(c, owner, pc) != c.not;
                    conds.push(label(c, v));
                    Some(v)
                }
                None => None,
            };
            let passes = link.passes_given(a, b);
            parts.push(format!("E{} [{}] {}", link.index, conds.join(" "), if passes { "passes" } else { "fails" }));
            if passes {
                verdict = if link.index < dlg.entries.len() { format!("starts E{}", link.index) } else { format!("E{} out of range", link.index) };
                break;
            }
        }
        format!("start {}; {verdict}", parts.join("; "))
    }

    /// A one-liner conversation (see `Dialog::is_one_liner`) is a bark: its
    /// entry scripts run as in a conversation, the line goes out as
    /// `WorldEvent::BarkLine`, and the end-of-dialogue scripts follow. No
    /// conversation state is kept, nobody turns, and there is no camera or
    /// pause. Non-player barks never change facing.
    fn speak_one_liner(&mut self, owner: ObjectId, pc: ObjectId, dlg: &Arc<Dialog>, entry: usize, bark: Option<(i32, i32)>) {
        self.speak_line(owner, pc, dlg, entry, true, bark);
    }

    /// Runs a bare line's scripts and emits it as a bark. `ends` marks a bare
    /// line started as a bark conversation, which also runs the dialog's
    /// end-of-dialogue scripts; SpeakOneLinerConversation does not.
    fn speak_line(&mut self, owner: ObjectId, pc: ObjectId, dlg: &Arc<Dialog>, entry: usize, ends: bool, pos: Option<(i32, i32)>) {
        let Some(node) = dlg.entries.get(entry) else { return };
        *self.barks_by.entry(owner).or_default() += 1;
        self.run_node_scripts(node, owner, pc);
        self.pump();
        let speaker = self.named_speaker(node, owner).unwrap_or(owner);
        let text = self.substitute(&self.resolve(&node.text));
        if !text.trim().is_empty() {
            let alien_race = alien_row(node.alien_race, dlg.alien_race_owner, self.named_speaker(node, owner), owner);
            let vo = node.vo_resref.clone().filter(|s| !s.is_empty());
            let sound = node.sound.clone().filter(|s| !s.is_empty());
            self.emit(WorldEvent::BarkLine {
                speaker,
                dialog: dlg.resref.clone(),
                text,
                vo,
                sound,
                alien_race,
                emotion: node.emotion,
                delay: node.delay,
                pos,
            });
        }
        if ends {
            self.run_end_scripts(dlg, owner, pc, false);
            self.pump();
        }
    }

    /// Turns two conversation participants toward each other. Only living
    /// creatures turn (placeables and doors never do), not NotReorienting
    /// creatures, and not while SetLockOrientationInDialog holds them. Bevy
    /// smooths the turn.
    fn face_each_other(&mut self, a: ObjectId, b: ObjectId) {
        if a == b || a == OBJECT_INVALID || b == OBJECT_INVALID {
            return;
        }
        let (Some(pa), Some(pb)) = (self.get(a).map(|o| o.position), self.get(b).map(|o| o.position)) else { return };
        for (me, other) in [(a, pb), (b, pa)] {
            let Some(o) = self.get_mut(me) else { continue };
            let turns = o.kind == ObjectKind::Creature
                && o.turns_in_dialog()
                && !o.local_bools.get(&LOCK_ORIENTATION_LOCAL).copied().unwrap_or(false);
            if turns && let Some(bearing) = bearing_towards(o.position, other) {
                o.bearing = bearing;
            }
        }
    }

    fn eval_condition(&mut self, c: &Condition, owner: ObjectId, pc: ObjectId) -> bool {
        let ctx = ScriptCtx { params: c.params, string_param: c.string_param.clone(), last_speaker: pc, ..Default::default() };
        self.run_conditional(&c.resref, owner, ctx)
    }

    fn run_node_scripts(&mut self, node: &Node, owner: ObjectId, pc: ObjectId) {
        for call in [&node.script, &node.script2].into_iter().flatten() {
            self.run_call(call, owner, pc);
        }
    }

    fn run_call(&mut self, call: &ScriptCall, owner: ObjectId, pc: ObjectId) {
        if call.resref.is_empty() {
            return;
        }
        let ctx = ScriptCtx { params: call.params, string_param: call.string_param.clone(), last_speaker: pc, ..Default::default() };
        self.run_script(&call.resref, owner, ctx);
    }

    fn enter_entry(&mut self, entry: usize) {
        let Some(c) = &self.conversation else { return };
        let (dlg, owner, pc) = (c.dialog.clone(), c.owner, c.pc);
        let Some(node) = dlg.entries.get(entry) else {
            self.end_conversation(false);
            return;
        };
        if let Some(c) = &mut self.conversation {
            c.entry = Some(entry);
            c.replies.clear();
            c.shown_for = 0.0;
        }
        // Every cutscene line ends stealth for both participants (nwscript
        // 819), not only the start of the conversation.
        if Self::cutscene_line(&dlg, node) {
            self.end_stealth_for_dialog(owner, pc);
        }
        // Speaker and listener face each other before the node's scripts run,
        // so a script that sets facing itself still wins.
        if !Self::scripted_staging(&dlg, node) {
            let speaker = self.named_speaker(node, owner).unwrap_or(owner);
            let listener = if node.listener.is_empty() { pc } else { self.by_tag(&node.listener, 0).unwrap_or(pc) };
            self.face_each_other(speaker, listener);
        }
        self.run_node_scripts(node, owner, pc);
        self.pump();
        // A script may have ended or replaced the conversation.
        if !self.conversation.as_ref().is_some_and(|c| Arc::ptr_eq(&c.dialog, &dlg) && c.entry == Some(entry)) {
            return;
        }
        let replies = dlg.replies_for(entry, &mut |c: &Condition| self.eval_condition(c, owner, pc));
        if let Some(c) = &mut self.conversation {
            c.replies = replies;
        }
    }

    /// The object a line's speaker tag names. An empty tag, or the owner's own
    /// tag, is the owner: by_tag returns the first object with a tag, which may
    /// be an earlier object that shares the owner's tag. Otherwise the first
    /// object with the tag. None when nothing matches.
    fn named_speaker(&self, node: &Node, owner: ObjectId) -> Option<ObjectId> {
        if node.speaker.is_empty() || self.get(owner).is_some_and(|o| o.tag.eq_ignore_ascii_case(&node.speaker)) {
            Some(owner)
        } else {
            self.by_tag(&node.speaker, 0)
        }
    }

    /// The current line and replies, if a conversation is running.
    pub fn conversation_view(&self) -> Option<ConversationView> {
        let c = self.conversation.as_ref()?;
        let entry = c.entry?;
        let node = c.dialog.entries.get(entry)?;
        let speaker = self.named_speaker(node, c.owner).unwrap_or(c.owner);
        let listener = if node.listener.is_empty() { c.pc } else { self.by_tag(&node.listener, 0).unwrap_or(c.pc) };
        let alien_race = alien_row(node.alien_race, c.dialog.alien_race_owner, self.named_speaker(node, c.owner), c.owner);
        let text = self.substitute(&self.resolve(&node.text));
        let replies: Vec<ReplyView> = c
            .replies
            .iter()
            .filter_map(|&r| {
                let n = c.dialog.replies.get(r)?;
                Some(ReplyView { reply: r, text: self.substitute(&self.resolve(&n.text)) })
            })
            .collect();
        let owner_is_door = self.get(c.owner).is_some_and(|o| o.kind == ObjectKind::Door);
        let own_line = node.speaker.is_empty() || self.get(c.owner).is_some_and(|o| o.tag.eq_ignore_ascii_case(&node.speaker));
        let boxed = is_message_box(owner_is_door, own_line, &text, &replies);
        Some(ConversationView {
            dialog: c.dialog.resref.clone(),
            owner: c.owner,
            speaker,
            listener,
            entry,
            text,
            vo: node.vo_resref.clone().filter(|s| !s.is_empty()),
            sound: node.sound.clone().filter(|s| !s.is_empty()),
            alien_race,
            emotion: node.emotion,
            delay: node.delay,
            fade_type: node.fade_type,
            unskippable: node.node_unskippable,
            paused: c.paused,
            replies,
            boxed,
        })
    }

    /// Picks the `i`-th offered reply.
    pub fn choose_reply(&mut self, i: usize) {
        // For the dialog history log (sheet.rs).
        if let Some(t) = self.conversation_view().and_then(|v| v.replies.get(i).map(|r| r.text.clone()))
            && !t.is_empty()
        {
            self.sheet.chosen_replies.push(t);
        }
        let Some(c) = &self.conversation else { return };
        let (dlg, owner, pc) = (c.dialog.clone(), c.owner, c.pc);
        let Some(&reply) = c.replies.get(i) else {
            if c.replies.is_empty() {
                self.end_conversation(false);
            }
            return;
        };
        if let Some(node) = dlg.replies.get(reply) {
            self.run_node_scripts(node, owner, pc);
            self.pump();
        }
        if !self.conversation.as_ref().is_some_and(|c| Arc::ptr_eq(&c.dialog, &dlg)) {
            return;
        }
        match dlg.next_entry(reply, &mut |c: &Condition| self.eval_condition(c, owner, pc)) {
            Some(e) => self.enter_entry(e),
            None => self.end_conversation(false),
        }
    }

    /// Continues past a line: picks the only reply when it is a bare
    /// "[Continue]", ends the conversation when there are no replies.
    /// Does nothing when real choices are offered.
    pub fn advance_conversation(&mut self) {
        let Some(view) = self.conversation_view() else {
            if self.conversation.is_some() {
                self.end_conversation(false);
            }
            return;
        };
        match view.replies.as_slice() {
            [] => self.end_conversation(false),
            [only] if only.text.trim().is_empty() => self.choose_reply(0),
            _ => {}
        }
    }

    /// Ends the running conversation, firing its end scripts.
    pub fn end_conversation(&mut self, aborted: bool) {
        let Some(c) = self.conversation.take() else { return };
        self.run_end_scripts(&c.dialog, c.owner, c.pc, aborted);
        self.emit(WorldEvent::ConversationEnded { owner: c.owner, aborted });
        self.pump();
    }

    /// The dialog's end script and the owner's end-of-dialogue event; shared
    /// by conversations and barks.
    fn run_end_scripts(&mut self, dlg: &Dialog, owner: ObjectId, pc: ObjectId, aborted: bool) {
        let script = if aborted { dlg.end_conversation_abort.clone() } else { dlg.end_conversation.clone() };
        if let Some(s) = script.filter(|s| !s.is_empty()) {
            self.run_script(&s, owner, ScriptCtx { last_speaker: pc, ..Default::default() });
        }
        let label = match self.get(owner).map(|o| o.kind) {
            Some(crate::ObjectKind::Creature) => "ScriptEndDialogu",
            _ => "OnEndDialogue",
        };
        self.queue_event(owner, label, ScriptCtx { last_speaker: pc, ..Default::default() });
    }

    /// Harness/auto mode: picks reply 0 after a short pause.
    pub(crate) fn tick_conversation(&mut self, dt: f32) {
        let Some(c) = &mut self.conversation else { return };
        c.shown_for += dt;
        if !self.auto_dialog || c.paused || c.shown_for < 0.2 {
            return;
        }
        let empty = c.replies.is_empty();
        if self.trace
            && let Some(v) = self.conversation_view()
        {
            println!("    [{}] {}: {}", v.entry, self.describe(v.speaker), v.text);
            for (i, r) in v.replies.iter().enumerate() {
                println!("      {i}) {}", if r.text.is_empty() { "[Continue]" } else { &r.text });
            }
        }
        if empty {
            self.end_conversation(false);
        } else {
            self.choose_reply(0);
        }
    }

    /// Replaces <CUSTOMn>, <FirstName>, <FullName> and similar tokens and
    /// drops {director notes}, which the game never displays.
    pub fn substitute(&self, text: &str) -> String {
        let text = strip_braces(text);
        if !text.contains('<') {
            return text;
        }
        let text = text.as_str();
        let pc_name = self
            .get(self.player)
            .or(self.stashed_pc.as_ref())
            .map(|o| o.name.clone())
            .unwrap_or_default();
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(start) = rest.find('<') {
            out.push_str(&rest[..start]);
            let Some(end) = rest[start..].find('>') else {
                out.push_str(&rest[start..]);
                return out;
            };
            let token = &rest[start + 1..start + end];
            let lower = token.to_ascii_lowercase();
            let rep = if let Some(n) = lower.strip_prefix("custom").and_then(|n| n.parse::<i32>().ok()) {
                Some(self.tokens.get(&n).cloned().unwrap_or_default())
            } else {
                match lower.as_str() {
                    "firstname" | "fullname" | "playername" => Some(pc_name.clone()),
                    _ => None,
                }
            };
            match rep {
                Some(r) => out.push_str(&r),
                None => out.push_str(&rest[start..start + end + 1]),
            }
            rest = &rest[start + end + 1..];
        }
        out.push_str(rest);
        out
    }

    /// Owner of the running conversation, or OBJECT_INVALID.
    pub fn conversation_owner(&self) -> ObjectId {
        self.conversation.as_ref().map_or(OBJECT_INVALID, |c| c.owner)
    }
}

/// Removes {...} spans and the whitespace they leave at the ends.
fn strip_braces(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut depth = 0u32;
    for ch in text.chars() {
        match ch {
            '{' => depth += 1,
            '}' if depth > 0 => depth -= 1,
            _ if depth == 0 => out.push(ch),
            _ => {}
        }
    }
    out.trim().to_owned()
}

/// The alien voice set (alienvo.2da row, 0 for a human voice) of a line. The
/// node's own AlienRaceNode wins. Otherwise the owner's AlienRaceOwner applies
/// only when the owner speaks: `speaker` is the owner for an empty speaker tag
/// or a tag that resolves to it. A named speaker that does not resolve is not
/// the owner.
fn alien_row(node_race: u32, owner_race: u32, speaker: Option<ObjectId>, owner: ObjectId) -> u32 {
    match node_race {
        0 if speaker == Some(owner) => owner_race,
        race => race,
    }
}

#[cfg(test)]
mod tests;
