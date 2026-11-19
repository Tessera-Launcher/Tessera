//! Adapt the shared DTCG design system to gpui-component, including its Base
//! projection. Theme::update keeps menus, fields, sliders and scrollbars in sync.
//! The settings page can replace the generated blue accent with a seed color;
//! the rest of the palette is then rebuilt at the same contrast levels.
use std::sync::{OnceLock, RwLock};

use crate::design_tokens::{color, font, motion, radius};
use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{px, rgb, rgba, App};

/// One of the seven swatches in the settings page.
pub(crate) struct AccentChoice {
    pub(crate) hex: &'static str,
    pub(crate) rgb: u32,
}

/// Presets stay ordered around the color wheel, with the generated blue first.
pub(crate) const ACCENTS: [AccentChoice; 7] = [
    AccentChoice {
        hex: "#2F7FC1",
        rgb: 0x2F7FC1,
    },
    AccentChoice {
        hex: "#1697A6",
        rgb: 0x1697A6,
    },
    AccentChoice {
        hex: "#2F9E63",
        rgb: 0x2F9E63,
    },
    AccentChoice {
        hex: "#D4A017",
        rgb: 0xD4A017,
    },
    AccentChoice {
        hex: "#E2722E",
        rgb: 0xE2722E,
    },
    AccentChoice {
        hex: "#CC4B4B",
        rgb: 0xCC4B4B,
    },
    AccentChoice {
        hex: "#8257C9",
        rgb: 0x8257C9,
    },
];

const DEFAULT_ACCENT: u32 = 0x2F7FC1;

/// The palettes currently in use. Replaced as a whole when the accent changes;
/// keeping the leaked values alive lets the rest of the UI keep its `'static`
/// palette references.
struct Active {
    accent: u32,
    light: &'static color::Palette,
    dark: &'static color::Palette,
}

static ACTIVE: OnceLock<RwLock<Active>> = OnceLock::new();

fn active() -> &'static RwLock<Active> {
    ACTIVE.get_or_init(|| {
        RwLock::new(Active {
            accent: DEFAULT_ACCENT,
            light: &color::LIGHT,
            dark: &color::DARK,
        })
    })
}

pub(crate) fn colors(cx: &App) -> &'static color::Palette {
    let active = active().read().expect("theme palette lock");
    if Theme::global(cx).is_dark() {
        active.dark
    } else {
        active.light
    }
}

pub(crate) fn set(mode: tessera_core::settings::Theme, accent: &str, cx: &mut App) {
    let accent = accent_rgb(accent).unwrap_or(DEFAULT_ACCENT);
    {
        let mut active = active().write().expect("theme palette lock");
        if active.accent != accent {
            let light = Box::leak(Box::new(accent_palette(&color::LIGHT, accent, false)));
            let dark = Box::leak(Box::new(accent_palette(&color::DARK, accent, true)));
            *active = Active {
                accent,
                light,
                dark,
            };
        }
    }
    apply(
        match mode {
            tessera_core::settings::Theme::Light => ThemeMode::Light,
            tessera_core::settings::Theme::Dark => ThemeMode::Dark,
        },
        cx,
    );
}

fn accent_rgb(value: &str) -> Option<u32> {
    let hex = tessera_core::settings::normalize_accent_hex(value)?;
    u32::from_str_radix(hex.trim_start_matches('#'), 16).ok()
}

/// Keep the generated palette for the default blue. Other accents reuse the
/// same surface, border and text neutrals, while the brand roles are rebuilt
/// from the chosen hue at the same perceptual contrast.
fn accent_palette(base: &color::Palette, accent: u32, dark: bool) -> color::Palette {
    if accent == DEFAULT_ACCENT {
        return *base;
    }

    let (hue, saturation, _) = rgb_to_hsl(accent);
    // The generated ramp is a little more saturated at the role steps than the
    // seed swatch, which keeps accent text from looking washed out.
    let saturation = (saturation * 1.12).clamp(0.0, 0.92);
    let mut palette = *base;

    if dark {
        palette.text_accent = with_luminance(hue, saturation, 0.40);
        palette.text_accent_hover = with_luminance(hue, saturation, 0.55);
        palette.fill_primary = with_luminance(hue, saturation, 0.115);
        palette.fill_primary_hover = with_luminance(hue, saturation, 0.16);
        palette.selected = hsl(hue, saturation * 0.55, 0.16);
    } else {
        palette.text_accent = with_luminance(hue, saturation, 0.075);
        palette.text_accent_hover = with_luminance(hue, saturation, 0.05);
        palette.fill_primary = with_luminance(hue, saturation, 0.085);
        palette.fill_primary_hover = with_luminance(hue, saturation, 0.05);
        palette.selected = hsl(hue, saturation * 0.55, 0.93);
    }

    palette.border_focus = palette.text_accent;
    palette.status_success = palette.text_accent;
    palette
}

