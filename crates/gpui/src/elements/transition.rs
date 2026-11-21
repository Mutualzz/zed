use std::rc::Rc;
use std::time::Duration;

use scheduler::Instant;

use crate::{Background, BoxShadow, Fill, Hsla, Pixels, Rgba, Style, point, px};

/// A style property that [`StyleTransition`] can animate, like CSS `transition-property`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransitionProperty {
    /// The solid background color.
    Background,
    /// The border color.
    BorderColor,
    /// The text color.
    TextColor,
    /// The element opacity.
    Opacity,
    /// The box shadows.
    BoxShadow,
}

impl TransitionProperty {
    /// Every animatable property, like CSS `transition: all`.
    pub const ALL: [TransitionProperty; 5] = [
        TransitionProperty::Background,
        TransitionProperty::BorderColor,
        TransitionProperty::TextColor,
        TransitionProperty::Opacity,
        TransitionProperty::BoxShadow,
    ];
}

/// How an element animates between the styles it is given, like the CSS `transition` property.
#[derive(Clone)]
pub struct StyleTransition {
    /// How long a change takes.
    pub duration: Duration,
    /// Maps linear progress in 0..1 to eased progress.
    pub easing: Rc<dyn Fn(f32) -> f32>,
    /// The properties that animate; the others change immediately.
    pub properties: Vec<TransitionProperty>,
}

