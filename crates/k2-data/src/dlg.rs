//! Dialog trees (DLG), including the K2 additions: a second action script
//! (Script2) and second condition (Active2) per node/link, five int
//! parameters plus a string parameter for each, per-link negation flags
//! (Not/Not2) and a Logic field combining the two conditions.

use k2_res::Resources;
use rakata_core::ResourceType;
use rakata_formats::gff::GffStruct;

use crate::gff::{GffExt, LocString};
use crate::{Result, read_gff};

/// An action script call on a node (Script / Script2).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ScriptCall {
    pub resref: String,
    pub params: [i32; 5],
    pub string_param: String,
}

/// A conditional script on a link (Active / Active2).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Condition {
    pub resref: String,
    pub params: [i32; 5],
    pub string_param: String,
    /// Negate the script's result.
    pub not: bool,
}

/// Link from one node to another list (StartingList, RepliesList, EntriesList).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Link {
    /// Index into the target list (entries for starting and reply links,
    /// replies for entry links).
    pub index: usize,
    pub active: Option<Condition>,
    pub active2: Option<Condition>,
    /// How active and active2 combine: 0 AND, 1 OR (community docs;
    /// unverified against the engine).
    pub logic: i32,
    /// True for a link that points at a node owned elsewhere in the tree
    /// (an editor "link"), as opposed to the node's own child.
    pub is_child: bool,
    pub comment: String,
}

impl Link {
    /// Evaluate this link's conditions with a script evaluator. active2 runs
    /// only when active does not decide the link (AND with a false active, OR
    /// with a true one), as the engine short-circuits.
    pub fn passes(&self, eval: &mut impl FnMut(&Condition) -> bool) -> bool {
        let mut run = |c: &Condition| eval(c) != c.not;
        let Some(a) = self.active.as_ref().map(&mut run) else {
            return self.active2.as_ref().is_none_or(|c| run(c));
        };
        match &self.active2 {
            Some(c) if a != (self.logic == 1) => run(c),
            _ => a,
        }
    }

    /// Whether the link passes, given the result of each present condition
    /// (`a` for `active`, `b` for `active2`, each after its `not` is applied).
    /// The start diagnostics (explain_start) evaluate both conditions and use
    /// this; it gives the same answer as `passes`.
    pub fn passes_given(&self, a: Option<bool>, b: Option<bool>) -> bool {
        match (a, b) {
            (None, None) => true,
            (Some(a), None) | (None, Some(a)) => a,
            (Some(a), Some(b)) => {
                if self.logic == 1 {
                    a || b
                } else {
                    a && b
                }
            }
        }
    }
}

/// One participant animation on a node.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NodeAnim {
    pub participant: String,
    pub animation: u32,
}

/// An entry (NPC line) or reply (player line).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Node {
    pub text: LocString,
    /// Tag of the speaking object; empty means the conversation owner.
    pub speaker: String,
    pub listener: String,
    pub script: Option<ScriptCall>,
    pub script2: Option<ScriptCall>,
    pub vo_resref: Option<String>,
    pub sound: Option<String>,
    /// Milliseconds; None means the default (VO length or text-based).
    pub delay: Option<u32>,
    pub camera_angle: u32,
    pub camera_id: Option<i32>,
    pub camera_animation: Option<u32>,
    pub cam_vid_effect: i32,
    pub fade_type: u32,
    pub emotion: i32,
    /// AlienRaceNode: alienvo.2da row of the speaker. Only used for the
    /// alien voice when the line has no voice file; 0 for humans.
    pub alien_race: u32,
    pub facial_anim: i32,
    pub anims: Vec<NodeAnim>,
    pub quest: String,
    pub plot_index: i32,
    pub plot_xp_percentage: f32,
    pub node_unskippable: bool,
    pub wait_flags: u32,
    pub node_id: i32,
    pub comment: String,
    pub links: Vec<Link>,
}

/// A stunt model override for a participant.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Stunt {
    pub participant: String,
    pub model: String,
}

