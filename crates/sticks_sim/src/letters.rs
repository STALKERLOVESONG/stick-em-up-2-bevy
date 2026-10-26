//! `ExplodableText` (the Skyline tutorial texts): on `OnLoadedLevelEverywhere` the text object
//! is replaced by one object per character, "Explodable <c>", each a single-character
//! `tk2dTextMesh` (anchor middle centre) with its own `TextShadower`, a `BoxCollider2D`, a
//! `Health` of 6.1035156e-5 (any hit exhausts it), `ExplodeOnHealthExhausted`,
//! `ExplodeSpritesAndLines` and `DestroyOnExplosionFinished`. A new object is on layer 0, so the
//! letters are solid scenery (characters stand on them, projectiles and lines of sight stop at
//! them) until they are shot.
//!
//! [`split`] transcribes `ExplodableText.Go`: the whole text's estimated bounds give the top-left
//! start; lines are aligned within the whole width by the anchor; a space only advances
//! (`size.x + Spacing`); a letter sits at the pen position plus half its own estimated size.

use unity_compat::{Quat, Vec3};
use unity_data::fonts::{Font, SceneText};

use crate::explosion::{FADE_FADE_TIME, MAX_PART_ANGULAR_SPEED, MAX_PART_SPEED, PartMotion};
use crate::rng::GameRng;
use crate::text::{TextLayout, estimated_bounds, format_text, text_local_geometry};

/// `ExplodableText.Go`: `health.startingHealth = 6.1035156E-05f`.
pub const LETTER_HEALTH: f32 = 6.103_515_6e-5;
/// `ExplodeSpritesAndLines.fadeWaitTime` of a component added at run time.
pub const LETTER_FADE_WAIT: f32 = 2.0;

/// One "Explodable <c>" object.
#[derive(Clone, Debug, PartialEq)]
pub struct Letter {
    pub ch: char,
    /// World position (the object has no rotation and unit scale).
    pub position: Vec3,
    /// The letter's text settings (`CopySettings`, then `formatting = false`, `maxChars = 1`,
    /// anchor middle centre; spacing and line spacing keep a new mesh's 0).
    pub layout: TextLayout,
    pub color: [f32; 4],
    /// The `BoxCollider2D`: offset from `position` and size.
    pub collider_offset: [f32; 2],
    pub collider_size: [f32; 2],
}