impl StyleTransition {
    fn animates(&self, property: TransitionProperty) -> bool {
        self.properties.contains(&property)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TransitionValues {
    background: Option<Hsla>,
    border_color: Option<Hsla>,
    text_color: Option<Hsla>,
    opacity: f32,
    box_shadow: Vec<BoxShadow>,
}

impl TransitionValues {
    fn from_style(style: &Style) -> Self {
        Self {
            background: style
                .background
                .as_ref()
                .and_then(Fill::color)
                .and_then(|background| background.as_solid()),
            border_color: style.border_color,
            text_color: style.text.color,
            opacity: style.opacity.unwrap_or(1.0),
            box_shadow: style.box_shadow.clone(),
        }
    }

    fn apply_to(&self, style: &mut Style) {
        if let Some(background) = self.background
            && style
                .background
                .as_ref()
                .and_then(Fill::color)
                .is_none_or(|current| current.as_solid().is_some())
        {
            style.background = Some(Fill::Color(Background::from(background)));
        }
        if self.border_color.is_some() {
            style.border_color = self.border_color;
        }
        if self.text_color.is_some() {
            style.text.color = self.text_color;
        }
        if style.opacity.is_some() || self.opacity != 1.0 {
            style.opacity = Some(self.opacity);
        }
        if !style.box_shadow.is_empty() || !self.box_shadow.is_empty() {
            style.box_shadow = self.box_shadow.clone();
        }
    }

    fn mix(from: &Self, to: &Self, progress: f32, transition: &StyleTransition) -> Self {
        let eased = |property| {
            if transition.animates(property) {
                progress
            } else {
                1.0
            }
        };
        Self {
            background: mix_color(
                from.background,
                to.background,
                eased(TransitionProperty::Background),
            ),
            border_color: mix_color(
                from.border_color,
                to.border_color,
                eased(TransitionProperty::BorderColor),
            ),
            text_color: mix_color(
                from.text_color,
                to.text_color,
                eased(TransitionProperty::TextColor),
            ),
            opacity: from.opacity
                + (to.opacity - from.opacity) * eased(TransitionProperty::Opacity),
            box_shadow: mix_shadows(
                &from.box_shadow,
                &to.box_shadow,
                eased(TransitionProperty::BoxShadow),
            ),
        }
    }
}

fn mix_pixels(from: Pixels, to: Pixels, progress: f32) -> Pixels {
    from + (to - from) * progress
}

fn faded_shadow(shadow: &BoxShadow) -> BoxShadow {
    BoxShadow {
        color: transparent_like(shadow.color),
        offset: point(px(0.0), px(0.0)),
        blur_radius: px(0.0),
        spread_radius: px(0.0),
        inset: shadow.inset,
    }
}

/// Interpolates shadow lists like CSS: the shorter list is padded with transparent, zero-sized
/// copies, and lists whose shadows differ in `inset` switch at the end instead.
fn mix_shadows(from: &[BoxShadow], to: &[BoxShadow], progress: f32) -> Vec<BoxShadow> {
    let count = from.len().max(to.len());
    let pad = |list: &[BoxShadow], other: &[BoxShadow]| -> Vec<BoxShadow> {
        (0..count)
            .map(|index| {
                list.get(index)
                    .cloned()
                    .unwrap_or_else(|| faded_shadow(&other[index]))
            })
            .collect()
    };
    let padded_from = pad(from, to);
    let padded_to = pad(to, from);
    if padded_from
        .iter()
        .zip(&padded_to)
        .any(|(first, second)| first.inset != second.inset)
    {
        return if progress >= 1.0 { to.to_vec() } else { from.to_vec() };
    }
    padded_from
        .iter()
        .zip(&padded_to)
        .map(|(first, second)| BoxShadow {
            color: mix_color(Some(first.color), Some(second.color), progress)
                .unwrap_or(second.color),
            offset: point(
                mix_pixels(first.offset.x, second.offset.x, progress),
                mix_pixels(first.offset.y, second.offset.y, progress),
            ),
            blur_radius: mix_pixels(first.blur_radius, second.blur_radius, progress),
            spread_radius: mix_pixels(first.spread_radius, second.spread_radius, progress),
            inset: second.inset,
        })
        .collect()
}

fn transparent_like(color: Hsla) -> Hsla {
    Hsla { a: 0.0, ..color }
}

/// Interpolates two colors the way CSS transitions do: in premultiplied sRGB, with a missing
/// color treated as transparent.
pub(crate) fn mix_color(from: Option<Hsla>, to: Option<Hsla>, progress: f32) -> Option<Hsla> {
    let (from, to) = match (from, to) {
        (None, None) => return None,
        (Some(from), None) => (from, transparent_like(from)),
        (None, Some(to)) => (transparent_like(to), to),
        (Some(from), Some(to)) => (from, to),
    };
    let from = Rgba::from(from);
    let to = Rgba::from(to);
    let alpha = from.a + (to.a - from.a) * progress;
    let channel = |first: f32, second: f32| {
        let premultiplied = first * from.a + (second * to.a - first * from.a) * progress;
        if alpha <= f32::EPSILON {
            0.0
        } else {
            (premultiplied / alpha).clamp(0.0, 1.0)
        }
    };
    let mixed = Rgba {
        r: channel(from.r, to.r),
        g: channel(from.g, to.g),
        b: channel(from.b, to.b),
        a: alpha.clamp(0.0, 1.0),
    };
    Some(Hsla::from(mixed))
}

#[derive(Clone, Debug)]
pub(crate) struct TransitionState {
    from: TransitionValues,
    to: TransitionValues,
    started: Instant,
}

impl TransitionState {
    /// Animates `style` toward its own values, starting a new transition from what is currently
    /// drawn whenever the target changes. Returns whether the element is still animating.
    pub(crate) fn apply(
        state: &mut Option<Self>,
        transition: &StyleTransition,
        style: &mut Style,
        now: Instant,
    ) -> bool {
        let target = TransitionValues::from_style(style);
        let current = match state {
            Some(existing) => {
                if existing.to != target {
                    let drawn = existing.value(transition, now);
                    *existing = Self {
                        from: drawn,
                        to: target,
                        started: now,
                    };
                }
                existing
            }
            None => state.insert(Self {
                from: target.clone(),
                to: target,
                started: now,
            }),
        };
        let value = current.value(transition, now);
        value.apply_to(style);
        current.from != current.to && now.duration_since(current.started) < transition.duration
    }

    fn value(&self, transition: &StyleTransition, now: Instant) -> TransitionValues {
        let elapsed = now.saturating_duration_since(self.started);
        if transition.duration.is_zero() || elapsed >= transition.duration {
            return self.to.clone();
        }
        let linear = elapsed.as_secs_f32() / transition.duration.as_secs_f32();
        TransitionValues::mix(&self.from, &self.to, (transition.easing)(linear), transition)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{black, white};

    fn transition() -> StyleTransition {
        StyleTransition {
            duration: Duration::from_millis(300),
            easing: Rc::new(|progress| progress),
            properties: TransitionProperty::ALL.to_vec(),
        }
    }

    fn style_with_background(color: Hsla) -> Style {
        Style {
            background: Some(Fill::from(color)),
            ..Style::default()
        }
    }

    fn background(style: &Style) -> Hsla {
        style
            .background
            .as_ref()
            .and_then(Fill::color)
            .and_then(|background| background.as_solid())
            .expect("solid background")
    }

    #[test]
    fn the_first_style_is_drawn_without_animating() {
        let mut state = None;
        let mut style = style_with_background(black());
        let animating = TransitionState::apply(&mut state, &transition(), &mut style, Instant::now());
        assert!(!animating);
        assert_eq!(background(&style), black());
    }

    #[test]
    fn a_changed_style_eases_from_what_was_drawn() {
        let mut state = None;
        let start = Instant::now();
        TransitionState::apply(&mut state, &transition(), &mut style_with_background(black()), start);
        let mut hovered = style_with_background(white());
        assert!(TransitionState::apply(&mut state, &transition(), &mut hovered, start));
        assert_eq!(background(&hovered), black());
        let mut midway = style_with_background(white());
        TransitionState::apply(
            &mut state,
            &transition(),
            &mut midway,
            start + Duration::from_millis(150),
        );
        let lightness = Rgba::from(background(&midway)).r;
        assert!((lightness - 0.5).abs() < 0.01, "{lightness}");
        let mut done = style_with_background(white());
        let animating = TransitionState::apply(
            &mut state,
            &transition(),
            &mut done,
            start + Duration::from_millis(300),
        );
        assert!(!animating);
        assert_eq!(Rgba::from(background(&done)).r, 1.0);
    }

    #[test]
    fn a_new_background_fades_in_from_transparent() {
        let mixed = mix_color(None, Some(white()), 0.5).expect("color");
        assert!((mixed.a - 0.5).abs() < 0.001);
        assert_eq!(Rgba::from(mixed).r, 1.0);
    }

    #[test]
    fn properties_left_out_change_immediately() {
        let only_opacity = StyleTransition {
            properties: vec![TransitionProperty::Opacity],
            ..transition()
        };
        let mut state = None;
        let start = Instant::now();
        TransitionState::apply(&mut state, &only_opacity, &mut style_with_background(black()), start);
        let mut changed = style_with_background(white());
        TransitionState::apply(
            &mut state,
            &only_opacity,
            &mut changed,
            start + Duration::from_millis(150),
        );
        assert_eq!(Rgba::from(background(&changed)).r, 1.0);
    }

    #[test]
    fn a_new_shadow_grows_from_nothing() {
        let ring = BoxShadow {
            color: white(),
            offset: point(px(0.0), px(0.0)),
            blur_radius: px(0.0),
            spread_radius: px(4.0),
            inset: false,
        };
        let mixed = mix_shadows(&[], std::slice::from_ref(&ring), 0.5);
        assert_eq!(mixed.len(), 1);
        assert_eq!(mixed[0].spread_radius, px(2.0));
        assert!((mixed[0].color.a - 0.5).abs() < 0.001);
        assert_eq!(mix_shadows(std::slice::from_ref(&ring), &[], 1.0)[0].spread_radius, px(0.0));
    }

    #[test]
    fn css_ease_matches_the_keyword_curve() {
        let ease = crate::css_ease;
        assert_eq!(ease(0.0), 0.0);
        assert!((ease(1.0) - 1.0).abs() < 1e-4);
        assert!((ease(0.5) - 0.8024).abs() < 0.01);
    }
}
