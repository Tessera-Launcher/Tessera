//! Adapt the shared DTCG design system to gpui-component, including its Base
//! projection. Theme::update keeps menus, fields, sliders and scrollbars in sync.
use crate::design_tokens::{color, font, motion, radius};
use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{px, rgb, rgba, App};

pub(crate) fn colors(cx: &App) -> &'static color::Palette {
    if Theme::global(cx).is_dark() {
        &color::DARK
    } else {
        &color::LIGHT
    }
}

pub(crate) fn install(cx: &mut App) {
    apply(ThemeMode::Light, cx);
}

pub(crate) fn toggle(cx: &mut App) {
    let mode = if Theme::global(cx).is_dark() {
        ThemeMode::Light
    } else {
        ThemeMode::Dark
    };
    apply(mode, cx);
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