/// `ExplodableText.Go` for a text whose object is at `text.world`.
pub fn split(font: &Font, text: &SceneText) -> Vec<Letter> {
    let layout = TextLayout::of(text);
    let t = text.world.t;
    let mut pos = Vec3::new(t[0], t[1], t[2]);
    let (center, size) = estimated_bounds(font, &layout, &text.text);
    // `pos += Utility.ZeroZ(wholeBounds.min)`.
    pos.x += center[0] - size[0] * 0.5;
    pos.y += center[1] - size[1] * 0.5;
    let start_x = pos.x;
    let total_line_height = (font.line_height + layout.line_spacing) * layout.scale[1];
    let formatted = format_text(font, &layout, &text.text);
    let lines: Vec<&str> = formatted.split('\n').collect();
    pos.y += (lines.len() - 1) as f32 * total_line_height;
    let letter_layout = TextLayout {
        scale: layout.scale,
        anchor: 4,
        kerning: layout.kerning,
        max_chars: 1,
        inline_styling: layout.inline_styling,
        formatting: false,
        word_wrap_width: 0,
        spacing: 0.0,
        line_spacing: 0.0,
    };
    let mut letters = Vec::new();
    for line in lines {
        pos.x = start_x;
        let excess = size[0] - estimated_bounds(font, &layout, line).1[0];
        match layout.anchor % 3 {
            1 => pos.x += excess / 2.0,
            2 => pos.x += excess,
            _ => {}
        }
        for ch in line.chars() {
            let bounds = estimated_bounds(font, &layout, &ch.to_string()).1;
            if ch == ' ' {
                pos.x += bounds[0] + layout.spacing;
                continue;
            }
            let position = Vec3::new(pos.x + bounds[0] * 0.5, pos.y + bounds[1] * 0.5, pos.z);
            // TODO(UNCERTAIN): `AddComponent<BoxCollider2D>()` after `goTM.Init()` is taken to fit
            // the collider to the letter's mesh bounds.
            let mesh = text_local_geometry(font, &letter_layout, &ch.to_string(), text.color, text.color, false);
            let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
            for p in &mesh.positions {
                for k in 0..2 {
                    lo[k] = lo[k].min(p[k]);
                    hi[k] = hi[k].max(p[k]);
                }
            }
            let (collider_offset, collider_size) =
                if mesh.positions.is_empty() { ([0.0, 0.0], [0.0, 0.0]) } else { ([(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5], [hi[0] - lo[0], hi[1] - lo[1]]) };
            letters.push(Letter { ch, position, layout: letter_layout.clone(), color: text.color, collider_offset, collider_size });
            pos.x += bounds[0];
        }
        pos.y -= total_line_height;
    }
    letters
}

/// A letter's `Explosion`: its one `TextMeshPart` ("Exploding Text" with a `TextShadower`)
/// flying, spinning and fading like every other part.
#[derive(Clone, Debug, PartialEq)]
pub struct LetterExplosion {
    pub position: Vec3,
    pub rotation: Quat,
    pub start_color: [f32; 4],
    pub color: [f32; 4],
    pub motion: PartMotion,
    pub run_time: f32,
    pub fade_wait_time: f32,
}

impl LetterExplosion {
    /// `Explosion.Go` (then its immediate `Update`).
    pub fn go(letter: &Letter, rng: &mut GameRng, delta_time: f32) -> LetterExplosion {
        let mut e = LetterExplosion {
            position: letter.position,
            rotation: Quat::IDENTITY,
            start_color: letter.color,
            color: letter.color,
            motion: PartMotion::randomized(rng, MAX_PART_SPEED, MAX_PART_ANGULAR_SPEED),
            run_time: 0.0,
            fade_wait_time: LETTER_FADE_WAIT,
        };
        e.update(delta_time);
        e
    }

    /// `Explosion.Update`; true once finished (`DestroyOnExplosionFinished` then removes it).
    pub fn update(&mut self, delta_time: f32) -> bool {
        self.run_time += delta_time;
        let num = (self.run_time - self.fade_wait_time).max(0.0) / FADE_FADE_TIME;
        if num > 1.0 {
            return true;
        }
        self.position = self.position + self.motion.linear * delta_time;
        self.rotation = self.rotation * Quat::about_z((-(self.motion.angular * delta_time)).to_radians());
        self.color = self.start_color.map(|c| c + (0.0 - c) * num.clamp(0.0, 1.0));
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use unity_data::fonts::FontChar;
    use unity_data::math::Affine;

    fn font() -> Font {
        let mut f = Font { version: 2, line_height: 2.0, texel_size: [0.01, 0.01], use_dictionary: true, ..Font::default() };
        let glyph = FontChar { p0: [0.0, 1.5], p1: [0.8, 0.0], uv0: [0.0, 1.0], uv1: [0.1, 0.9], flipped: false, advance: 1.0 };
        for c in ['A', 'B', ' ', '\0'] {
            f.dict.insert(c as u32, glyph);
        }
        f
    }

    fn text(s: &str, anchor: i64) -> SceneText {
        SceneText {
            object: 1,
            name: "t".into(),
            font: "f".into(),
            text: s.into(),
            color: [1.0, 0.5, 0.0, 1.0],
            color2: [1.0; 4],
            use_gradient: false,
            anchor,
            scale: [1.0; 3],
            kerning: false,
            max_chars: 16,
            inline_styling: false,
            formatting: false,
            word_wrap_width: 0,
            spacing: 0.0,
            line_spacing: 0.0,
            world: Affine::from_trs([10.0, 5.0, -0.1], (0.0, 0.0, 0.0, 1.0), [1.0; 3]),
            world_rotation: [0.0, 0.0, 0.0, 1.0],
            shadow_of: None,
        }
    }

    #[test]
    fn letters_replace_the_text_where_its_glyphs_were() {
        // Upper left: the text hangs from (10, 5); each character is one advance (1) wide and
        // one line (2) tall, so the first letter's centre is at (10.5, 5 - 1).
        let letters = split(&font(), &text("AB A", 0));
        assert_eq!(letters.iter().map(|l| l.ch).collect::<String>(), "ABA");
        assert_eq!(letters[0].position, Vec3::new(10.5, 4.0, -0.1));
        assert_eq!(letters[1].position.x, 11.5);
        // The space only advances the pen.
        assert_eq!(letters[2].position.x, 13.5);
        assert!(letters.iter().all(|l| l.layout.anchor == 4 && l.layout.max_chars == 1 && !l.layout.formatting));
        // The collider fits the glyph (0.8 x 1.5), centred on the anchored mesh.
        assert!((letters[0].collider_size[0] - 0.8).abs() < 1e-5 && (letters[0].collider_size[1] - 1.5).abs() < 1e-5);
        // Centred text: the same letters, shifted by half the width.
        let centred = split(&font(), &text("AB", 1));
        assert!((centred[0].position.x - 9.5).abs() < 1e-5, "{:?}", centred[0].position);
    }

    #[test]
    fn a_shot_letter_flies_and_fades_out() {
        let letter = &split(&font(), &text("A", 0))[0];
        let mut e = LetterExplosion::go(letter, &mut GameRng::from_seed(9), 0.02);
        assert!(e.motion.linear.magnitude() <= MAX_PART_SPEED + 1e-4);
        assert!(!e.update(2.0));
        assert!(!e.update(0.5));
        assert!(e.color[3] < 0.6 && e.color[3] > 0.4);
        assert!(e.update(0.6));
    }
}
