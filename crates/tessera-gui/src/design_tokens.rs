// Generated from design/tokens. Run npm run build.
// Colors with alpha are RGBA (use rgba); opaque colors are RGB (use rgb).
#![allow(dead_code)]

pub mod font {
    pub const BODY: f32 = 14.0;
    pub const CAPTION: f32 = 12.0;
    pub const DISPLAY: f32 = 32.0;
    pub const FAMILY: [&str; 4] = ["Segoe UI", "Microsoft YaHei", "Noto Sans CJK SC", "sans-serif"];
    pub const LINE_HEIGHT: f32 = 1.428571;
    pub const LINE_HEIGHT_CAPTION: f32 = 1.333333;
    pub const LINE_HEIGHT_DISPLAY: f32 = 1.25;
    pub const LINE_HEIGHT_TITLE: f32 = 1.333333;
    pub const MONO: [&str; 3] = ["Cascadia Code", "Consolas", "monospace"];
    pub const TITLE: f32 = 21.0;
}

pub mod layout {
    pub const CHECKBOX: f32 = 16.0;
    pub const CLOCK_WIDTH: f32 = 44.0;
    pub const CONTROL_HEIGHT: f32 = 36.0;
    pub const DETAILS: f32 = 380.0;
    pub const DIALOG: f32 = 480.0;
    pub const INDICATOR_HEIGHT: f32 = 16.0;
    pub const INDICATOR_WIDTH: f32 = 3.0;
    pub const INSPECTOR: f32 = 280.0;
    pub const LABEL_WIDTH: f32 = 80.0;
    pub const LOADER_DOT: f32 = 6.0;
    pub const METADATA_WIDTH: f32 = 64.0;
    pub const MIN_HEIGHT: f32 = 640.0;
    pub const MIN_WIDTH: f32 = 960.0;
    pub const MUSIC_VERSIONS: f32 = 240.0;
    pub const PANEL_HEIGHT: f32 = 272.0;
    pub const PROGRESS_HEIGHT: f32 = 8.0;
    pub const PROGRESS_MAX: f32 = 280.0;
    pub const ROW_HEIGHT: f32 = 40.0;
    pub const SIDEBAR: f32 = 208.0;
    pub const SLIDER_MIN: f32 = 80.0;
    pub const STATUS_DOT: f32 = 8.0;
    pub const TRACK_GROUP: f32 = 52.0;
    pub const TREE_INDENT: f32 = 16.0;
    pub const VOLUME_WIDTH: f32 = 96.0;
    pub const WINDOW_HEIGHT: f32 = 760.0;
    pub const WINDOW_WIDTH: f32 = 1160.0;
}

pub mod motion {
    pub const EASE: [f32; 4] = [0.2, 0.0, 0.0, 1.0];
    pub const FAST: u64 = 150;
    pub const STANDARD: u64 = 200;
}

pub mod opacity {
    pub const DISABLED: f32 = 0.4;
}

pub mod radius {
    pub const LG: f32 = 12.0;
    pub const MD: f32 = 6.0;
    pub const PILL: f32 = 1600.0;
    pub const SM: f32 = 3.0;
}

pub mod space {
    pub const LG: f32 = 16.0;
    pub const MD: f32 = 12.0;
    pub const SM: f32 = 8.0;
    pub const XL: f32 = 24.0;
    pub const XS: f32 = 4.0;
    pub const XXL: f32 = 32.0;
    pub const XXS: f32 = 2.0;
    pub const XXXL: f32 = 48.0;
}

pub mod stroke {
    pub const FOCUS: f32 = 2.0;
    pub const THIN: f32 = 1.0;
}

pub mod weight {
    pub const BOLD: f32 = 700.0;
    pub const MEDIUM: f32 = 500.0;
    pub const REGULAR: f32 = 400.0;
    pub const SEMIBOLD: f32 = 600.0;
}

pub mod color {
    #[derive(Clone, Copy)]
    pub struct Palette {
        pub canvas: u32,
        pub surface: u32,
        pub raised: u32,
        pub hover: u32,
        pub selected: u32,
        pub backdrop: u32,
        pub text_primary: u32,
        pub text_secondary: u32,
        pub text_accent: u32,
        pub text_accent_hover: u32,
        pub text_on_fill: u32,
        pub fill_primary: u32,
        pub fill_primary_hover: u32,
        pub fill_neutral: u32,
        pub fill_warning: u32,
        pub fill_danger: u32,
        pub border_default: u32,
        pub border_strong: u32,
        pub border_focus: u32,
        pub status_success: u32,
        pub status_warning: u32,
        pub status_danger: u32,
    }
    pub const DARK: Palette = Palette {
        canvas: 0x060606,
        surface: 0x0d0d0d,
        raised: 0x191919,
        hover: 0x262626,
        selected: 0x112538,
        backdrop: 0x111111cc,
        text_primary: 0xf8f8f8,
        text_secondary: 0xb1b1b1,
        text_accent: 0x89bdef,
        text_accent_hover: 0xaecfee,
        text_on_fill: 0xfefefe,
        fill_primary: 0x0e629e,
        fill_primary_hover: 0x0c79c4,
        fill_neutral: 0xb1b1b1,
        fill_warning: 0xca9329,
        fill_danger: 0xf07268,
        border_default: 0x373737,
        border_strong: 0x696969,
        border_focus: 0x89bdef,
        status_success: 0x89bdef,
        status_warning: 0xca9329,
        status_danger: 0xf07268,
    };
    pub const LIGHT: Palette = Palette {
        canvas: 0xf6f6f6,
        surface: 0xfefefe,
        raised: 0xeeeeee,
        hover: 0xe1e1e1,
        selected: 0xe7eff7,
        backdrop: 0x11111166,
        text_primary: 0x161616,
        text_secondary: 0x525252,
        text_accent: 0x10598f,
        text_accent_hover: 0x144b77,
        text_on_fill: 0xfefefe,
        fill_primary: 0x11568b,
        fill_primary_hover: 0x15456c,
        fill_neutral: 0x525252,
        fill_warning: 0x77540d,
        fill_danger: 0xa02725,
        border_default: 0xc7c7c7,
        border_strong: 0x808080,
        border_focus: 0x10598f,
        status_success: 0x10598f,
        status_warning: 0x77540d,
        status_danger: 0xa02725,
    };
}