/// A full dialog tree.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Dialog {
    pub resref: String,
    pub entries: Vec<Node>,
    pub replies: Vec<Node>,
    pub starting: Vec<Link>,
    pub skippable: bool,
    pub conversation_type: i32,
    pub computer_type: u32,
    pub camera_model: Option<String>,
    pub ambient_track: Option<String>,
    pub end_conversation: Option<String>,
    pub end_conversation_abort: Option<String>,
    pub delay_entry: u32,
    pub delay_reply: u32,
    pub vo_id: String,
    pub animated_cut: bool,
    pub unequip_items: bool,
    pub unequip_h_item: bool,
    pub old_hit_check: bool,
    pub stunts: Vec<Stunt>,
    /// AlienRaceOwner: alienvo.2da row of the conversation owner.
    pub alien_race_owner: u32,
}

fn params(s: &GffStruct, prefix: &str, suffix: &str) -> [i32; 5] {
    std::array::from_fn(|i| s.int_or(&format!("{prefix}{}{suffix}", i + 1), 0))
}

fn script_call(s: &GffStruct, label: &str, suffix: &str, str_label: &str) -> Option<ScriptCall> {
    s.resref(label).map(|resref| ScriptCall {
        resref,
        params: params(s, "ActionParam", suffix),
        string_param: s.string_or_empty(str_label),
    })
}

fn condition(s: &GffStruct, label: &str, suffix: &str, str_label: &str, not_label: &str) -> Option<Condition> {
    s.resref(label).map(|resref| Condition {
        resref,
        params: params(s, "Param", suffix),
        string_param: s.string_or_empty(str_label),
        not: s.flag(not_label),
    })
}

fn link(s: &GffStruct) -> Link {
    Link {
        index: s.uint("Index").unwrap_or(0) as usize,
        active: condition(s, "Active", "", "ParamStrA", "Not"),
        active2: condition(s, "Active2", "b", "ParamStrB", "Not2"),
        logic: s.int_or("Logic", 0),
        is_child: s.flag("IsChild"),
        comment: s.string_or_empty("LinkComment"),
    }
}

fn node(s: &GffStruct, links_label: &str) -> Node {
    let delay = s.uint("Delay").filter(|&d| d != u32::MAX);
    Node {
        text: s.locstring("Text"),
        speaker: s.string_or_empty("Speaker"),
        listener: s.string_or_empty("Listener"),
        script: script_call(s, "Script", "", "ActionParamStrA"),
        script2: script_call(s, "Script2", "b", "ActionParamStrB"),
        vo_resref: s.resref("VO_ResRef"),
        sound: s.resref("Sound"),
        delay,
        camera_angle: s.uint("CameraAngle").unwrap_or(0),
        camera_id: s.int("CameraID"),
        camera_animation: s.uint("CameraAnimation"),
        cam_vid_effect: s.int_or("CamVidEffect", -1),
        fade_type: s.uint("FadeType").unwrap_or(0),
        emotion: s.int_or("Emotion", 0),
        alien_race: s.uint("AlienRaceNode").unwrap_or(0),
        facial_anim: s.int_or("FacialAnim", 0),
        anims: s
            .list("AnimList")
            .iter()
            .map(|a| NodeAnim { participant: a.string_or_empty("Participant"), animation: a.uint("Animation").unwrap_or(0) })
            .collect(),
        quest: s.string_or_empty("Quest"),
        plot_index: s.int_or("PlotIndex", -1),
        plot_xp_percentage: s.float_or("PlotXPPercentage", 1.0),
        node_unskippable: s.flag("NodeUnskippable"),
        wait_flags: s.uint("WaitFlags").unwrap_or(0),
        node_id: s.int_or("NodeID", -1),
        comment: s.string_or_empty("Comment"),
        links: s.list(links_label).iter().map(link).collect(),
    }
}

impl Dialog {
    pub fn load(res: &Resources, resref: &str) -> Result<Self> {
        Ok(Self::from_gff(&read_gff(res, resref, ResourceType::Dlg)?.root, resref))
    }