fn apply(mode: ThemeMode, cx: &mut App) {
    Theme::change(mode, None, cx);
    let colors = colors(cx);
    let installed = cx.text_system().all_font_names();
    let ui_family = font::FAMILY
        .into_iter()
        .find(|family| installed.iter().any(|name| name.as_str() == *family));
    let mono_family = font::MONO
        .into_iter()
        .find(|family| installed.iter().any(|name| name.as_str() == *family));
    Theme::update(cx, |theme| {
        macro_rules! colors {
            ($($value:expr => [$($field:ident),+]),+ $(,)?) => {
                $($(theme.colors.$field = rgb($value).into();)+)+
            };
        }
        colors! {
            colors.canvas => [background, title_bar, status_bar],
            colors.surface => [accordion, group_box, list, list_head, popover, sidebar, tab, tab_bar, table, table_head, table_foot],
            colors.raised => [button, secondary, button_secondary, muted, list_even, table_even, skeleton, switch, tab_bar_segmented, description_list_label],
            colors.hover => [button_hover, secondary_hover, button_secondary_hover, list_hover, table_hover],
            colors.selected => [accent, button_active, secondary_active, button_secondary_active, list_active, table_active, selection, sidebar_accent, tab_active, drop_target],
            colors.border_default => [border, input, sidebar_border, table_row_border, title_bar_border, status_bar_border, window_border, chart_grid, scrollbar],
            colors.border_strong => [scrollbar_thumb],
            colors.border_focus => [ring, drag_border, list_active_border, table_active_border],
            colors.text_primary => [foreground, button_foreground, secondary_foreground, button_secondary_foreground, popover_foreground, sidebar_foreground, sidebar_accent_foreground, tab_active_foreground, table_head_foreground, table_foot_foreground, group_box_foreground],
            colors.text_secondary => [muted_foreground, tab_foreground, scrollbar_thumb_hover, description_list_label_foreground, magenta, magenta_light, chart_4, chart_5],
            colors.text_accent => [link, link_active, caret],
            colors.text_accent_hover => [link_hover],
            colors.text_on_fill => [primary_foreground, button_primary_foreground, info_foreground, button_info_foreground, success_foreground, button_success_foreground, warning_foreground, button_warning_foreground, danger_foreground, button_danger_foreground, sidebar_primary_foreground, accent_foreground, switch_thumb],
            colors.fill_primary => [primary, primary_active, button_primary, button_primary_active, info, info_active, button_info, button_info_active, success, success_active, button_success, button_success_active, sidebar_primary, progress_bar, slider_bar, slider_thumb, blue, green, cyan, chart_1, chart_bullish],
            colors.fill_primary_hover => [primary_hover, button_primary_hover, info_hover, button_info_hover, success_hover, button_success_hover, blue_light, green_light, cyan_light],
            colors.fill_warning => [warning, warning_active, warning_hover, button_warning, button_warning_active, button_warning_hover],
            colors.fill_danger => [danger, danger_active, danger_hover, button_danger, button_danger_active, button_danger_hover],
            colors.status_warning => [yellow, yellow_light, chart_2],
            colors.status_danger => [red, red_light, chart_3, chart_bearish]
        }
        // Keep the 16 px scrollbar lane from painting a visible rail behind the 6 px thumb.
        theme.colors.scrollbar.a = 0.0;
        theme.colors.overlay = rgba(colors.backdrop).into();
        // Keep the library's platform fallback if none of our families exists.
        if let Some(family) = ui_family {
            theme.font_family = family.into();
        }
        if let Some(family) = mono_family {
            theme.mono_font_family = family.into();
        }
        theme.font_size = px(font::BODY);
        theme.mono_font_size = px(font::CAPTION);
        theme.radius = px(radius::SM);
        theme.radius_lg = px(radius::LG);
        theme.motion.duration_fast = std::time::Duration::from_millis(motion::FAST);
        theme.motion.duration_normal = std::time::Duration::from_millis(motion::STANDARD);
    });
}

