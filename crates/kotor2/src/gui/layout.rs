//! .gui resources (GFF "GUI ", type 2047): the control tree.
//!
//! The root struct is itself a control (normally a TGuiPanel) whose
//! CONTROLS list holds the children. Child EXTENTs are relative to the
//! root's top-left. See docs/notes/gui.md for the field semantics.

use k2_data::GffExt;
use k2_res::{ResKey, Resources};
use rakata_core::ResourceType;
use rakata_formats::gff::GffStruct;

/// CONTROLTYPE values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Panel,
    Label,
    LabelHilight,
    Button,
    Toggle,
    Slider,
    Scrollbar,
    Progress,
    ListBox,
    Other(i32),
}

impl Kind {
    fn from(v: i32) -> Self {
        match v {
            2 => Kind::Panel,
            4 => Kind::Label,
            5 => Kind::LabelHilight,
            6 => Kind::Button,
            7 => Kind::Toggle,
            8 => Kind::Slider,
            9 => Kind::Scrollbar,
            10 => Kind::Progress,
            11 => Kind::ListBox,
            o => Kind::Other(o),
        }
    }

    pub fn clickable(self) -> bool {
        matches!(self, Kind::Button | Kind::Toggle | Kind::LabelHilight | Kind::Slider)
    }
}

/// BORDER / HILIGHT / SELECTED / HILIGHTSELECTED / PROGRESS: a framed box.
/// The fill sits inside the frame (inset by `dimension`); fill, edges and
/// corners are all tinted by `color`.
// Fields not drawn yet (FILLSTYLE, PULSING, scrollbar art) are parsed so
// screens built on this can use them.
#[allow(dead_code)]
#[derive(Debug, Clone, Default)]
pub struct Border {
    pub corner: Option<String>,
    pub edge: Option<String>,
    pub fill: Option<String>,
    pub fill_style: i32,
    pub dimension: f32,
    pub color: [f32; 3],
    pub pulsing: bool,
}

impl Border {
    fn parse(s: &GffStruct) -> Self {
        Border {
            corner: tex(s, "CORNER"),
            edge: tex(s, "EDGE"),
            fill: tex(s, "FILL"),
            fill_style: s.int_or("FILLSTYLE", 0),
            dimension: s.int_or("DIMENSION", 0) as f32,
            color: s.vec3("COLOR").unwrap_or([1.0; 3]),
            pulsing: s.flag("PULSING"),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.corner.is_none() && self.edge.is_none() && self.fill.is_none()
    }
}

/// A texture resref; "" and "0" mean none.
fn tex(s: &GffStruct, label: &str) -> Option<String> {
    s.resref(label).filter(|r| r != "0")
}

#[allow(dead_code)]
#[derive(Debug, Clone, Default)]
pub struct TextDef {
    pub font: String,
    pub text: String,
    pub strref: Option<u32>,
    /// Bitfield: 1 left, 2 hcenter, 4 right; 8 top, 16 vcenter, 32 bottom.
    pub align: i32,
    pub color: [f32; 3],
    pub pulsing: bool,
}

/// Keyboard neighbours of a control (its MOVETO struct): indices into
/// `GuiDef::controls`, -1 where there is none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MoveTo {
    pub up: i32,
    pub down: i32,
    pub left: i32,
    pub right: i32,
}

impl Default for MoveTo {
    fn default() -> Self {
        Self { up: -1, down: -1, left: -1, right: -1 }
    }
}