    pub fn from_gff(s: &GffStruct, resref: &str) -> Self {
        Self {
            resref: resref.to_ascii_lowercase(),
            entries: s.list("EntryList").iter().map(|e| node(e, "RepliesList")).collect(),
            replies: s.list("ReplyList").iter().map(|e| node(e, "EntriesList")).collect(),
            starting: s.list("StartingList").iter().map(link).collect(),
            skippable: s.flag("Skippable"),
            conversation_type: s.int_or("ConversationType", 0),
            computer_type: s.uint("ComputerType").unwrap_or(0),
            camera_model: s.resref("CameraModel"),
            ambient_track: s.resref("AmbientTrack"),
            end_conversation: s.resref("EndConversation"),
            end_conversation_abort: s.resref("EndConverAbort"),
            delay_entry: s.uint("DelayEntry").unwrap_or(0),
            delay_reply: s.uint("DelayReply").unwrap_or(0),
            vo_id: s.string_or_empty("VO_ID"),
            animated_cut: s.flag("AnimatedCut"),
            unequip_items: s.flag("UnequipItems"),
            unequip_h_item: s.flag("UnequipHItem"),
            old_hit_check: s.flag("OldHitCheck"),
            stunts: s
                .list("StuntList")
                .iter()
                .filter_map(|t| t.resref("StuntModel").map(|m| Stunt { participant: t.string_or_empty("Participant"), model: m }))
                .collect(),
            alien_race_owner: s.uint("AlienRaceOwner").unwrap_or(0),
        }
    }

    /// First starting entry whose conditions pass.
    pub fn start(&self, eval: &mut impl FnMut(&Condition) -> bool) -> Option<usize> {
        self.starting.iter().find(|l| l.passes(eval)).map(|l| l.index).filter(|&i| i < self.entries.len())
    }

    /// Replies available after an entry, as reply indices in order.
    pub fn replies_for(&self, entry: usize, eval: &mut impl FnMut(&Condition) -> bool) -> Vec<usize> {
        let Some(e) = self.entries.get(entry) else { return Vec::new() };
        e.links.iter().filter(|l| l.passes(eval)).map(|l| l.index).filter(|&i| i < self.replies.len()).collect()
    }

    /// The entry that follows a reply, if any (None ends the conversation).
    pub fn next_entry(&self, reply: usize, eval: &mut impl FnMut(&Condition) -> bool) -> Option<usize> {
        let r = self.replies.get(reply)?;
        r.links.iter().find(|l| l.passes(eval)).map(|l| l.index).filter(|&i| i < self.entries.len())
    }

    /// True when `entry` is a bare line: it has no reply links at all (not
    /// even a single [Continue]), so nothing follows it. Such a line is played
    /// as a bark (no conversation). Any reply link makes a conversation, and
    /// so do cutscene lines: a camera model, AnimatedCut, a node fade, or a
    /// node that runs an animated camera (CameraAngle 4, dialog.rs
    /// `cutscene_line`).
    pub fn is_one_liner(&self, entry: usize) -> bool {
        let Some(node) = self.entries.get(entry) else { return false };
        node.links.is_empty()
            && !self.animated_cut
            && self.camera_model.is_none()
            && node.fade_type == 0
            && node.camera_angle != 4
    }

    /// Validate link indices; returns human-readable problems.
    pub fn check(&self) -> Vec<String> {
        let mut out = Vec::new();
        for l in &self.starting {
            if l.index >= self.entries.len() {
                out.push(format!("starting link -> entry {} out of range", l.index));
            }
        }
        for (i, e) in self.entries.iter().enumerate() {
            for l in &e.links {
                if l.index >= self.replies.len() {
                    out.push(format!("entry {i} -> reply {} out of range", l.index));
                }
            }
        }
        for (i, r) in self.replies.iter().enumerate() {
            for l in &r.links {
                if l.index >= self.entries.len() {
                    out.push(format!("reply {i} -> entry {} out of range", l.index));
                }
            }
        }
        out
    }
}