/// sRGB hex to HSL with all components in `0..=1`.
fn rgb_to_hsl(rgb: u32) -> (f32, f32, f32) {
    let r = ((rgb >> 16) & 0xff) as f32 / 255.0;
    let g = ((rgb >> 8) & 0xff) as f32 / 255.0;
    let b = (rgb & 0xff) as f32 / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let lightness = (max + min) / 2.0;
    if (max - min).abs() < f32::EPSILON {
        return (0.0, 0.0, lightness);
    }
    let delta = max - min;
    let saturation = if lightness > 0.5 {
        delta / (2.0 - max - min)
    } else {
        delta / (max + min)
    };
    let hue = if max == r {
        ((g - b) / delta + if g < b { 6.0 } else { 0.0 }) / 6.0
    } else if max == g {
        ((b - r) / delta + 2.0) / 6.0
    } else {
        ((r - g) / delta + 4.0) / 6.0
    };
    (hue, saturation, lightness)
}

fn hsl(hue: f32, saturation: f32, lightness: f32) -> u32 {
    let saturation = saturation.clamp(0.0, 1.0);
    let lightness = lightness.clamp(0.0, 1.0);
    if saturation <= f32::EPSILON {
        let channel = (lightness * 255.0).round() as u32;
        return channel << 16 | channel << 8 | channel;
    }
    let q = if lightness < 0.5 {
        lightness * (1.0 + saturation)
    } else {
        lightness + saturation - lightness * saturation
    };
    let p = 2.0 * lightness - q;
    let channel = |offset: f32| {
        let mut value = hue + offset;
        if value < 0.0 {
            value += 1.0;
        }
        if value > 1.0 {
            value -= 1.0;
        }
        let value = if value < 1.0 / 6.0 {
            p + (q - p) * 6.0 * value
        } else if value < 0.5 {
            q
        } else if value < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - value) * 6.0
        } else {
            p
        };
        (value.clamp(0.0, 1.0) * 255.0).round() as u32
    };
    channel(1.0 / 3.0) << 16 | channel(0.0) << 8 | channel(-1.0 / 3.0)
}

fn luminance(rgb: u32) -> f32 {
    let channel = |shift: u32| {
        let value = ((rgb >> shift) & 0xff) as f32 / 255.0;
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(16) + 0.7152 * channel(8) + 0.0722 * channel(0)
}

/// Find the lightness at which `hue`/`saturation` has the requested WCAG
/// luminance. This keeps white fill text readable across yellow, green and blue.
fn with_luminance(hue: f32, saturation: f32, target: f32) -> u32 {
    let (mut low, mut high) = (0.0, 1.0);
    for _ in 0..24 {
        let middle = (low + high) / 2.0;
        if luminance(hsl(hue, saturation, middle)) < target {
            low = middle;
        } else {
            high = middle;
        }
    }
    hsl(hue, saturation, (low + high) / 2.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accent_palettes_keep_white_fill_text_readable() {
        for preset in ACCENTS {
            for dark in [false, true] {
                let palette = accent_palette(
                    if dark { &color::DARK } else { &color::LIGHT },
                    preset.rgb,
                    dark,
                );
                let fill = luminance(palette.fill_primary);
                assert!((1.05 / (fill + 0.05)) >= 4.5, "{} {dark}", preset.hex);
            }
        }
    }

    #[test]
    fn hex_parsing_accepts_hashless_uppercase_and_rejects_short_values() {
        assert_eq!(accent_rgb("#2f7fc1"), Some(0x2F7FC1));
        assert_eq!(accent_rgb("2F7FC1"), Some(0x2F7FC1));
        assert_eq!(accent_rgb("#abc"), None);
        assert_eq!(accent_rgb("#gggggg"), None);
    }
}