impl MoveTo {
    fn map(self, f: impl Fn(i32) -> i32) -> Self {
        Self { up: f(self.up), down: f(self.down), left: f(self.left), right: f(self.right) }
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct Control {
    pub kind: Kind,
    pub tag: String,
    pub id: i32,
    /// left, top, width, height in layout pixels.
    pub rect: [f32; 4],
    pub move_to: MoveTo,
    pub border: Option<Border>,
    pub hilight: Option<Border>,
    pub selected: Option<Border>,
    pub hilight_selected: Option<Border>,
    pub text: Option<TextDef>,
    /// Progress bars: the bar fill.
    pub progress: Option<Border>,
    pub cur_value: i32,
    pub max_value: i32,
    pub start_from_left: bool,
    pub is_selected: bool,
    /// List boxes: the item template and the scrollbar.
    pub proto: Option<Box<Control>>,
    pub scrollbar: Option<Box<Control>>,
    pub padding: f32,
    pub left_scrollbar: bool,
    /// Scrollbars: thumb and arrow images.
    pub thumb: Option<String>,
    pub dir: Option<String>,
}

impl Control {
    fn parse(s: &GffStruct) -> Self {
        let ext = s.child("EXTENT");
        let r = |l: &str| ext.map_or(0.0, |e| e.int_or(l, 0) as f32);
        let border = |l: &str| s.child(l).map(Border::parse);
        let text = s.child("TEXT").map(|t| TextDef {
            font: t.resref("FONT").unwrap_or_default(),
            text: t.string_or_empty("TEXT"),
            strref: t.uint("STRREF").filter(|&v| v != u32::MAX),
            align: t.int_or("ALIGNMENT", 18),
            color: t.vec3("COLOR").unwrap_or([1.0; 3]),
            pulsing: t.flag("PULSING"),
        });
        let image = |l: &str| s.child(l).and_then(|c| tex(c, "IMAGE"));
        Control {
            kind: Kind::from(s.int_or("CONTROLTYPE", 0)),
            tag: s.string_or_empty("TAG"),
            id: s.int_or("ID", -1),
            rect: [r("LEFT"), r("TOP"), r("WIDTH"), r("HEIGHT")],
            move_to: s.child("MOVETO").map_or_else(MoveTo::default, |m| MoveTo { up: m.int_or("UP", -1), down: m.int_or("DOWN", -1), left: m.int_or("LEFT", -1), right: m.int_or("RIGHT", -1) }),
            border: border("BORDER"),
            hilight: border("HILIGHT"),
            selected: border("SELECTED"),
            hilight_selected: border("HILIGHTSELECTED"),
            text,
            progress: border("PROGRESS"),
            cur_value: s.int_or("CURVALUE", 0),
            max_value: s.int_or("MAXVALUE", 100),
            start_from_left: s.flag("STARTFROMLEFT"),
            is_selected: s.flag("ISSELECTED"),
            proto: s.child("PROTOITEM").map(|p| Box::new(Control::parse(p))),
            scrollbar: s.child("SCROLLBAR").map(|p| Box::new(Control::parse(p))),
            padding: s.int_or("PADDING", 0) as f32,
            left_scrollbar: s.flag("LEFTSCROLLBAR"),
            thumb: image("THUMB"),
            dir: image("DIR"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct GuiDef {
    pub name: String,
    pub root: Control,
    pub controls: Vec<Control>,
}

impl GuiDef {
    pub fn index(&self, tag: &str) -> Option<usize> {
        self.controls.iter().position(|c| c.tag.eq_ignore_ascii_case(tag))
    }

    /// Exchanges where controls `a` and `b` sit: their rects trade, and so
    /// do their keyboard neighbours. Each takes the other's MoveTo, with
    /// references to `a` and `b` renamed, so every neighbour still names the
    /// control that now sits beside it.
    pub fn swap_places(&mut self, a: usize, b: usize) {
        let rect = self.controls[a].rect;
        self.controls[a].rect = self.controls[b].rect;
        self.controls[b].rect = rect;
        let (ma, mb) = (self.controls[a].move_to, self.controls[b].move_to);
        let renamed = |i: i32| match i {
            i if i == a as i32 => b as i32,
            i if i == b as i32 => a as i32,
            i => i,
        };
        for c in &mut self.controls {
            c.move_to = c.move_to.map(renamed);
        }
        self.controls[a].move_to = mb.map(renamed);
        self.controls[b].move_to = ma.map(renamed);
    }
}

/// Loads a GUI by resref (override first, like every resource).
pub fn load(res: &Resources, name: &str) -> Option<GuiDef> {
    let bytes = res.get(&ResKey::new(name, ResourceType::Gui)).ok().flatten()?;
    let gff = rakata_formats::read_gff_from_bytes(&bytes).ok()?;
    let top = &gff.root;
    Some(GuiDef { name: name.to_owned(), root: Control::parse(top), controls: top.list("CONTROLS").iter().map(Control::parse).collect() })
}

/// The first existing variant of a GUI: the 800x600 PC layout where K2
/// ships per-resolution files, then the plain PC one.
pub fn resolve(res: &Resources, base: &str) -> Option<String> {
    [format!("{base}8x6_p"), format!("{base}_p"), base.to_owned()]
        .into_iter()
        .find(|n| res.locate(&ResKey::new(n, ResourceType::Gui)).is_some())
}

#[cfg(test)]
mod tests;
