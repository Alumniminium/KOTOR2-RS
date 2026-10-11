//! Bitmap text: glyph quads from a KotOR font texture.
//!
//! `spawn_glyphs` lays text into a box for GUI controls. `BitmapText` is a
//! component for free-standing UI (dialog lines, replies): it sizes its own
//! node so it can sit in a flex layout, and redraws when changed or when
//! the GUI scale changes.

use std::sync::Arc;

use bevy::prelude::*;

use super::assets::{BitmapFont, GuiAssets};
use super::GuiScale;
use crate::model::GameRes;

/// Marks glyph nodes so a rebuild can find them.
#[derive(Component)]
pub struct Glyph;

/// Lays `text` into `area` (window px, relative to `parent`). Alignment is
/// the GUI bitfield: 1 left, 2 centre, 4 right; 8 top, 16 middle, 32 bottom.
#[allow(clippy::too_many_arguments)]
pub fn spawn_glyphs(commands: &mut Commands, parent: Entity, font: &BitmapFont, text: &str, area: [f32; 4], align: i32, color: Color, s: f32, wrap: bool) {
    let lines = font.wrap(text, if wrap { area[2] / s } else { 0.0 });
    let lh = font.height * s;
    let total = lh * lines.len() as f32;
    let y0 = if align & 8 != 0 {
        area[1]
    } else if align & 32 != 0 {
        area[1] + area[3] - total
    } else {
        area[1] + (area[3] - total) * 0.5
    };
    for (n, line) in lines.iter().enumerate() {
        let lw = font.width(line) * s;
        let mut x = if align & 1 != 0 {
            area[0]
        } else if align & 4 != 0 {
            area[0] + area[2] - lw
        } else {
            area[0] + (area[2] - lw) * 0.5
        };
        let y = (y0 + n as f32 * lh).round();
        for c in line.chars() {
            let Some(g) = font.glyph(c) else { continue };
            let (gw, gh) = (g.rect.width() * s, g.rect.height() * s);
            if c != ' ' {
                commands.spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px(x.round()),
                        top: Val::Px(y),
                        width: Val::Px(gw),
                        height: Val::Px(gh),
                        ..default()
                    },
                    ImageNode { image: font.tex.image.clone(), rect: Some(g.rect), color, image_mode: NodeImageMode::Stretch, ..default() },
                    Glyph,
                    super::GuiPiece,
                    ChildOf(parent),
                ));
            }
            x += (g.rect.width() + font.spacing) * s;
        }
    }
}

/// Free-standing bitmap text. `max_width` is in layout px (0 = no wrap).
#[derive(Component, Clone, PartialEq)]
pub struct BitmapText {
    pub text: String,
    pub font: String,
    pub color: Color,
    pub max_width: f32,
    /// Alignment bitfield (horizontal bits only matter here).
    pub align: i32,
}

impl BitmapText {
    pub fn new(text: impl Into<String>, font: &str, color: Color) -> Self {
        Self { text: text.into(), font: font.to_owned(), color, max_width: 0.0, align: 9 }
    }

    pub fn wrap(mut self, w: f32) -> Self {
        self.max_width = w;
        self
    }

    pub fn align(mut self, a: i32) -> Self {
        self.align = a;
        self
    }
}

pub fn build_bitmap_text(
    mut commands: Commands,
    scale: Res<GuiScale>,
    game: Option<Res<GameRes>>,
    mut images: ResMut<Assets<Image>>,
    mut assets: ResMut<GuiAssets>,
    mut q: Query<(Entity, Ref<BitmapText>, &mut Node, Option<&Children>)>,
    glyphs: Query<(), With<Glyph>>,
) {
    let Some(game) = game else { return };
    for (e, bt, mut node, children) in &mut q {
        if !bt.is_changed() && !scale.is_changed() {
            continue;
        }
        for c in children.into_iter().flatten() {
            if glyphs.contains(*c) {
                commands.entity(*c).despawn();
            }
        }
        let Some(font): Option<Arc<BitmapFont>> = assets.font_or_default(&game.0, &mut images, &bt.font) else { continue };
        let s = scale.s;
        let lines = font.wrap(&bt.text, bt.max_width);
        let w = if bt.max_width > 0.0 { bt.max_width } else { lines.iter().map(|l| font.width(l)).fold(0.0, f32::max) };
        let h = font.height * lines.len().max(1) as f32;
        node.width = Val::Px(w * s);
        node.height = Val::Px(h * s);
        if bt.text.is_empty() {
            node.height = Val::Px(0.0);
            continue;
        }
        spawn_glyphs(&mut commands, e, &font, &bt.text, [0.0, 0.0, w * s, h * s], (bt.align & 7) | 8, bt.color, s, bt.max_width > 0.0);
    }
}

