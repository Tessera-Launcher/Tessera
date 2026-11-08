//! Drawing the main screen.
//!
//! Rendering is a function of the state plus a small layout cache: `draw` records
//! where it put things so the next mouse event can be hit-tested, and the details
//! pane measures the selected version once per selection. Nothing here starts a
//! thread, touches the network or changes a screen.

use crossterm::event::KeyCode;
use ratatui::buffer::CellWidth;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Cell, Clear, Paragraph, Row, Table, Wrap};
use ratatui::Frame;

use crate::app::{App, Area, Button, Layout as Areas, LoginStep, Screen};
use crate::discover::{self, LocalVersion};
use crate::install::Loader;

const ACCENT: Color = Color::Rgb(0x6c, 0xd6, 0x7a);
const SNAPSHOT: Color = Color::Rgb(0xe8, 0xc9, 0x6a);
const LEGACY: Color = Color::Rgb(0xc4, 0x8a, 0xd8);
const BROKEN: Color = Color::Rgb(0xe0, 0x6c, 0x6c);
const DIM: Color = Color::Rgb(0x8a, 0x8a, 0x92);

/// Most `azure_app_ids.toml` entries drawn at once; the rest scroll.
const SAVED_ROWS: usize = 6;

/// Render one frame.
pub fn draw(frame: &mut Frame, app: &mut App) -> Areas {
    let mut layout = Areas::default();
    // Hit areas belong to the screen that recorded them. A full-screen pane does
    // not draw the main screen underneath, so a stale footer must not stay
    // clickable behind it.
    layout.buttons.clear();

    // Download, Installing and Java cover the whole terminal. Drawing the main
    // screen first would waste a frame's layout work (and a directory walk for the
    // size cell) only to paint over it.
    let full_screen = matches!(
        app.screen,
        Screen::Download | Screen::Installing | Screen::Java
    );
    if !full_screen {
        draw_main(frame, app, &mut layout);
    }

    match app.screen {
        Screen::Login => draw_login(frame, frame.area(), app, &mut layout),
        Screen::Accounts => draw_accounts(frame, frame.area(), app, &mut layout),
        Screen::Download => draw_download(frame, frame.area(), app, &mut layout),
        Screen::Installing => draw_installing(frame, frame.area(), app, &mut layout),
        Screen::Java => draw_java(frame, frame.area(), app, &mut layout),
        Screen::Main => {}
    }
    layout
}

/// The main screen, drawn under the popups (login, accounts) but not under the
/// full-screen panes.
fn draw_main(frame: &mut Frame, app: &mut App, layout: &mut Areas) {
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(10),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    draw_header(frame, header, app, layout);

    let [list, right] =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Min(38)]).areas(body);
    draw_version_list(frame, list, app, layout);
    let [details, log] = Layout::vertical([Constraint::Length(9), Constraint::Min(4)]).areas(right);
    draw_details(frame, details, app);
    draw_log(frame, log, app, layout);

    draw_footer(frame, footer, app, layout);
}

fn draw_header(frame: &mut Frame, area: Rect, app: &mut App, layout: &mut Areas) {
    let title = Line::from(vec![
        Span::styled(" yaml ", Style::new().fg(Color::Black).bg(ACCENT).bold()),
        Span::styled(" Minecraft Java 版启动器 ", Style::new().fg(Color::White)),
    ]);
    let block = Block::bordered()
        .title(title)
        .border_style(Style::new().fg(DIM));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // The account line doubles as a button. It is rendered as its own widget in a
    // measured rect instead of as a block title: a title lives on the border row,
    // which is where a click on it has to be hit-tested.
    let account_line = match &app.account {
        crate::app::Account::SignedOut => {
            Line::from(Span::styled(" 未登录 · 点此登录", Style::new().fg(ACCENT)))
        }
        crate::app::Account::SignedIn {
            name,
            copy,
            application,
            ..
        } => Line::from(vec![
            Span::styled(format!(" {name}"), Style::new().fg(ACCENT)),
            // The `(1)` of a repeated session, dimmed: it is not part of the name.
            Span::styled(crate::accounts::suffix(*copy), Style::new().fg(DIM)),
            Span::styled(format!(" · {application}"), Style::new().fg(ACCENT)),
        ]),
    };
    let account_text = match &app.account {
        crate::app::Account::SignedOut => "未登录 · 点此登录".to_string(),
        signed_in => signed_in.label(),
    };
    let account_width = account_text.cell_width().saturating_add(2).min(inner.width);

    let info_width = inner.width.saturating_sub(account_width);
    let info = Line::from(vec![
        Span::styled(" 目录 ", Style::new().fg(DIM)),
        Span::styled(
            app.root.display().to_string(),
            Style::new().fg(Color::White),
        ),
        Span::styled("   ·   ", Style::new().fg(DIM)),
        Span::styled(
            format!("{} 个版本", app.versions.len()),
            Style::new().fg(ACCENT),
        ),
        Span::styled("   ·   ", Style::new().fg(DIM)),
        Span::styled(
            format!("{} 个账户", app.accounts.len()),
            Style::new().fg(ACCENT),
        ),
    ]);
    frame.render_widget(
        Paragraph::new(info),
        Rect {
            width: info_width,
            ..inner
        },
    );

    let account_area = Rect {
        x: inner.x + info_width,
        y: inner.y,
        width: account_width,
        height: 1,
    };
    frame.render_widget(Paragraph::new(account_line), account_area);
    layout.account = Area::new(
        account_area.x,
        account_area.y,
        account_area.width,
        account_area.height,
    );
}

fn draw_version_list(frame: &mut Frame, area: Rect, app: &mut App, layout: &mut Areas) {
    let hidden = app.hidden_base_count();
    let title = if hidden > 0 {
        format!(
            " 版本 ({}) · 已隐藏 {hidden} 个基础版本 · h 显示 ",
            app.versions.len()
        )
    } else {
        format!(" 版本 ({}) ", app.versions.len())
    };
    let block = Block::bordered()
        .title(Line::from(title))
        .border_style(Style::new().fg(DIM));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Data rows start below the table header; that is the region clicks address.
    let rows_visible = inner.height.saturating_sub(1) as usize;
    layout.list = Area::new(
        inner.x,
        inner.y.saturating_add(1),
        inner.width,
        rows_visible as u16,
    );
    app.clamp_list(rows_visible);

    if app.versions.is_empty() {
        let hint = Text::from(vec![
            Line::from(Span::styled(
                "没有找到任何版本",
                Style::new().fg(DIM).add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(
                "按 r 重新扫描，或先下载一个版本",
                Style::new().fg(DIM),
            )),
        ]);
        frame.render_widget(
            Paragraph::new(hint)
                .alignment(Alignment::Center)
                .wrap(Wrap { trim: true }),
            inner,
        );
        return;
    }

    // Only the visible slice is built: the list is the one panel that can hold
    // hundreds of rows.
    let offset = app.list_offset();
    let end = (offset + rows_visible).min(app.versions.len());
    let selected = app.selected;
    let confirming = app.uninstall_confirm().map(str::to_string);
    let rows: Vec<Row> = app.versions[offset..end]
        .iter()
        .enumerate()
        .map(|(visible_index, version)| {
            let is_selected = offset + visible_index == selected;
            let mut id = Span::styled(
                version.id.clone(),
                if is_selected {
                    Style::new().fg(ACCENT)
                } else {
                    Style::new().fg(Color::White)
                },
            );
            if !version.has_jar && version.inherits.is_none() {
                id = id.fg(BROKEN);
            }
            // The row waiting for the second `x` is repainted, so the question in
            // the status bar has something on screen pointing at it.
            let is_confirming = confirming.as_deref() == Some(version.dir.as_str());
            if is_confirming {
                id = id
                    .fg(BROKEN)
                    .add_modifier(Modifier::BOLD | Modifier::REVERSED);
            }
            let marker = if is_selected { "▶ " } else { "  " };
            let mut row = Row::new(vec![
                Cell::from(Line::from(vec![
                    Span::styled(marker, Style::new().fg(ACCENT)),
                    id,
                ])),
                Cell::from(Line::from(Span::styled(
                    kind_label(version),
                    Style::new().fg(kind_color(&version.kind)),
                ))),
                Cell::from(Line::from(Span::styled(
                    version
                        .java
                        .map_or_else(|| "-".to_string(), |j| format!("{j}")),
                    Style::new().fg(DIM),
                ))),
            ]);
            if is_selected {
                row = row.style(Style::new().bg(Color::Rgb(0x26, 0x28, 0x2e)));
            }
            row
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Min(14),
            Constraint::Length(16),
            Constraint::Length(4),
        ],
    )
    .header(Row::new(vec!["  版本", "类型", "Java"]).style(Style::new().fg(DIM).bold()))
    .column_spacing(1);

    frame.render_widget(table, inner);
}

fn draw_details(frame: &mut Frame, area: Rect, app: &mut App) {
    let block = Block::bordered()
        .title(Line::from(" 详情 "))
        .border_style(Style::new().fg(DIM));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let Some(version) = app.selected_version().cloned() else {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled("未选择版本", Style::new().fg(DIM))))
                .alignment(Alignment::Center),
            inner,
        );
        return;
    };
    let dir = app.selected_dir();
    // Cached by directory: walking the tree on every frame is a syscall storm on
    // a version with a full `mods/` folder, and nothing changes between frames.
    let size = app.selected_size();

    let mut rows = vec![
        kv("ID", version.id.clone()),
        kv("类型", kind_label(&version)),
        kv("基础版本", version.base_version().to_string()),
        kv(
            "Java",
            version
                .java
                .map_or_else(|| "未指定".to_string(), |j| format!("{j}")),
        ),
        kv(
            "主类",
            version
                .main_class
                .clone()
                .unwrap_or_else(|| "-".to_string()),
        ),
    ];
    if let Some(dir) = &dir {
        rows.push(kv("目录", dir.display().to_string()));
        rows.push(kv("大小", discover::human_size(size)));
    }

    let table = Table::new(
        rows.into_iter().map(|(k, v)| {
            Row::new(vec![
                Cell::from(Line::from(Span::styled(k, Style::new().fg(DIM)))),
                Cell::from(Line::from(Span::styled(v, Style::new().fg(Color::White)))),
            ])
        }),
        [Constraint::Length(10), Constraint::Min(10)],
    )
    .column_spacing(1);
    frame.render_widget(table, inner);
}

fn draw_log(frame: &mut Frame, area: Rect, app: &mut App, layout: &mut Areas) {
    // The border carries the scroll state, so it can only be drawn once the
    // position is known; `inner` is the same either way.
    let inner = Block::bordered().inner(area);

    layout.log = Area::new(inner.x, inner.y, inner.width, inner.height);

    // The log follows the tail until the user scrolls back; `log_scroll` counts
    // lines away from the newest one.
    let height = inner.height as usize;
    let total = app.log.len();
    let max_scroll = total.saturating_sub(height);
    app.clamp_log_scroll(max_scroll);

    let scroll = app.log_scroll.min(max_scroll);
    let end = total - scroll;
    let start = end.saturating_sub(height);

    let title = if scroll > 0 {
        format!(" 日志 ({scroll} 行之前，滚轮向下回到底部) ")
    } else {
        " 日志 ".to_string()
    };
    let block = Block::bordered()
        .title(Line::from(Span::styled(
            title,
            Style::new().fg(if scroll > 0 { ACCENT } else { DIM }),
        )))
        .border_style(Style::new().fg(if scroll > 0 { ACCENT } else { DIM }));
    frame.render_widget(block, area);

    let lines: Vec<Line> = app
        .log
        .iter()
        .skip(start)
        .take(end - start)
        .map(|line| {
            Line::from(Span::styled(
                format!("  {line}"),
                Style::new().fg(Color::Rgb(0xc8, 0xc8, 0xce)),
            ))
        })
        .collect();
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
}

fn draw_footer(frame: &mut Frame, area: Rect, app: &mut App, layout: &mut Areas) {
    // (key, label) per screen: the label is what is drawn, the key is what a click
    // replays, so the hint and the binding cannot drift apart.
    let entries: Vec<(String, KeyCode)> = match app.screen {
        Screen::Main => vec![
            ("d 下载".to_string(), KeyCode::Char('d')),
            ("x 卸载".to_string(), KeyCode::Char('x')),
            ("a 账户".to_string(), KeyCode::Char('a')),
            ("v Java".to_string(), KeyCode::Char('v')),
            ("h 基础".to_string(), KeyCode::Char('h')),
            ("r 刷新".to_string(), KeyCode::Char('r')),
            ("q 退出".to_string(), KeyCode::Char('q')),
        ],
        Screen::Accounts => vec![
            ("l 登录".to_string(), KeyCode::Char('l')),
            ("d 移除".to_string(), KeyCode::Char('d')),
            ("Esc/右键 关闭".to_string(), KeyCode::Esc),
        ],
        Screen::Download => vec![
            ("/ 搜索".to_string(), KeyCode::Char('/')),
            ("f 类型".to_string(), KeyCode::Char('f')),
            ("Esc/右键 取消".to_string(), KeyCode::Esc),
        ],
        Screen::Installing => vec![("Esc/右键 后台运行".to_string(), KeyCode::Esc)],
        // The Java picker is a full-screen pane, so its keys are drawn inside it.
        Screen::Java => Vec::new(),
        Screen::Login => vec![("Esc/右键 取消".to_string(), KeyCode::Esc)],
    };

    let sep = " · ";
    // Drop whole entries from the left until the row fits, separator included: a
    // narrow terminal loses a hint instead of ending with a dangling `· `.
    let mut entries = entries;
    let mut buttons_width = 0usize;
    while !entries.is_empty() {
        buttons_width = entries
            .iter()
            .map(|(text, _)| cell_width_of(text))
            .sum::<usize>()
            + cell_width_of(sep) * entries.len().saturating_sub(1);
        // One column of breathing room, so the row never starts flush left.
        if buttons_width < area.width as usize {
            break;
        }
        entries.remove(0);
    }
    let area_width = area.width as usize;
    // Two columns of breathing room, so a long status never touches the buttons.
    let status_room = area_width.saturating_sub(buttons_width + 2);
    let full = format!(" {}", app.status);
    let status = if cell_width_of(&full) > status_room && status_room > 0 {
        format!("{}…", truncate_to_width(&full, status_room - 1))
    } else {
        truncate_to_width(&full, status_room)
    };
    let status_width = cell_width_of(&status);
    let buttons_column = area_width.saturating_sub(buttons_width);
    let gap = buttons_column.saturating_sub(status_width);

    let mut spans = vec![
        Span::styled(status, Style::new().fg(ACCENT)),
        Span::styled(" ".repeat(gap), Style::new()),
    ];
    let mut placed: Vec<(usize, usize, KeyCode)> = Vec::new();
    let mut column = buttons_column;
    for (index, (text, code)) in entries.iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled(sep, Style::new().fg(DIM)));
            column += cell_width_of(sep);
        }
        placed.push((column, cell_width_of(text), *code));
        spans.push(Span::styled(
            text.clone(),
            Style::new()
                .fg(Color::White)
                .add_modifier(Modifier::UNDERLINED),
        ));
        column += cell_width_of(text);
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)).wrap(Wrap { trim: false }),
        area,
    );

    // Record the clickable spans; anything past the edge is dropped.
    for (start, width, code) in placed {
        let x = area.x as usize + start;
        if x + width > area.x as usize + area.width as usize {
            continue;
        }
        record_button(layout, x as u16, area.y, width as u16, code);
    }
}

/// Cut `text` to at most `max` display cells, counting CJK as two.
fn truncate_to_width(text: &str, max: usize) -> String {
    let mut used = 0;
    let mut out = String::new();
    for ch in text.chars() {
        let width = ch.to_string().cell_width() as usize;
        if used + width > max {
            break;
        }
        used += width;
        out.push(ch);
    }
    out
}

/// Display width of a string (CJK counts as two cells).
fn cell_width_of(text: &str) -> usize {
    text.cell_width() as usize
}

/// The version + loader picker.
fn draw_download(frame: &mut Frame, area: Rect, app: &mut App, layout: &mut Areas) {
    // Full-screen pane: wipe whatever the main screen left behind, otherwise the
    // empty parts of the picker show stale rows.
    frame.render_widget(Clear, area);
    let title = Line::from(vec![
        Span::styled(" 下载 ", Style::new().fg(ACCENT).bold()),
        Span::styled(
            // Total, and how much of it the filter lets through.
            if app.download.matched() == app.download.all.len() {
                format!("{} · {} ", app.source_label(), app.download.all.len())
            } else {
                format!(
                    "{} · {} / {} ",
                    app.source_label(),
                    app.download.matched(),
                    app.download.all.len()
                )
            },
            Style::new().fg(DIM),
        ),
    ]);
    let block = Block::bordered()
        .title(title)
        .border_style(Style::new().fg(ACCENT));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    layout.download_versions = Area::default();
    layout.download_loaders = Area::default();
    layout.download_offset = 0;
    layout.download_search = Area::default();
    layout.download_filters.clear();

    if app.download.stage == crate::app::DownloadStage::LoadingVersions {
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(""),
                Line::from(Span::styled(
                    "正在获取版本列表…",
                    Style::new().fg(DIM).add_modifier(Modifier::BOLD),
                )),
                Line::from(Span::styled("首次可能需要几秒", Style::new().fg(DIM))),
            ])
            .alignment(Alignment::Center),
            inner,
        );
        return;
    }

    // The filter bar takes the top of the body, so both panes start below it.
    let [filter_area, body] =
        Layout::vertical([Constraint::Length(3), Constraint::Min(6)]).areas(inner);
    // The chips are a fixed row of four; the search box takes the rest.
    let [search_area, chips_area] =
        Layout::horizontal([Constraint::Min(20), Constraint::Length(30)]).areas(filter_area);
    let [versions_area, loaders_area] =
        Layout::horizontal([Constraint::Percentage(58), Constraint::Min(20)]).areas(body);

    let (search_hit, chips) = draw_filter_bar(frame, search_area, chips_area, &app.download);
    layout.download_search = search_hit;
    layout.download_filters = chips;

    // ---- versions ----
    let focused = app.download.focus == crate::app::PickerFocus::Versions;
    // The picker covers the footer, so the fold hint lives in the pane's title
    // rather than in a key label nobody can see.
    let folded = app.download.folded_rows();
    let versions_title = if folded > 0 {
        format!(" 版本 · {folded} 组折叠 · Space 展开 ")
    } else {
        " 版本 ".to_string()
    };
    let versions_block = Block::bordered()
        .title(Line::from(versions_title))
        .border_style(Style::new().fg(if focused { ACCENT } else { DIM }));
    let versions_inner = versions_block.inner(versions_area);
    frame.render_widget(versions_block, versions_area);
    layout.download_versions_pane = Area::new(
        versions_area.x,
        versions_area.y,
        versions_area.width,
        versions_area.height,
    );
    let visible = versions_inner.height.saturating_sub(1) as usize;
    if app.download.rows.is_empty() {
        layout.download_versions = Area::default();
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled(
                    "没有匹配的版本",
                    Style::new().fg(DIM).add_modifier(Modifier::BOLD),
                )),
                Line::from(Span::styled(
                    if app.download.all.is_empty() {
                        "版本列表还没到"
                    } else {
                        "Esc 清空搜索，或 f 换类型"
                    },
                    Style::new().fg(DIM),
                )),
            ])
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true }),
            versions_inner,
        );
    } else {
        let (offset, hit) = draw_version_rows(frame, versions_inner, &app.download, visible);
        layout.download_offset = offset;
        layout.download_versions = hit;
    }

    // ---- loaders ----
    let focused = app.download.focus == crate::app::PickerFocus::Loaders;
    let loaders_block = Block::bordered()
        .title(Line::from(" 安装内容 "))
        .border_style(Style::new().fg(if focused { ACCENT } else { DIM }));
    let loaders_inner = loaders_block.inner(loaders_area);
    frame.render_widget(loaders_block, loaders_area);
    layout.download_loaders_pane = Area::new(
        loaders_area.x,
        loaders_area.y,
        loaders_area.width,
        loaders_area.height,
    );
    // Data rows only, and the first of them is the first line of the pane: unlike
    // the version table this one has no header row, so adding one here would make
    // every click land on the row above the pointer.
    // The switch lives on the pane's last row, so the list and its hint keep one
    // row clear at the bottom.
    let switch_row = loaders_inner
        .y
        .saturating_add(loaders_inner.height.saturating_sub(1));
    let list_height = loaders_inner.height.saturating_sub(1);
    let loader_rows = (app.download.loaders.len() as u16).min(list_height);
    layout.download_loaders = Area::new(
        loaders_inner.x,
        loaders_inner.y,
        loaders_inner.width,
        loader_rows,
    );

    let rows: Vec<Row> = app
        .download
        .loaders
        .iter()
        .enumerate()
        .map(|(index, loader)| {
            let selected = index == app.download.loader_pick;
            let mut row = Row::new(vec![Cell::from(Line::from(vec![
                Span::styled(if selected { "▶ " } else { "  " }, Style::new().fg(ACCENT)),
                Span::styled(
                    loader.label(),
                    if selected {
                        Style::new().fg(ACCENT)
                    } else {
                        Style::new().fg(Color::White)
                    },
                ),
            ]))]);
            if selected {
                row = row.style(Style::new().bg(Color::Rgb(0x26, 0x28, 0x2e)));
            }
            row
        })
        .collect();
    frame.render_widget(Table::new(rows, [Constraint::Min(10)]), loaders_inner);

    // A hint under the loader list: what Enter will do. It starts right below the
    // last row, so it cannot be mistaken for one of them.
    if list_height > loader_rows {
        let hint_area = Rect {
            y: loaders_inner.y + loader_rows,
            height: loaders_inner.height.saturating_sub(loader_rows),
            ..loaders_inner
        };
        let loader = app.download.picked_loader();
        let text = if loader == Loader::None {
            "仅安装原版".to_string()
        } else {
            format!("{} · 自动选最新版本", loader.label())
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(text, Style::new().fg(DIM))))
                .wrap(Wrap { trim: true }),
            hint_area,
        );
    }

    draw_source_switch(frame, loaders_inner, switch_row, app, layout);
}

/// The download source switch, right-aligned on the loader pane's last row.
///
/// `--mirror` only decides where the launcher starts; from here on the pick can be
/// changed per install, and nothing about it is written to disk.
fn draw_source_switch(frame: &mut Frame, inner: Rect, row: u16, app: &mut App, layout: &mut Areas) {
    layout.download_sources.clear();
    let prefix = "下载源 ";
    let chips: Vec<(&str, bool)> = crate::install::Source::ALL
        .into_iter()
        .map(|source| (source.label(), app.source == source))
        .collect();
    let total = (cell_width_of(prefix)
        + chips
            .iter()
            .map(|(label, _)| cell_width_of(label) + 2)
            .sum::<usize>()) as u16;
    // A pane too narrow for the whole switch simply does not get one: it must not
    // push the loader rows around.
    if total > inner.width || inner.height == 0 {
        return;
    }
    // Right-aligned: the row itself is the widest thing that fits.
    let x = inner.x + inner.width - total;
    layout.download_sources = draw_source_chips(frame, x, row, prefix, &chips);
}

/// Draw `下载源 <chip> <chip>` at `x` and return one hit area per chip.
///
/// The download picker and the Java picker both have this switch; the chips differ
/// only in which enum says they are active.
fn draw_source_chips(
    frame: &mut Frame,
    x: u16,
    row: u16,
    prefix: &str,
    chips: &[(&str, bool)],
) -> Vec<Area> {
    let mut spans = vec![Span::styled(prefix.to_string(), Style::new().fg(DIM))];
    let mut column = x + cell_width_of(prefix) as u16;
    let mut areas = Vec::with_capacity(chips.len());
    for (label, active) in chips {
        let text = format!(" {label} ");
        let width = cell_width_of(&text) as u16;
        areas.push(Area::new(column, row, width, 1));
        spans.push(Span::styled(text, source_chip_style(*active)));
        column += width;
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)),
        Rect::new(x, row, column - x, 1),
    );
    areas
}

/// A source chip: filled when active, dim otherwise.
fn source_chip_style(active: bool) -> Style {
    if active {
        Style::new()
            .fg(Color::Black)
            .bg(ACCENT)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(DIM)
    }
}

/// The filter bar: a search box on the left, the kind chips on the right.
fn draw_filter_bar(
    frame: &mut Frame,
    search: Rect,
    chips: Rect,
    download: &crate::app::DownloadUi,
) -> (Area, Vec<Area>) {
    let focused = download.focus == crate::app::PickerFocus::Search;
    let search_block = Block::bordered()
        .title(Line::from(" 搜索 "))
        .border_style(Style::new().fg(if focused { ACCENT } else { DIM }));
    let search_inner = search_block.inner(search);
    frame.render_widget(search_block, search);
    let query = &download.query;
    let mut spans = Vec::new();
    if query.is_empty() {
        spans.push(Span::styled(
            if focused {
                "▏输入版本号，如 1.20 或 1.8"
            } else {
                "/ 搜索"
            },
            Style::new().fg(DIM),
        ));
    } else {
        spans.push(Span::styled(query.clone(), Style::new().fg(Color::White)));
    }
    if focused {
        // A caret, because the terminal's own cursor stays hidden.
        spans.push(Span::styled(
            "▏",
            Style::new().fg(ACCENT).add_modifier(Modifier::SLOW_BLINK),
        ));
    }
    let matched = download.matched();
    let total = download.all.len();
    if !download.query.is_empty() {
        spans.push(Span::styled(
            format!("  {matched}/{total}"),
            Style::new().fg(DIM),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), search_inner);

    let chips_block = Block::bordered()
        .title(Line::from(" 类型 "))
        .border_style(Style::new().fg(DIM));
    let chips_inner = chips_block.inner(chips);
    frame.render_widget(chips_block, chips);

    let mut chips_hit = Vec::new();
    let mut spans = Vec::new();
    let mut column = chips_inner.x;
    for filter in crate::app::VersionFilter::ALL {
        let active = download.filter == filter;
        let text = format!(" {} ", filter.label());
        let width = cell_width_of(&text) as u16;
        if column + width > chips_inner.x + chips_inner.width {
            break;
        }
        chips_hit.push(Area::new(column, chips_inner.y, width, 1));
        spans.push(Span::styled(
            text,
            if active {
                Style::new()
                    .fg(Color::Black)
                    .bg(ACCENT)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::new().fg(DIM)
            },
        ));
        column += width;
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), chips_inner);
    (
        Area::new(
            search_inner.x,
            search_inner.y,
            search_inner.width,
            search_inner.height,
        ),
        chips_hit,
    )
}

/// The version rows of the picker, header included.
///
/// The list is drawn from `download.rows`, so a folded snapshot family takes one
/// line and an opened one shows its members indented under the header. `picked`
/// and `versions` are untouched by the folding: a row is only an index into them.
fn draw_version_rows(
    frame: &mut Frame,
    inner: Rect,
    download: &crate::app::DownloadUi,
    visible: usize,
) -> (usize, Area) {
    let cursor = download.cursor();
    let offset = cursor.saturating_sub(visible.saturating_sub(1));
    // Data rows only: the header above them is not a clickable version.
    let hit = Area::new(
        inner.x,
        inner.y.saturating_add(1),
        inner.width,
        inner.height.saturating_sub(1),
    );
    let version = |index: usize| download.version(index);
    let rows: Vec<Row> = download
        .rows
        .iter()
        .enumerate()
        .skip(offset)
        .take(visible)
        .map(|(index, drawn)| {
            let selected = index == cursor;
            let dim = Style::new().fg(DIM);

            // Marker column, then one indent step per tree level, then the branch
            // marker for a group. A plain version keeps the old two-column look.
            let mut label = vec![Span::styled(
                if selected { "▶ " } else { "  " },
                Style::new().fg(ACCENT),
            )];
            for _ in 0..drawn.depth {
                label.push(Span::styled("  ", Style::new()));
            }

            let (name, kind, date) = match &drawn.kind {
                crate::version_tree::RowKind::Version(index) => {
                    let version = version(*index);
                    (
                        version.id.clone(),
                        Span::styled(
                            version.kind.clone(),
                            Style::new().fg(kind_color(&version.kind)),
                        ),
                        version.released.chars().take(10).collect::<String>(),
                    )
                }
                crate::version_tree::RowKind::Group {
                    first,
                    last,
                    expanded,
                    ..
                } => {
                    label.push(Span::styled(
                        if *expanded { "▾ " } else { "▸ " },
                        Style::new().fg(ACCENT),
                    ));
                    let kind = version(*first).kind.as_str();
                    (
                        crate::version_tree::title(
                            &download.all,
                            download.visible(),
                            *first,
                            *last,
                        ),
                        Span::styled(group_kind_label(kind), Style::new().fg(kind_color(kind))),
                        format!("{} 个", last - first + 1),
                    )
                }
            };
            label.push(Span::styled(
                name,
                if selected {
                    Style::new().fg(ACCENT)
                } else {
                    Style::new().fg(Color::White)
                },
            ));

            let mut row = Row::new(vec![
                Cell::from(Line::from(label)),
                Cell::from(Line::from(kind)),
                Cell::from(Line::from(Span::styled(date, dim))),
            ]);
            if selected {
                row = row.style(Style::new().bg(Color::Rgb(0x26, 0x28, 0x2e)));
            }
            row
        })
        .collect();
    frame.render_widget(
        Table::new(
            rows,
            [
                Constraint::Min(12),
                Constraint::Length(10),
                Constraint::Length(10),
            ],
        )
        .header(Row::new(vec!["  版本", "类型", "发布"]).style(Style::new().fg(DIM).bold()))
        .column_spacing(1),
        inner,
    );
    (offset, hit)
}

/// The Java runtime picker: four feature versions, a source switch and the proxy
/// field the GitHub source uses.
fn draw_java(frame: &mut Frame, area: Rect, app: &mut App, layout: &mut Areas) {
    frame.render_widget(Clear, area);
    layout.java_rows.clear();
    layout.java_sources.clear();
    layout.java_proxy = Area::default();

    let block = Block::bordered()
        .title(
            Line::from(format!(" 下载 Java（{}） ", app.java.source.label()))
                .style(Style::new().fg(ACCENT).bold()),
        )
        .border_style(Style::new().fg(ACCENT));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [table_area, switch_area, proxy_area, hint_area] = Layout::vertical([
        Constraint::Min(6),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);

    let (os, arch) = crate::java::platform();
    let table_block = Block::bordered()
        .title(Line::from(format!(
            " Java 运行时 · {os}/{arch} → {} ",
            app.runtimes.display()
        )))
        .border_style(Style::new().fg(DIM));
    let table_inner = table_block.inner(table_area);
    frame.render_widget(table_block, table_area);

    let header = Line::from(Span::styled(
        "  版本        最新 JRE              大小        状态".to_string(),
        Style::new().fg(DIM).bold(),
    ));
    let mut lines = vec![header];

    for (index, feature) in crate::java::FEATURES.iter().enumerate() {
        let selected = app.java.selected == index && !app.java.runtimes.is_empty();
        let runtime = app
            .java
            .runtimes
            .iter()
            .find(|runtime| runtime.major == feature.major);
        let version = runtime.map_or("—".to_string(), |runtime| runtime.version());
        let size = runtime.map_or("—".to_string(), |runtime| {
            crate::discover::human_size(runtime.size)
        });
        let update = runtime.and_then(|runtime| app.java.update_for(runtime));
        let running = app.java.stage == crate::app::JavaStage::Downloading
            && app.java.current == Some(feature.major);
        let status = if running {
            format!("下载中 {}", app.java.progress_text())
        } else if let Some(installed) = update {
            format!("可更新（{}）", installed.version)
        } else if app.java.is_installed(feature.major) {
            "已安装".to_string()
        } else {
            match (&app.java.message, app.java.current) {
                (Some(message), Some(current))
                    if feature.major == current
                        && app.java.stage == crate::app::JavaStage::Failed =>
                {
                    message.clone()
                }
                _ => "未安装".to_string(),
            }
        };
        let status_style = if running {
            Style::new().fg(ACCENT)
        } else if update.is_some() {
            Style::new().fg(SNAPSHOT)
        } else if app.java.stage == crate::app::JavaStage::Failed
            && app.java.current == Some(feature.major)
        {
            Style::new().fg(BROKEN)
        } else if app.java.is_installed(feature.major) {
            Style::new().fg(ACCENT)
        } else {
            Style::new().fg(DIM)
        };

        lines.push(Line::from(vec![
            Span::styled(if selected { "▶ " } else { "  " }, Style::new().fg(ACCENT)),
            Span::styled(
                format!("{:<12}", feature.label()),
                if selected {
                    Style::new().fg(ACCENT).bold()
                } else {
                    Style::new().fg(Color::White)
                },
            ),
            Span::styled(format!("{version:<20}"), Style::new().fg(Color::White)),
            Span::styled(format!("{size:<11}"), Style::new().fg(DIM)),
            Span::styled(status, status_style),
        ]));
    }

    // Row hit areas: the header takes the first line of the table, the rows follow.
    let rows_top = table_inner.y.saturating_add(1);
    for index in 0..crate::java::FEATURES.len() {
        layout.java_rows.push(Area::new(
            table_inner.x,
            rows_top + index as u16,
            table_inner.width,
            1,
        ));
    }
    if app.java.stage == crate::app::JavaStage::Loading {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  正在向 Adoptium 查询最新 JRE…",
            Style::new().fg(DIM),
        )));
    }
    frame.render_widget(Paragraph::new(lines), table_inner);

    // ---- source switch ----
    let chips: Vec<(&str, bool)> = crate::java::Source::ALL
        .into_iter()
        .map(|source| (source.label(), app.java.source == source))
        .collect();
    layout.java_sources =
        draw_source_chips(frame, switch_area.x, switch_area.y, " 下载源 ", &chips);

    // ---- proxy field ----
    let active = app.java.source == crate::java::Source::Github;
    let mut spans = vec![Span::styled(" GitHub Proxy ", Style::new().fg(DIM))];
    if app.java.proxy.is_empty() {
        spans.push(Span::styled(
            if active {
                "如 gh-proxy.com"
            } else {
                "（仅官方源使用）"
            },
            Style::new().fg(DIM),
        ));
    } else {
        spans.push(Span::styled(
            app.java.proxy.clone(),
            Style::new().fg(if active { ACCENT } else { DIM }),
        ));
    }
    if app.java.editing_proxy {
        spans.push(Span::styled(
            "▏",
            Style::new().fg(ACCENT).add_modifier(Modifier::SLOW_BLINK),
        ));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)),
        Rect::new(proxy_area.x, proxy_area.y, proxy_area.width, 1),
    );
    layout.java_proxy = Area::new(proxy_area.x, proxy_area.y, proxy_area.width, 1);

    // ---- keys ----
    let updates = app.java.update_count();
    let hint = if app.java.editing_proxy {
        " 输入代理地址 · Enter/Esc 结束编辑".to_string()
    } else if updates > 0 {
        format!(
            " ↑↓ 选择 · Enter 下载/更新 · u 全部更新（{updates}） · s 换源 · p 改代理 · Esc 返回"
        )
    } else {
        " ↑↓ 选择 · Enter 下载 · s 换源 · p 改代理 · Esc 返回".to_string()
    };
    draw_hint(
        frame,
        Rect::new(hint_area.x, hint_area.y, hint_area.width, 1),
        &[(" ", None), (&hint, None)],
        DIM,
        layout,
    );
}

/// The running install: log in the middle, progress pinned at the bottom.
/// Draw a gauge's label so it stays readable over both halves of the bar.
///
/// Inside the filled part the text is dark on the bar's colour; outside it is
/// light on the trough. Each run of characters gets its own span, so the switch
/// happens exactly where the bar ends.
fn draw_gauge_label(
    frame: &mut Frame,
    area: Rect,
    text: &str,
    fraction: f64,
    bar: Color,
    trough: Color,
) {
    if area.is_empty() {
        return;
    }
    let width = cell_width_of(text) as u16;
    let start = area.x + area.width.saturating_sub(width) / 2;
    let filled_end = area.x + (f64::from(area.width) * fraction.clamp(0.0, 1.0)).round() as u16;

    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut run = String::new();
    let mut run_dark = start < filled_end;
    let mut column = start;
    for ch in text.chars() {
        let dark = column < filled_end;
        if dark != run_dark && !run.is_empty() {
            spans.push(progress_label_span(
                std::mem::take(&mut run),
                run_dark,
                bar,
                trough,
            ));
            run_dark = dark;
        }
        run.push(ch);
        column += ch.to_string().cell_width();
    }
    if !run.is_empty() {
        spans.push(progress_label_span(run, run_dark, bar, trough));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)),
        Rect::new(start, area.y, width.min(area.width), 1),
    );
}

fn progress_label_span(text: String, dark: bool, bar: Color, trough: Color) -> Span<'static> {
    Span::styled(
        text,
        if dark {
            // On the bar: dark text, the bar's own colour behind it.
            Style::new().fg(Color::Black).bg(bar)
        } else {
            // In the trough: light text.
            Style::new().fg(Color::White).bg(trough)
        },
    )
}

fn draw_installing(frame: &mut Frame, area: Rect, app: &mut App, layout: &mut Areas) {
    frame.render_widget(Clear, area);
    let [log_area, progress_area] =
        Layout::vertical([Constraint::Min(6), Constraint::Length(5)]).areas(area);

    draw_log(frame, log_area, app, layout);

    let download = &app.download;
    let (title, border, gauge_color) = match download.stage {
        crate::app::DownloadStage::Done => (" 安装完成 ", ACCENT, ACCENT),
        crate::app::DownloadStage::Failed => (" 安装失败 ", BROKEN, BROKEN),
        crate::app::DownloadStage::Training => (" AOT 训练 ", SNAPSHOT, SNAPSHOT),
        _ => (" 安装进度 ", ACCENT, ACCENT),
    };
    let block = Block::bordered()
        .title(Line::from(title).style(Style::new().fg(border).bold()))
        .border_style(Style::new().fg(border));
    let inner = block.inner(progress_area);
    frame.render_widget(block, progress_area);

    match download.stage {
        crate::app::DownloadStage::Failed => {
            frame.render_widget(
                Paragraph::new(vec![
                    Line::from(Span::styled(
                        download.error.clone().unwrap_or_else(|| "未知错误".into()),
                        Style::new().fg(BROKEN),
                    )),
                    Line::from(Span::styled("Esc 返回", Style::new().fg(DIM))),
                ]),
                inner,
            );
            return;
        }
        crate::app::DownloadStage::Done => {
            let (version, loader) = download
                .installed
                .clone()
                .unwrap_or_else(|| ("?".to_string(), None));
            let mut lines = vec![Line::from(Span::styled(
                format!(
                    "{version}{} 已安装",
                    loader.map_or_else(String::new, |l| format!(" + {l}"))
                ),
                Style::new().fg(ACCENT),
            ))];
            if let Some(note) = &download.aot_note {
                lines.push(Line::from(Span::styled(note.clone(), Style::new().fg(DIM))));
            }
            lines.push(Line::from(Span::styled(
                "Esc 返回列表",
                Style::new().fg(DIM),
            )));
            frame.render_widget(Paragraph::new(lines), inner);
            return;
        }
        crate::app::DownloadStage::Training => {
            let ratio = download.aot_fraction();
            let label = if download.aot_preparing {
                "准备 Java 28 EA…".to_string()
            } else if download.aot_assembling {
                "组装 AOT 缓存…".to_string()
            } else if download.aot_total > 0 {
                format!(
                    "{}% · {} / {} 个候选类",
                    (ratio * 100.0).round() as u16,
                    download.aot_loaded,
                    download.aot_total
                )
            } else {
                format!("已加载 {} 个类", download.aot_loaded)
            };
            let trough = Color::Rgb(0x26, 0x28, 0x2e);
            let gauge = ratatui::widgets::Gauge::default()
                .gauge_style(Style::new().fg(SNAPSHOT).bg(trough))
                .ratio(ratio)
                .label(Span::raw(label.clone()));
            let [gauge_area, stats_area] =
                Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(inner);
            frame.render_widget(gauge, gauge_area);
            draw_gauge_label(frame, gauge_area, &label, ratio, SNAPSHOT, trough);
            let stats = if download.aot_preparing {
                "正在安装/查找 Java 28 EA，完成后自动开始训练"
            } else if download.aot_assembling {
                "AOT 缓存组装中（通常几分钟，Esc 可回主界面）"
            } else {
                "正在记录类加载 · 训练结束后自动退出并组装缓存"
            };
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(stats, Style::new().fg(DIM)))),
                stats_area,
            );
            return;
        }
        _ => {}
    }

    let percent = download.percent();
    let trough = Color::Rgb(0x26, 0x28, 0x2e);
    let label = format!(
        "{percent}%  {}/s",
        discover::human_size(download.speed as u64)
    );
    // The label is drawn twice on purpose: `Gauge` reserves and recolours the
    // cells under it, then `draw_gauge_label` writes the same text again with the
    // colour each side of the fill needs. Asking `Gauge` for a white label would
    // leave white text on the green bar, which is unreadable.
    let gauge = ratatui::widgets::Gauge::default()
        .gauge_style(Style::new().fg(gauge_color).bg(trough))
        .ratio(download.fraction())
        .label(Span::raw(label.clone()));
    let [gauge_area, stats_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(inner);
    frame.render_widget(gauge, gauge_area);
    draw_gauge_label(
        frame,
        gauge_area,
        &label,
        download.fraction(),
        gauge_color,
        trough,
    );

    let stats = Line::from(vec![
        Span::styled(
            format!(" {} / {} 个文件", download.done, download.total),
            Style::new().fg(Color::White),
        ),
        Span::styled("   ·   ", Style::new().fg(DIM)),
        Span::styled(
            format!("剩余 {}", discover::human_size(download.remaining_bytes)),
            Style::new().fg(DIM),
        ),
        Span::styled("   ·   ", Style::new().fg(DIM)),
        Span::styled(
            if download.batch_total_bytes > 0 {
                format!("本批 {}", discover::human_size(download.batch_total_bytes))
            } else {
                "本批大小未知".to_string()
            },
            Style::new().fg(DIM),
        ),
    ]);
    frame.render_widget(Paragraph::new(stats), stats_area);
}

/// The account manager, drawn over the main screen.
/// Draw one hint row whose key words are clickable.
///
/// `segments` are drawn left to right. A segment carrying a key is the whole
/// phrase the footer would draw (`Enter 切换`, not just `Enter`): it is underlined
/// as one unit and recorded in [`Layout::buttons`], so a click anywhere on the
/// words replays that key through the same path the footer buttons use. Segments
/// without a key are plain text in `color`.
/// Record one clickable span in the layout.
fn record_button(layout: &mut Areas, x: u16, y: u16, width: u16, code: KeyCode) {
    if width == 0 {
        return;
    }
    layout
        .buttons
        .push(Button::new(Area::new(x, y, width, 1), code));
}

fn draw_hint(
    frame: &mut Frame,
    area: Rect,
    segments: &[(&str, Option<KeyCode>)],
    color: Color,
    layout: &mut Areas,
) {
    let mut spans = Vec::with_capacity(segments.len());
    let mut x = area.x;
    for (text, key) in segments {
        let width = cell_width_of(text).min(u16::MAX as usize) as u16;
        let style = match key {
            Some(_) => Style::new()
                .fg(Color::White)
                .add_modifier(Modifier::UNDERLINED),
            None => Style::new().fg(color),
        };
        spans.push(Span::styled(*text, style));
        if let Some(code) = key {
            record_button(layout, x, area.y, width, *code);
        }
        x = x.saturating_add(width);
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_accounts(frame: &mut Frame, area: Rect, app: &mut App, layout: &mut Areas) {
    let width = area.width.saturating_sub(6).clamp(40, 72);
    let rows = app.accounts.len().clamp(1, 10) as u16;
    let height = (rows + 4).min(area.height);

    let popup = Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, popup);
    layout.accounts_popup = Area::new(popup.x, popup.y, popup.width, popup.height);
    layout.account_rows.clear();

    let block = Block::bordered()
        .title(
            Line::from(format!(" 账户 ({}) ", app.accounts.len()))
                .style(Style::new().fg(ACCENT).bold()),
        )
        .border_style(Style::new().fg(ACCENT));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    if app.accounts.is_empty() {
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(""),
                Line::from(Span::styled("还没有账户", Style::new().fg(DIM).bold())),
                Line::from(Span::styled("按 l 登录一个", Style::new().fg(DIM))),
            ])
            .alignment(Alignment::Center),
            inner,
        );
        return;
    }

    let visible = inner.height.saturating_sub(1) as usize;
    let offset = app
        .accounts_ui
        .selected
        .saturating_sub(visible.saturating_sub(1));
    // Two sessions of one player differ only by file, so that is the comparison.
    let active = app.account.file().map(std::path::Path::to_path_buf);

    let rows: Vec<Row> = app
        .accounts
        .iter()
        .enumerate()
        .skip(offset)
        .take(visible)
        .map(|(index, account)| {
            let is_selected = index == app.accounts_ui.selected;
            let is_active = active.as_deref() == Some(account.file.as_path());
            layout.account_rows.push(Area::new(
                inner.x,
                inner.y + (index - offset) as u16,
                inner.width,
                1,
            ));
            let mut row = Row::new(vec![
                Cell::from(Line::from(vec![
                    Span::styled(
                        if is_selected { "▶ " } else { "  " },
                        Style::new().fg(ACCENT),
                    ),
                    Span::styled(
                        account.player_name.clone(),
                        if is_selected {
                            Style::new().fg(ACCENT)
                        } else {
                            Style::new().fg(Color::White)
                        },
                    ),
                    // The number of a repeated session, always dimmed.
                    Span::styled(crate::accounts::suffix(account.copy), Style::new().fg(DIM)),
                ])),
                Cell::from(Line::from(Span::styled(
                    account.application.clone(),
                    Style::new().fg(DIM),
                ))),
                Cell::from(Line::from(Span::styled(
                    if is_active { "当前" } else { "" },
                    Style::new().fg(ACCENT),
                ))),
            ]);
            if is_selected {
                row = row.style(Style::new().bg(Color::Rgb(0x26, 0x28, 0x2e)));
            }
            row
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Min(14),
            Constraint::Min(10),
            Constraint::Length(4),
        ],
    )
    .column_spacing(1);
    frame.render_widget(table, inner);

    let hint_area = Rect {
        y: inner.y + inner.height.saturating_sub(1),
        height: 1,
        ..inner
    };
    if app.accounts_ui.confirm_delete {
        draw_hint(
            frame,
            hint_area,
            &[
                (" 再按一次 ", None),
                ("d 确认移除", Some(KeyCode::Char('d'))),
                (" · ", None),
                ("Esc 取消", Some(KeyCode::Esc)),
            ],
            BROKEN,
            layout,
        );
    } else {
        draw_hint(
            frame,
            hint_area,
            &[
                (" ", None),
                ("Enter 切换", Some(KeyCode::Enter)),
                (" · ", None),
                ("d 移除", Some(KeyCode::Char('d'))),
                (" · ", None),
                ("l 登录新账户", Some(KeyCode::Char('l'))),
                (" · ", None),
                ("Esc 关闭", Some(KeyCode::Esc)),
            ],
            DIM,
            layout,
        );
    }
}

/// The login overlay, drawn over the main screen.
fn draw_login(frame: &mut Frame, area: Rect, app: &mut App, layout: &mut Areas) {
    // The step is cloned so the per-step helpers can take `&mut App`: the
    // overlay mutates its scroll offset while drawing.
    let step = app.login.step.clone();
    let width = area.width.saturating_sub(4).clamp(36, 68);
    let height = match &step {
        LoginStep::Choose => 10,
        LoginStep::EnterAppId => 11,
        LoginStep::ChooseAppIds => {
            if app.login.app_ids.is_empty() {
                10
            } else {
                (app.login.app_ids.len().min(SAVED_ROWS) * 2 + 4) as u16
            }
        }
        LoginStep::Waiting { .. } => 12,
        LoginStep::Done { .. } | LoginStep::Failed(_) => 8,
    }
    .min(area.height);

    let popup = Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, popup);
    layout.overlay = Area::new(popup.x, popup.y, popup.width, popup.height);
    layout.overlay_options.clear();

    let title = match &step {
        LoginStep::Choose => " 登录 · 选择身份 ",
        LoginStep::EnterAppId => " 登录 · 自定义 app id ",
        LoginStep::ChooseAppIds => " 登录 · 现有凭据 ",
        LoginStep::Waiting { .. } => " 登录 · 等待授权 ",
        LoginStep::Done { .. } => " 登录 · 成功 ",
        LoginStep::Failed(_) => " 登录 · 失败 ",
    };
    let block = Block::bordered()
        .title(Line::from(title).style(Style::new().fg(ACCENT).bold()))
        .border_style(Style::new().fg(ACCENT));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    match &step {
        LoginStep::Choose => draw_login_choose(frame, inner, app, layout),
        LoginStep::EnterAppId => draw_login_app_id(frame, inner, app, layout),
        LoginStep::ChooseAppIds => draw_login_saved(frame, inner, app, layout),
        LoginStep::Waiting { user_code, url } => {
            draw_login_waiting(frame, inner, layout, user_code, url);
        }
        LoginStep::Done {
            player,
            application,
        } => draw_login_done(frame, inner, layout, player, application),
        LoginStep::Failed(message) => draw_login_failed(frame, inner, layout, message),
    }
}

/// The identity screen: official, custom app id, or the saved list.
fn draw_login_choose(frame: &mut Frame, inner: Rect, app: &mut App, layout: &mut Areas) {
    let options = [
        (
            "官方启动器",
            "Minecraft Launcher 的 title client id，走设备令牌 + SISU",
        ),
        (
            "自定义 app id",
            "其他启动器的 Azure 应用 client id，走 user + XSTS",
        ),
        (
            "现有凭据……",
            "从 凭据/启动器/azure_app_ids.toml 读取的 app id 清单",
        ),
    ];
    let mut lines = Vec::new();
    for (index, (name, hint)) in options.iter().enumerate() {
        // Two clickable rows per option: the name and its hint.
        layout.overlay_options.push(Area::new(
            inner.x,
            inner.y.saturating_add((index as u16) * 2),
            inner.width,
            2,
        ));
        let chosen = app.login.choice == index;
        lines.push(Line::from(vec![
            Span::styled(if chosen { " ▶ " } else { "   " }, Style::new().fg(ACCENT)),
            Span::styled(
                *name,
                if chosen {
                    Style::new().fg(ACCENT).bold()
                } else {
                    Style::new().fg(Color::White)
                },
            ),
        ]));
        lines.push(Line::from(Span::styled(
            format!("     {hint}"),
            Style::new().fg(DIM),
        )));
    }
    lines.push(Line::from(""));
    let hint_row = inner.y.saturating_add(lines.len() as u16);
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
    draw_hint(
        frame,
        Rect::new(inner.x, hint_row, inner.width, 1),
        &[
            (" ↑↓ 选择", None),
            (" · ", None),
            ("Enter 下一步", Some(KeyCode::Enter)),
            (" · ", None),
            ("Esc 取消", Some(KeyCode::Esc)),
        ],
        DIM,
        layout,
    );
}

/// The custom-app-id input. Enter is only offered once something was typed.
fn draw_login_app_id(frame: &mut Frame, inner: Rect, app: &mut App, layout: &mut Areas) {
    let filled = !app.login.app_id.trim().is_empty();
    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            " 输入启动器的 Azure 应用 client id：",
            Style::new().fg(Color::White),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("  ", Style::new()),
            Span::styled(app.login.app_id.clone(), Style::new().fg(ACCENT)),
            Span::styled("▏", Style::new().fg(ACCENT)),
        ]),
        Line::from(""),
    ];
    let hint_row = inner.y.saturating_add(lines.len() as u16);
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
    if filled {
        draw_hint(
            frame,
            Rect::new(inner.x, hint_row, inner.width, 1),
            &[
                (" ", None),
                ("Enter 开始登录", Some(KeyCode::Enter)),
                (" · ", None),
                ("Esc 返回", Some(KeyCode::Esc)),
            ],
            DIM,
            layout,
        );
    } else {
        // Nothing can be started yet, so only the way back is clickable.
        draw_hint(
            frame,
            Rect::new(inner.x, hint_row, inner.width, 1),
            &[
                (
                    " 不能为空 · 例如 Prism 的 c36a9fb6-4f2a-41ff-90bd-ae7cc92031eb",
                    None,
                ),
                (" · ", None),
                ("Esc 返回", Some(KeyCode::Esc)),
            ],
            BROKEN,
            layout,
        );
    }
}

/// The saved app ids, windowed so a long list scrolls under the selection.
fn draw_login_saved(frame: &mut Frame, inner: Rect, app: &mut App, layout: &mut Areas) {
    let mut lines = Vec::new();
    if let Some(error) = &app.login.app_ids_error {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            " 没有可用的 app id",
            Style::new().fg(BROKEN).bold(),
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!(" {error}"),
            Style::new().fg(Color::White),
        )));
        lines.push(Line::from(""));
        let hint_row = inner.y.saturating_add(lines.len() as u16);
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
        draw_hint(
            frame,
            Rect::new(inner.x, hint_row, inner.width, 1),
            &[
                (" ", None),
                ("Esc 返回", Some(KeyCode::Esc)),
                (" · 编辑上面这个文件后按 ", None),
                ("Enter 重试", Some(KeyCode::Enter)),
            ],
            DIM,
            layout,
        );
    } else {
        // One row of height is enough per entry (name + id on one line),
        // but two keeps a long id readable when the popup is narrow.
        let visible = ((inner.height.saturating_sub(2) / 2).max(1)) as usize;
        let last = app.login.app_ids.len().saturating_sub(1);
        let selected = app.login.app_id_choice.min(last);
        // Scroll so the selection stays on screen, then clamp the window
        // so the list never starts past its own end.
        let mut offset = app.login.app_id_offset.min(selected);
        if selected >= offset + visible {
            offset = selected + 1 - visible;
        }
        offset = offset.min(last.saturating_sub(visible.saturating_sub(1)));
        app.login.app_id_offset = offset;
        app.login.app_id_choice = selected;

        for (row, entry) in app
            .login
            .app_ids
            .iter()
            .enumerate()
            .skip(offset)
            .take(visible)
        {
            layout.overlay_options.push(Area::new(
                inner.x,
                inner.y.saturating_add(((row - offset) * 2) as u16),
                inner.width,
                2,
            ));
            let chosen = row == selected;
            lines.push(Line::from(vec![
                Span::styled(if chosen { " ▶ " } else { "   " }, Style::new().fg(ACCENT)),
                Span::styled(
                    entry.label().to_string(),
                    if chosen {
                        Style::new().fg(ACCENT).bold()
                    } else {
                        Style::new().fg(Color::White)
                    },
                ),
            ]));
            lines.push(Line::from(Span::styled(
                format!("     {}", entry.client_id()),
                Style::new().fg(DIM),
            )));
        }
        let more = app.login.app_ids.len().saturating_sub(offset + visible);
        let tail = if app.login.app_ids.len() > visible {
            format!(" · 共 {} 项，下面还有 {} 项", app.login.app_ids.len(), more)
        } else {
            String::new()
        };
        let hint_row = inner.y.saturating_add(lines.len() as u16);
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
        draw_hint(
            frame,
            Rect::new(inner.x, hint_row, inner.width, 1),
            &[
                (" ↑↓ 选择", None),
                (" · ", None),
                ("Enter 登录", Some(KeyCode::Enter)),
                (" · ", None),
                ("Esc 返回", Some(KeyCode::Esc)),
                (&tail, None),
            ],
            DIM,
            layout,
        );
    }
}

/// Waiting for the browser: the link, the code, and the copy/open buttons.
fn draw_login_waiting(
    frame: &mut Frame,
    inner: Rect,
    layout: &mut Areas,
    user_code: &str,
    url: &str,
) {
    // Rows 3 and 7 are drawn by `draw_hint` (the copy/open buttons); they
    // stay in the paragraph as empty lines so everything below keeps its
    // place.
    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            " 1. 在浏览器打开下面地址",
            Style::new().fg(DIM),
        )),
        Line::from(Span::styled(
            format!("    {url}"),
            Style::new().fg(Color::White),
        )),
        Line::from(""),
        Line::from(""),
        Line::from(Span::styled(" 2. 输入代码", Style::new().fg(DIM))),
        Line::from(Span::styled(
            format!("    {user_code}"),
            Style::new().fg(ACCENT).bold(),
        )),
        Line::from(""),
        Line::from(""),
        Line::from(""),
    ];
    let row = |index: usize| inner.y.saturating_add(index as u16);
    let (url_row, code_row, hint_row) = (row(3), row(7), row(9));
    frame.render_widget(Paragraph::new(lines), inner);
    draw_hint(
        frame,
        Rect::new(inner.x, url_row, inner.width, 1),
        &[
            (" ", None),
            ("o 浏览器打开", Some(KeyCode::Char('o'))),
            (" · ", None),
            ("y 复制链接", Some(KeyCode::Char('y'))),
        ],
        DIM,
        layout,
    );
    draw_hint(
        frame,
        Rect::new(inner.x, code_row, inner.width, 1),
        &[(" ", None), ("c 复制设备码", Some(KeyCode::Char('c')))],
        DIM,
        layout,
    );
    draw_hint(
        frame,
        Rect::new(inner.x, hint_row, inner.width, 1),
        &[
            (" 等待浏览器确认… ", None),
            ("Esc 取消", Some(KeyCode::Esc)),
        ],
        DIM,
        layout,
    );
}

/// The success screen, after the credential file was written.
fn draw_login_done(
    frame: &mut Frame,
    inner: Rect,
    layout: &mut Areas,
    player: &str,
    application: &str,
) {
    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            format!(" 已登录：{player}"),
            Style::new().fg(ACCENT).bold(),
        )),
        Line::from(Span::styled(
            format!(" 身份：{application}"),
            Style::new().fg(Color::White),
        )),
        Line::from(""),
    ];
    let hint_row = inner.y.saturating_add(lines.len() as u16);
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
    draw_hint(
        frame,
        Rect::new(inner.x, hint_row, inner.width, 1),
        &[
            (" 凭据已写入 data/凭据/账户/", None),
            (" · ", None),
            ("Enter 返回", Some(KeyCode::Enter)),
            (" · ", None),
            ("Esc 关闭", Some(KeyCode::Esc)),
        ],
        DIM,
        layout,
    );
}

/// The failure screen: the reason and the way back.
fn draw_login_failed(frame: &mut Frame, inner: Rect, layout: &mut Areas, message: &str) {
    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(" 登录失败", Style::new().fg(BROKEN).bold())),
        Line::from(""),
        Line::from(Span::styled(
            format!(" {message}"),
            Style::new().fg(Color::White),
        )),
        Line::from(""),
    ];
    let hint_row = inner.y.saturating_add(lines.len() as u16);
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
    draw_hint(
        frame,
        Rect::new(inner.x, hint_row, inner.width, 1),
        &[
            (" ", None),
            ("Esc 返回", Some(KeyCode::Esc)),
            (" · 检查网络/代理后重试", None),
        ],
        DIM,
        layout,
    );
}

/// Turn a rendered buffer into plain text rows.
///
/// A double-width cell (any CJK character) occupies one cell but two columns, and
/// the buffer stores a placeholder in the following cell. Writing every cell would
/// turn `未登录` into `未 登 录`, so the placeholders are skipped the same way the
/// terminal backend does.
pub fn buffer_lines(buffer: &ratatui::buffer::Buffer) -> Vec<String> {
    let area = buffer.area;
    (0..area.height)
        .map(|y| {
            let mut line = String::new();
            let mut skip = 0u16;
            for x in 0..area.width {
                let cell = &buffer[(x, y)];
                if skip == 0 {
                    line.push_str(cell.symbol());
                }
                skip = skip.max(cell.cell_width()).saturating_sub(1);
            }
            line.trim_end().to_string()
        })
        .collect()
}

fn kv(key: &str, value: String) -> (String, String) {
    (key.to_string(), value)
}

fn kind_label(version: &LocalVersion) -> String {
    if version.kind != "-" {
        return version.kind.clone();
    }
    match version.loader_label() {
        Some(loader) => loader.to_string(),
        None => "未完成".to_string(),
    }
}

/// A folded group names its kind the way a single row would.
///
/// The group is one line for many versions, so the raw `old_alpha` is spelled out
/// rather than left as `snapshot`, which would be wrong for the pre-beta eras.
fn group_kind_label(kind: &str) -> &str {
    match kind {
        "snapshot" => "快照",
        "old_alpha" => "旧版 Alpha",
        "old_beta" => "旧版 Beta",
        other => other,
    }
}

fn kind_color(kind: &str) -> Color {
    match kind {
        "release" => ACCENT,
        "snapshot" => SNAPSHOT,
        "old_beta" | "old_alpha" => LEGACY,
        "-" => BROKEN,
        _ => DIM,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::discover::LocalVersion;
    use crate::install::InstallEvent;
    use crossterm::event::KeyModifiers;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn login(app: &App) -> Option<crate::login::Identity> {
        app.effects().iter().rev().find_map(|effect| match effect {
            crate::app::Effect::Login(identity) => Some(identity.clone()),
            _ => None,
        })
    }

    fn copied(app: &App) -> Option<String> {
        app.effects().iter().rev().find_map(|effect| match effect {
            crate::app::Effect::Copy(text) => Some(text.clone()),
            _ => None,
        })
    }

    fn opened(app: &App) -> Option<String> {
        app.effects().iter().rev().find_map(|effect| match effect {
            crate::app::Effect::OpenBrowser(url) => Some(url.clone()),
            _ => None,
        })
    }

    fn version(id: &str, kind: &str, java: u32, inherits: Option<&str>) -> LocalVersion {
        LocalVersion {
            dir: id.to_string(),
            id: id.to_string(),
            kind: kind.to_string(),
            java: Some(java),
            inherits: inherits.map(str::to_string),
            main_class: Some("net.minecraft.client.main.Main".to_string()),
            has_jar: true,
            release_time: None,
            is_base_only: false,
        }
    }

    /// Render into a fixed-size buffer and return the rows as text.
    fn render(app: &mut App, width: u16, height: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let mut layout = Areas::default();
        terminal
            .draw(|frame| {
                layout = draw(frame, app);
            })
            .expect("draw to the test backend");
        // What the main loop does with the returned layout: keep it for hit tests.
        app.layout = layout;
        buffer_lines(terminal.backend().buffer())
    }

    #[test]
    fn every_panel_renders_with_versions() {
        let mut app = App::new(
            "/mc",
            "/acc",
            vec![
                version("1.20.1+Fabric0.15.11", "-", 17, Some("1.20.1")),
                version("1.20.1", "release", 17, None),
                version("1.8.9", "release", 8, None),
            ],
            vec![],
        );
        let rows = render(&mut app, 100, 28);
        let screen = rows.join("\n");

        assert!(screen.contains("yaml"), "title missing:\n{screen}");
        assert!(screen.contains("未登录"), "account missing:\n{screen}");
        assert!(
            screen.contains("1.20.1+Fabric0.15.11"),
            "list missing:\n{screen}"
        );
        assert!(screen.contains("详情"), "details missing:\n{screen}");
        assert!(screen.contains("日志"), "log missing:\n{screen}");
        assert!(
            screen.contains("d 下载"),
            "footer buttons missing:\n{screen}"
        );
        assert!(
            screen.contains("q 退出"),
            "footer buttons missing:\n{screen}"
        );
        assert!(screen.contains("▶"), "selection marker missing:\n{screen}");
    }

    #[test]
    fn full_screen_panes_do_not_draw_the_main_screen_underneath() {
        let mut app = App::new(
            "/mc",
            "/acc",
            vec![version("1.20.1", "release", 17, None)],
            vec![],
        );

        app.screen = Screen::Java;
        let screen = render(&mut app, 100, 28).join("\n");
        assert!(screen.contains("下载 Java"), "{screen}");
        assert!(
            !screen.contains("详情") && !screen.contains(" yaml "),
            "the main screen is skipped, not painted over:\n{screen}"
        );

        app.screen = Screen::Download;
        let screen = render(&mut app, 100, 28).join("\n");
        assert!(screen.contains("下载 BMCLAPI"), "{screen}");
        assert!(!screen.contains("详情"), "{screen}");
    }

    #[test]
    fn login_overlay_offers_both_identities_and_never_pretends_to_have_an_id() {
        let mut app = App::new(
            "/mc",
            "/acc",
            vec![version("1.20.1", "release", 17, None)],
            vec![],
        );
        app.open_login();
        let screen = render(&mut app, 100, 28).join("\n");
        assert!(screen.contains("选择身份"), "{screen}");
        assert!(screen.contains("官方启动器"), "{screen}");
        assert!(screen.contains("自定义 app id"), "{screen}");
        assert!(
            !screen.contains("00000000402b5328"),
            "no id is implied:\n{screen}"
        );

        // Second step: the typed client id is what gets used.
        app.login.choice = 1;
        app.login.step = crate::app::LoginStep::EnterAppId;
        app.login.app_id = "c36a9fb6-4f2a-41ff-90bd-ae7cc92031eb".to_string();
        let screen = render(&mut app, 100, 28).join("\n");
        assert!(screen.contains("c36a9fb6"), "{screen}");
        assert!(screen.contains("Enter 开始登录"), "{screen}");
    }

    #[test]
    fn the_identity_screen_offers_the_saved_list_as_a_third_choice() {
        let mut app = App::new("/mc", "/acc", vec![], vec![]);
        app.open_login();
        let screen = render(&mut app, 100, 28).join("\n");
        assert!(screen.contains("官方启动器"), "{screen}");
        assert!(screen.contains("自定义 app id"), "{screen}");
        assert!(screen.contains("现有凭据"), "{screen}");
        // Where the list comes from is part of the choice, not a surprise later.
        assert!(screen.contains("azure_app_ids.toml"), "{screen}");
        assert_eq!(
            app.layout.overlay_options.len(),
            3,
            "every row is clickable"
        );
    }

    fn saved(name: &str, key: &str) -> crate::app_ids::AppId {
        crate::app_ids::AppId {
            name: name.to_string(),
            key: key.to_string(),
        }
    }

    #[test]
    fn the_saved_list_shows_names_with_their_ids_and_marks_the_selection() {
        let mut app = App::new("/mc", "/acc", vec![], vec![]);
        app.open_login();
        app.login.step = crate::app::LoginStep::ChooseAppIds;
        app.login.app_ids = vec![
            saved("HMCL", "6a3728d6-27a3-4180-99bb-479895b8f88e"),
            saved("Prism", "c36a9fb6-4f2a-41ff-90bd-ae7cc92031eb"),
        ];
        app.login.app_id_choice = 1;
        let screen = render(&mut app, 100, 28).join("\n");

        assert!(screen.contains("现有凭据"), "{screen}");
        assert!(screen.contains("HMCL"), "{screen}");
        assert!(screen.contains("6a3728d6"), "{screen}");
        assert!(screen.contains("Prism"), "{screen}");
        assert!(screen.contains("c36a9fb6"), "{screen}");
        assert!(
            screen.contains("▶ Prism"),
            "selection marker missing:\n{screen}"
        );
        assert!(screen.contains("Enter 登录"), "{screen}");
        assert_eq!(app.layout.overlay_options.len(), 2);
    }

    #[test]
    fn a_long_saved_list_scrolls_to_keep_the_selection_visible() {
        let mut app = App::new("/mc", "/acc", vec![], vec![]);
        app.open_login();
        app.login.step = crate::app::LoginStep::ChooseAppIds;
        app.login.app_ids = (0..8)
            .map(|n| saved(&format!("launcher-{n}"), &format!("{n}0000000-0000")))
            .collect();
        app.login.app_id_choice = 7;
        let screen = render(&mut app, 100, 28).join("\n");

        assert!(screen.contains("launcher-7"), "{screen}");
        assert!(
            !screen.contains("launcher-0"),
            "the window should have scrolled past the first row:\n{screen}"
        );
        assert!(screen.contains("共 8 项"), "{screen}");
        assert_eq!(app.layout.overlay_options.len(), 6, "six rows fit");
    }

    #[test]
    fn an_unreadable_saved_list_says_why_instead_of_showing_rows() {
        let mut app = App::new("/mc", "/acc", vec![], vec![]);
        app.open_login();
        app.login.step = crate::app::LoginStep::ChooseAppIds;
        app.login.app_ids_error = Some(
            "未找到 /data/凭据/启动器/azure_app_ids.toml：按 README 里的格式写入 app id".into(),
        );
        let screen = render(&mut app, 100, 40).join("\n");

        assert!(screen.contains("没有可用的 app id"), "{screen}");
        assert!(screen.contains("azure_app_ids.toml"), "{screen}");
        assert!(screen.contains("Esc 返回"), "{screen}");
        assert!(app.layout.overlay_options.is_empty(), "nothing to click");
    }

    #[test]
    fn waiting_overlay_shows_the_code_and_the_link() {
        let mut app = App::new("/mc", "/acc", vec![], vec![]);
        app.open_login();
        app.handle_login_event(crate::login::LoginEvent::DeviceCode {
            user_code: "ABCD-EFGH".to_string(),
            url: "https://microsoft.com/link".to_string(),
        });
        let screen = render(&mut app, 100, 28).join("\n");
        assert!(screen.contains("ABCD-EFGH"), "{screen}");
        assert!(screen.contains("microsoft.com/link"), "{screen}");
        assert!(screen.contains("等待"), "{screen}");
    }

    #[test]
    fn the_waiting_screen_offers_copy_and_open_buttons_under_what_they_copy() {
        let mut app = App::new("/mc", "/acc", vec![], vec![]);
        app.open_login();
        app.handle_login_event(crate::login::LoginEvent::DeviceCode {
            user_code: "ABCD-EFGH".to_string(),
            url: "https://www.microsoft.com/link?otc=ABCDEFGH".to_string(),
        });
        let screen = render(&mut app, 100, 30).join("\n");

        // Under the link: open it, or copy it. Under the code: copy the code.
        assert!(screen.contains("o 浏览器打开"), "{screen}");
        assert!(screen.contains("y 复制链接"), "{screen}");
        assert!(screen.contains("c 复制设备码"), "{screen}");

        // Each label is one clickable button that replays its key.
        let click_key = |app: &mut App, code: KeyCode| {
            let area = app
                .layout
                .buttons
                .iter()
                .find(|button| {
                    button.code == code && app.layout.overlay.contains(button.area.x, button.area.y)
                })
                .map(|button| button.area)
                .unwrap_or_else(|| panic!("no {code:?} button in the popup"));
            app.handle_mouse(click(area.x + area.width - 1, area.y));
        };
        click_key(&mut app, KeyCode::Char('y'));
        assert_eq!(
            copied(&app).as_deref(),
            Some("https://www.microsoft.com/link?otc=ABCDEFGH")
        );

        render(&mut app, 100, 30);
        click_key(&mut app, KeyCode::Char('c'));
        assert_eq!(copied(&app).as_deref(), Some("ABCD-EFGH"));

        render(&mut app, 100, 30);
        click_key(&mut app, KeyCode::Char('o'));
        assert_eq!(
            opened(&app).as_deref(),
            Some("https://www.microsoft.com/link?otc=ABCDEFGH")
        );
    }

    #[test]
    fn a_scrolled_log_shows_older_lines_and_says_how_far_back() {
        let mut app = App::new("/mc", "/acc", vec![], vec![]);
        for i in 0..200 {
            app.log(format!("entry-{i:03}"));
        }
        app.scroll_log(5);
        let screen = render(&mut app, 100, 28).join("\n");

        assert!(
            screen.contains("日志 (5 行之前"),
            "no scrollback hint:\n{screen}"
        );
        assert!(
            !screen.contains("entry-199"),
            "newest line should be off-screen"
        );
        assert!(
            screen.contains("entry-194"),
            "the scrolled window is wrong:\n{screen}"
        );
    }

    #[test]
    fn the_log_records_its_area_so_the_wheel_can_target_it() {
        let mut app = App::new("/mc", "/acc", vec![], vec![]);
        render(&mut app, 100, 28);
        assert!(app.layout.log.width > 0 && app.layout.log.height > 0);
        assert!(app.layout.list.width > 0, "list area recorded");
        assert!(app.layout.account.width > 0, "account button recorded");
    }

    #[test]
    fn the_account_manager_lists_accounts_and_marks_the_active_one() {
        let tmp = tempfile::tempdir().unwrap();
        for name in ["Alex", "Steve"] {
            crate::accounts::save(
                tmp.path(),
                "Minecraft Launcher",
                "id",
                name,
                "uuid",
                &serde_json::json!({}),
            )
            .unwrap();
        }
        let saved = crate::accounts::list(tmp.path());
        let mut app = App::new("/mc", tmp.path(), vec![], saved);
        app.open_accounts();
        let screen = render(&mut app, 100, 28).join("\n");

        assert!(screen.contains("账户 (2)"), "{screen}");
        assert!(screen.contains("Alex"), "{screen}");
        assert!(screen.contains("Steve"), "{screen}");
        assert!(screen.contains("Enter 切换"), "{screen}");
        // Rows are recorded for hit testing.
        assert_eq!(app.layout.account_rows.len(), 2, "{screen}");
    }

    /// The cells of one row as `(symbol, foreground)` pairs, in draw order.
    ///
    /// The text rows from [`buffer_lines`] collapse the placeholder cells of
    /// wide glyphs, so a colour has to be looked up on the buffer itself.
    fn row_cells(
        app: &mut App,
        (width, height): (u16, u16),
        y: u16,
    ) -> Vec<(String, ratatui::style::Color)> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| {
                draw(frame, app);
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        (0..width)
            .map(|x| {
                let cell = buffer.cell((x, y)).expect("cell inside the buffer");
                (cell.symbol().to_string(), cell.fg)
            })
            .collect()
    }

    /// Foreground and background of one cell in a freshly drawn frame.
    fn colors_at(
        app: &mut App,
        (width, height): (u16, u16),
        x: u16,
        y: u16,
    ) -> (ratatui::style::Color, ratatui::style::Color) {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| {
                draw(frame, app);
            })
            .unwrap();
        let cell = terminal.backend().buffer().cell((x, y)).expect("cell");
        (cell.fg, cell.bg)
    }

    /// Where `text` starts in a row of cells, ignoring which glyph is wide.
    fn find_cells(row: &[(String, ratatui::style::Color)], text: &[&str]) -> Option<usize> {
        row.windows(text.len()).position(|window| {
            window
                .iter()
                .zip(text)
                .all(|((symbol, _), want)| symbol == want)
        })
    }

    #[test]
    fn a_repeated_session_is_numbered_and_the_number_is_dimmed() {
        let tmp = tempfile::tempdir().unwrap();
        for (app_id, application) in [("app-official", "Minecraft Launcher"), ("app-hmcl", "HMCL")]
        {
            crate::accounts::save(
                tmp.path(),
                application,
                app_id,
                "Steve",
                "uuid",
                &serde_json::json!({}),
            )
            .unwrap();
        }
        let saved = crate::accounts::list(tmp.path());
        let mut app = App::new("/mc", tmp.path(), vec![], saved);
        app.accounts_ui.selected = 1;
        app.activate_selected_account();

        // The header names the session, and only the `(1)` is grey.
        let screen = render(&mut app, 100, 32);
        let header = &screen[app.layout.account.y as usize];
        assert!(header.contains("Steve (1) · HMCL"), "{header}");
        let header_y = app.layout.account.y;
        let row = row_cells(&mut app, (100, 32), header_y);
        let paren = find_cells(&row, &["(", "1", ")"]).expect("the number is drawn");
        for (symbol, fg) in &row[paren..paren + 3] {
            assert_eq!(*fg, DIM, "the number {symbol} must be dimmed");
        }
        // The name itself keeps the accent colour, i.e. is not part of the grey.
        let steve = find_cells(&row, &["S", "t", "e", "v", "e"]).expect("the name is drawn");
        assert_eq!(row[steve].1, ACCENT, "the name keeps its colour");
        // The grey span is exactly the number: the separator after it is accent again.
        assert_eq!(row[paren + 3].1, ACCENT, "the dim span ends at the number");

        // The manager lists both sessions as separate rows, one of them active.
        app.open_accounts();
        let screen = render(&mut app, 100, 32).join("\n");
        assert!(screen.contains("账户 (2)"), "{screen}");
        assert!(screen.contains("Steve (1)"), "{screen}");
        assert_eq!(app.layout.account_rows.len(), 2);
        assert_eq!(
            screen.matches("当前").count(),
            1,
            "exactly one session is active:\n{screen}"
        );

        // And in the row list the number is dimmed there too.
        let area = app.layout.account_rows[1];
        let row = row_cells(&mut app, (100, 32), area.y);
        let paren = find_cells(&row, &["(", "1", ")"]).expect("the row shows the number");
        for (symbol, fg) in &row[paren..paren + 3] {
            assert_eq!(*fg, DIM, "row number {symbol} must be dimmed");
        }
    }

    #[test]
    fn the_account_manager_empty_state_points_at_login() {
        let mut app = App::new("/mc", "/acc", vec![], vec![]);
        app.open_accounts();
        let screen = render(&mut app, 100, 28).join("\n");
        assert!(screen.contains("还没有账户"), "{screen}");
        assert!(screen.contains("按 l 登录"), "{screen}");
    }

    #[test]
    fn the_pending_delete_is_announced_in_the_manager() {
        let tmp = tempfile::tempdir().unwrap();
        crate::accounts::save(
            tmp.path(),
            "Minecraft Launcher",
            "id",
            "Steve",
            "uuid",
            &serde_json::json!({}),
        )
        .unwrap();
        let saved = crate::accounts::list(tmp.path());
        let mut app = App::new("/mc", tmp.path(), vec![], saved);
        app.open_accounts();
        app.accounts_ui.confirm_delete = true;
        let screen = render(&mut app, 100, 28).join("\n");
        assert!(screen.contains("再按一次 d 确认移除"), "{screen}");
    }

    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

    /// The middle of an area, which is always inside it.
    fn middle(area: Area) -> (u16, u16) {
        (area.x + area.width / 2, area.y)
    }

    fn click(column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    /// The button inside the popup that replays `code`: the footer has its own
    /// copies of `l` / `d` / `Esc`, so the hit area has to be disambiguated.
    fn popup_button(app: &App, code: KeyCode, inside: Area) -> Area {
        app.layout
            .buttons
            .iter()
            .find(|button| button.code == code && inside.contains(button.area.x, button.area.y))
            .map(|button| button.area)
            .unwrap_or_else(|| {
                panic!(
                    "no clickable {code:?} inside the popup; buttons: {:?}",
                    app.layout.buttons
                )
            })
    }

    #[test]
    fn the_account_manager_hint_is_clickable() {
        let tmp = tempfile::tempdir().unwrap();
        for name in ["Alex", "Steve"] {
            crate::accounts::save(
                tmp.path(),
                "Minecraft Launcher",
                "id",
                name,
                "uuid",
                &serde_json::json!({}),
            )
            .unwrap();
        }
        let saved = crate::accounts::list(tmp.path());
        let mut app = App::new("/mc", tmp.path(), vec![], saved);
        app.open_accounts();
        render(&mut app, 100, 28);

        // `l 登录新账户` in the popup opens the login overlay. The whole phrase is
        // the button, so clicking the words — not just the `l` — has to work.
        let area = popup_button(&app, KeyCode::Char('l'), app.layout.accounts_popup);
        assert_eq!(
            area.width as usize,
            cell_width_of("l 登录新账户"),
            "the whole phrase is clickable"
        );
        app.handle_mouse(click(area.x + area.width - 1, area.y));
        assert_eq!(app.screen, Screen::Login);
        app.handle_key(KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(app.screen, Screen::Accounts);

        // `Enter 切换` activates the highlighted row.
        app.accounts_ui.selected = 0;
        render(&mut app, 100, 28);
        let area = popup_button(&app, KeyCode::Enter, app.layout.accounts_popup);
        let (x, y) = middle(area);
        app.handle_mouse(click(x, y));
        assert_eq!(app.account.label(), "Alex · Minecraft Launcher");

        // `d 移除` asks first, and the confirm hint's `d` deletes for real.
        render(&mut app, 100, 28);
        let area = popup_button(&app, KeyCode::Char('d'), app.layout.accounts_popup);
        let (x, y) = middle(area);
        app.handle_mouse(click(x, y));
        assert!(app.accounts_ui.confirm_delete, "first click only asks");

        render(&mut app, 100, 28);
        let area = popup_button(&app, KeyCode::Char('d'), app.layout.accounts_popup);
        let (x, y) = middle(area);
        app.handle_mouse(click(x, y));
        assert_eq!(app.accounts.len(), 1, "second click deletes");
        assert!(!app.accounts_ui.confirm_delete);

        // `Esc 关闭` leaves the manager.
        render(&mut app, 100, 28);
        let area = popup_button(&app, KeyCode::Esc, app.layout.accounts_popup);
        let (x, y) = middle(area);
        app.handle_mouse(click(x, y));
        assert_eq!(app.screen, Screen::Main);
    }

    #[test]
    fn the_login_hint_buttons_replay_their_key() {
        let mut app = App::new("/mc", "/acc", vec![], vec![]);
        app.open_login();
        render(&mut app, 100, 28);

        // "Enter 下一步" on the official identity starts the flow, and the words
        // after the key are part of the button.
        let area = popup_button(&app, KeyCode::Enter, app.layout.overlay);
        assert_eq!(
            area.width as usize,
            cell_width_of("Enter 下一步"),
            "the whole phrase is clickable"
        );
        app.handle_mouse(click(area.x + area.width - 1, area.y));
        assert_eq!(login(&app), Some(crate::login::Identity::Official));

        // "Esc 取消" while waiting cancels it.
        assert!(matches!(
            app.login.step,
            crate::app::LoginStep::Waiting { .. }
        ));
        render(&mut app, 100, 28);
        let area = popup_button(&app, KeyCode::Esc, app.layout.overlay);
        let (x, y) = middle(area);
        app.handle_mouse(click(x, y));
        assert_eq!(app.screen, Screen::Main);

        // A failure screen keeps its only way out clickable.
        app.open_login();
        app.handle_login_event(crate::login::LoginEvent::Failed("网络不可达".into()));
        render(&mut app, 100, 28);
        let area = popup_button(&app, KeyCode::Esc, app.layout.overlay);
        let (x, y) = middle(area);
        app.handle_mouse(click(x, y));
        assert_eq!(app.screen, Screen::Main);
    }

    #[test]
    fn the_saved_list_and_the_success_screen_have_clickable_hints() {
        let tmp = tempfile::tempdir().unwrap();
        let accounts = tmp.path().join("凭据").join("账户");
        std::fs::create_dir_all(&accounts).unwrap();
        let file = crate::app_ids::path_under(accounts.parent().unwrap());
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(
            &file,
            "[[api_keys]]\nname = \"HMCL\"\nkey = \"6a3728d6-27a3-4180-99bb-479895b8f88e\"\n",
        )
        .unwrap();

        let mut app = App::new("/mc", &accounts, vec![], vec![]);
        app.open_login();
        app.handle_login_key(KeyCode::End, KeyModifiers::NONE);
        app.handle_login_key(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(app.login.step, crate::app::LoginStep::ChooseAppIds);

        // "Enter 登录" picks the highlighted entry; the words count too.
        render(&mut app, 100, 28);
        let area = popup_button(&app, KeyCode::Enter, app.layout.overlay);
        assert_eq!(area.width as usize, cell_width_of("Enter 登录"));
        app.handle_mouse(click(area.x + area.width - 1, area.y));
        assert_eq!(
            login(&app),
            Some(crate::login::Identity::Saved {
                name: "HMCL".to_string(),
                app_id: "6a3728d6-27a3-4180-99bb-479895b8f88e".to_string(),
            })
        );

        // And "Enter 返回" on the success panel closes the overlay.
        let saved_file = crate::accounts::save(
            &accounts,
            "HMCL",
            "6a3728d6-27a3-4180-99bb-479895b8f88e",
            "Steve",
            "uuid",
            &serde_json::json!({}),
        )
        .unwrap();
        app.handle_login_event(crate::login::LoginEvent::Succeeded {
            player_name: "Steve".into(),
            application: "HMCL".into(),
            file: saved_file,
        });
        render(&mut app, 100, 28);
        let area = popup_button(&app, KeyCode::Enter, app.layout.overlay);
        assert_eq!(area.width as usize, cell_width_of("Enter 返回"));
        app.handle_mouse(click(area.x + area.width - 1, area.y));
        assert_eq!(app.screen, Screen::Main);
    }

    #[test]
    fn the_footer_buttons_sit_at_the_right_edge_and_do_not_move() {
        let mut app = App::new("/mc", "/acc", vec![], vec![]);
        app.status = "短".to_string();
        render(&mut app, 100, 28);
        let short: Vec<u16> = app
            .layout
            .buttons
            .iter()
            .map(|button| button.area.x)
            .collect();

        // A long status must not push the buttons around.
        app.status = "这一行状态文字明显长得多，长到几乎占满整个页脚".repeat(2);
        render(&mut app, 100, 28);
        let long: Vec<u16> = app
            .layout
            .buttons
            .iter()
            .map(|button| button.area.x)
            .collect();
        assert_eq!(short, long, "buttons are pinned to the right edge");

        // And the last one ends flush with the footer's right edge.
        let last = app.layout.buttons.last().expect("footer buttons");
        assert_eq!(
            last.area.x + last.area.width,
            100,
            "the row ends flush with the frame's right edge"
        );
    }

    #[test]
    fn a_very_long_status_is_truncated_instead_of_covering_the_buttons() {
        let mut app = App::new("/mc", "/acc", vec![], vec![]);
        app.status = "长".repeat(200);
        let screen = render(&mut app, 60, 28).join("\n");
        assert!(
            screen.contains("q 退出"),
            "buttons survive the squeeze:\n{screen}"
        );
        let first = app.layout.buttons.first().expect("footer buttons");
        assert!(first.area.x >= 1, "and stay inside the frame");
    }

    #[test]
    fn footer_hints_are_buttons_and_replay_their_key() {
        use crossterm::event::KeyCode;
        use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

        let click = |column: u16, row: u16| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: crossterm::event::KeyModifiers::NONE,
        };

        let mut app = App::new(
            "/mc",
            "/acc",
            vec![version("1.20.1", "release", 17, None)],
            vec![],
        );
        render(&mut app, 100, 28);

        let find = |app: &App, code: KeyCode| {
            app.layout
                .buttons
                .iter()
                .find(|button| button.code == code)
                .copied()
                .unwrap_or_else(|| panic!("no footer button for {code:?}"))
        };

        // d 下载
        let button = find(&app, KeyCode::Char('d'));
        app.handle_mouse(click(button.area.x, button.area.y));
        assert_eq!(app.screen, Screen::Download);

        // Back to the main screen and try the toggle.
        app.handle_key(KeyCode::Esc, crossterm::event::KeyModifiers::NONE);
        assert_eq!(app.screen, Screen::Main);
        render(&mut app, 100, 28);
        let button = find(&app, KeyCode::Char('h'));
        app.handle_mouse(click(button.area.x, button.area.y));
        assert!(app.show_base_versions, "h button must toggle bases");

        // q 退出
        let button = find(&app, KeyCode::Char('q'));
        app.handle_mouse(click(button.area.x, button.area.y));
        assert!(app.should_quit);
    }

    #[test]
    fn the_main_footer_offers_a_clickable_uninstall() {
        use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

        let mut app = App::new(
            "/mc",
            "/acc",
            vec![version("1.20.1", "release", 17, None)],
            vec![],
        );
        render(&mut app, 100, 28);

        let button = app
            .layout
            .buttons
            .iter()
            .find(|button| button.code == KeyCode::Char('x'))
            .copied()
            .expect("uninstall button");
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: button.area.x,
            row: button.area.y,
            modifiers: crossterm::event::KeyModifiers::NONE,
        });

        assert_eq!(app.uninstall_confirm(), Some("1.20.1"));
        assert!(app.status.contains("再按一次 x"), "{}", app.status);
    }

    #[test]
    fn the_row_waiting_for_the_second_x_is_emphasised() {
        let mut app = App::new(
            "/mc",
            "/acc",
            vec![
                version("1.20.1", "release", 17, None),
                version("1.8.9", "release", 8, None),
            ],
            vec![],
        );
        app.request_uninstall();

        let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
        terminal
            .draw(|frame| {
                draw(frame, &mut app);
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let emphasised = (0..buffer.area.height)
            .flat_map(|y| (0..buffer.area.width).map(move |x| (x, y)))
            .filter(|&(x, y)| {
                buffer[(x, y)]
                    .modifier
                    .contains(ratatui::style::Modifier::REVERSED)
            })
            .count();

        assert!(
            emphasised >= "1.20.1".len(),
            "the id awaiting confirmation must be painted, found {emphasised} cells"
        );
    }

    #[test]
    fn the_account_manager_footer_offers_a_clickable_login() {
        let mut app = App::new("/mc", "/acc", vec![], vec![]);
        app.open_accounts();
        render(&mut app, 100, 28);

        let button = app
            .layout
            .buttons
            .iter()
            .find(|button| button.code == crossterm::event::KeyCode::Char('l'))
            .copied()
            .expect("login button");
        app.handle_mouse(crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column: button.area.x,
            row: button.area.y,
            modifiers: crossterm::event::KeyModifiers::NONE,
        });
        assert_eq!(app.screen, Screen::Login);
    }

    #[test]
    fn the_header_account_button_records_the_row_it_is_drawn_on() {
        let mut app = App::new("/mc", "/acc", vec![], vec![]);
        render(&mut app, 100, 28);
        // The label sits inside the header block's inner row, and the recorded area
        // has to match it or a click lands one row off.
        assert_eq!(app.layout.account.y, 1, "{:?}", app.layout.account);
        assert!(app.layout.account.height == 1);
        assert!(app.layout.account.x + app.layout.account.width <= 99);
    }

    fn java_runtime(major: u32, version: &str) -> crate::java::Runtime {
        crate::java::Runtime {
            major,
            release: format!("jdk-{version}"),
            name: format!("OpenJDK{major}U-jre_x64_linux_hotspot.tar.gz"),
            size: 50 * 1024 * 1024,
            sha256: "a".repeat(64),
            link: "https://github.com/adoptium/x/releases/download/y/z.tar.gz".to_string(),
            ea: major >= 28,
        }
    }

    #[test]
    fn the_java_picker_lists_every_feature_and_the_source_switch() {
        let mut app = App::new("/mc", "/acc", vec![], vec![]);
        app.open_java();
        app.handle_java_event(crate::java::Event::Listed(vec![
            java_runtime(8, "8u512-b09"),
            java_runtime(17, "17.0.21+9"),
            java_runtime(21, "21.0.13+9"),
            java_runtime(25, "25.0.5+8"),
            java_runtime(28, "28+23-ea-beta"),
        ]));
        let screen = render(&mut app, 100, 28).join("\n");

        for text in [
            "Java 8",
            "Java 17",
            "Java 21",
            "Java 25",
            "Java 28 EA",
            "28+23-ea-beta",
            "8u512-b09",
            "21.0.13+9",
            "50.0 MiB",
            "未安装",
            "清华源",
            "官方源（GitHub）",
            "GitHub Proxy",
            "运行时",
        ] {
            assert!(screen.contains(text), "missing {text:?}:\n{screen}");
        }
        assert_eq!(
            app.layout.java_rows.len(),
            5,
            "one clickable row per feature"
        );
        assert_eq!(app.layout.java_sources.len(), 2);
        assert!(app.layout.java_proxy.width > 0);

        // The active source is highlighted, the other one is dim.
        let chips = app.layout.java_sources.clone();
        assert_eq!(
            colors_at(&mut app, (100, 28), chips[0].x, chips[0].y).1,
            ACCENT,
            "清华源 is the default"
        );
        assert_ne!(
            colors_at(&mut app, (100, 28), chips[1].x, chips[1].y).1,
            ACCENT
        );
    }

    #[test]
    fn the_java_picker_shows_progress_and_what_is_already_installed() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let dir = crate::java::install_dir(&root.join("运行时"), "17.0.21+9");
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::write(dir.join("bin/java"), b"#!/bin/sh\n").unwrap();

        let mut app = App::new(&root, "/acc", vec![], vec![]);
        app.set_runtimes(root.join("运行时"));
        app.open_java();
        app.handle_java_event(crate::java::Event::Listed(vec![
            java_runtime(17, "17.0.21+9"),
            java_runtime(21, "21.0.13+9"),
        ]));
        // Start the 21 download, then pretend a third of it arrived.
        app.java.selected = 1;
        app.handle_java_key(KeyCode::Enter, KeyModifiers::NONE);
        app.handle_java_event(crate::java::Event::Progress {
            done: 12 * 1024 * 1024,
            total: 50 * 1024 * 1024,
            speed: 3.5 * 1024.0 * 1024.0,
        });
        let screen = render(&mut app, 100, 28).join("\n");
        assert!(screen.contains("已安装"), "{screen}");
        assert!(screen.contains("12.0 MiB / 50.0 MiB"), "{screen}");
        assert!(screen.contains("3.5 MiB/s"), "{screen}");
    }

    #[test]
    fn an_older_installed_build_is_offered_as_an_update() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let dir = crate::java::install_dir(&root.join("运行时"), "8u502-b07");
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::write(dir.join("bin/java"), b"#!/bin/sh\n").unwrap();

        let mut app = App::new(&root, "/acc", vec![], vec![]);
        app.set_runtimes(root.join("运行时"));
        app.open_java();
        app.handle_java_event(crate::java::Event::Listed(vec![java_runtime(
            8,
            "8u512-b09",
        )]));
        let screen = render(&mut app, 120, 28).join("\n");

        assert!(screen.contains("可更新（8u502-b07）"), "{screen}");
        assert!(screen.contains("u 全部更新（1）"), "{screen}");
    }

    #[test]
    fn a_java_failure_is_shown_in_its_row() {
        let mut app = App::new("/mc", "/acc", vec![], vec![]);
        app.open_java();
        app.handle_java_event(crate::java::Event::Listed(vec![java_runtime(
            21,
            "21.0.13+9",
        )]));
        app.handle_java_key(KeyCode::Enter, KeyModifiers::NONE);
        app.handle_java_event(crate::java::Event::Failed("哈希不匹配".to_string()));
        let screen = render(&mut app, 100, 28).join("\n");
        assert!(screen.contains("哈希不匹配"), "{screen}");
        assert!(screen.contains("Esc 返回"), "{screen}");
    }

    #[test]
    fn the_picker_shows_versions_loaders_and_the_source() {
        let mut app = App::new("/mc", "/acc", vec![], vec![]);
        app.open_download();
        app.handle_install_event(crate::install::InstallEvent::Versions(vec![
            crate::install::RemoteVersion {
                id: "1.21.11".into(),
                kind: "release".into(),
                released: "2025-12-09T00:00:00+00:00".into(),
            },
            crate::install::RemoteVersion {
                id: "1.8.9".into(),
                kind: "release".into(),
                released: "2015-12-03T00:00:00+00:00".into(),
            },
        ]));
        let screen = render(&mut app, 100, 28).join("\n");

        assert!(screen.contains("下载"), "{screen}");
        assert!(screen.contains("BMCLAPI"), "source missing:\n{screen}");
        assert!(screen.contains("1.21.11"), "{screen}");
        assert!(screen.contains("安装内容"), "{screen}");
        assert!(screen.contains("Fabric"), "{screen}");
        assert!(screen.contains("仅安装原版"), "{screen}");
        assert!(app.layout.download_versions.height > 0);
    }

    /// A picker with a release, a folded pre-release cycle, and an older release.
    fn tree_picker() -> App {
        let versions = [
            ("1.21.8", "release"),
            ("1.21.9-pre3", "snapshot"),
            ("1.21.9-pre2", "snapshot"),
            ("1.21.9-pre1", "snapshot"),
            ("1.21.7", "release"),
        ];
        let mut app = App::new("/mc", "/acc", vec![], vec![]);
        app.open_download();
        app.handle_install_event(InstallEvent::Versions(
            versions
                .iter()
                .map(|(id, kind)| crate::install::RemoteVersion {
                    id: id.to_string(),
                    kind: kind.to_string(),
                    released: "2025-01-01T00:00:00+00:00".to_string(),
                })
                .collect(),
        ));
        app
    }

    #[test]
    fn the_all_view_draws_a_snapshot_cycle_as_one_folded_line() {
        let mut app = tree_picker();
        let screen = render(&mut app, 110, 28).join("\n");

        // Releases keep the old two-column look, one line each.
        assert!(screen.contains("▶ 1.21.8"), "{screen}");
        assert!(screen.contains("  1.21.7"), "{screen}");
        // The whole cycle is one line, marked as foldable and counted.
        assert!(screen.contains("▸ 1.21.9-pre3 ~ 1.21.9-pre1"), "{screen}");
        assert!(screen.contains("3 个"), "{screen}");
        // None of the members is drawn while the cycle is folded.
        assert!(
            !screen.contains("1.21.9-pre2"),
            "a folded cycle must not list its members:\n{screen}"
        );
        // Three rows drawn: 1.21.8, the folded cycle, 1.21.7.
        assert_eq!(app.download.rows.len(), 3);
    }

    #[test]
    fn an_opened_cycle_is_drawn_indented_under_its_header() {
        let mut app = tree_picker();
        // Draw once, then click the folded cycle: the second data row.
        let before = render(&mut app, 110, 28).join("\n");
        let area = app.layout.download_versions;
        app.handle_mouse(click(area.x + 3, area.y + 1));
        let opened = render(&mut app, 110, 28).join("\n");
        assert!(opened.contains("▾ 1.21.9-pre3 ~ 1.21.9-pre1"), "{opened}");

        // Members sit one level in: the selection marker, two spaces, then the id.
        assert!(opened.contains("▶   1.21.9-pre3"), "{opened}");
        assert!(opened.contains("    1.21.9-pre2"), "{opened}");
        assert!(opened.contains("    1.21.9-pre1"), "{opened}");
        // The rows that were not folded are untouched.
        assert!(opened.contains("  1.21.8"), "{opened}");
        assert!(opened.contains("  1.21.7"), "{opened}");
        assert!(before.contains("▶ 1.21.8"), "{before}");
    }

    #[test]
    fn a_folded_old_version_run_is_drawn_with_its_era() {
        let versions = [
            ("b1.7.3", "old_beta"),
            ("b1.7.2", "old_beta"),
            ("a1.2.6", "old_alpha"),
            ("a1.0.16", "old_alpha"),
        ];
        let mut app = App::new("/mc", "/acc", vec![], vec![]);
        app.open_download();
        app.handle_install_event(InstallEvent::Versions(
            versions
                .iter()
                .map(|(id, kind)| crate::install::RemoteVersion {
                    id: id.to_string(),
                    kind: kind.to_string(),
                    released: "2011-01-01T00:00:00+00:00".to_string(),
                })
                .collect(),
        ));
        let screen = render(&mut app, 110, 28).join("\n");

        assert!(screen.contains("▸ b1.7.3 ~ b1.7.2"), "{screen}");
        assert!(
            screen.contains("旧版 Beta"),
            "beta run keeps its era:\n{screen}"
        );
        assert!(screen.contains("▸ a1.2.6 ~ a1.0.16"), "{screen}");
        assert!(
            screen.contains("旧版 Alpha"),
            "alpha run keeps its era:\n{screen}"
        );
    }

    #[test]
    fn the_loader_pane_offers_the_source_switch_in_its_bottom_corner() {
        let mut app = tree_picker();
        let screen = render(&mut app, 110, 28).join("\n");

        assert!(screen.contains("下载源"), "{screen}");
        assert!(screen.contains("官方源"), "{screen}");
        assert!(screen.contains("BMCLAPI"), "{screen}");
        assert_eq!(app.layout.download_sources.len(), 2, "two clickable chips");

        // The active one is highlighted; the other is dim.
        let chips = app.layout.download_sources.clone();
        let (mojang, bmclapi) = (chips[0], chips[1]);
        assert_eq!(
            colors_at(&mut app, (110, 28), bmclapi.x, bmclapi.y).1,
            ACCENT,
            "BMCLAPI is the default, so its chip carries the accent background"
        );
        let (mojang_fg, mojang_bg) = colors_at(&mut app, (110, 28), mojang.x, mojang.y);
        assert_eq!(mojang_fg, DIM, "the inactive chip is dim");
        assert_ne!(mojang_bg, ACCENT, "and has no highlight");

        // The switch sits on the pane's last row and never over a loader row.
        let loaders = app.layout.download_loaders;
        assert!(
            loaders.y + loaders.height <= bmclapi.y,
            "the list must stop above the switch: {loaders:?} vs {bmclapi:?}"
        );
        // It is the pane's last content row, above its bottom border.
        let pane = app.layout.download_loaders_pane;
        assert_eq!(
            bmclapi.y + 1,
            pane.y + pane.height - 1,
            "the switch is on the pane's last row: {pane:?}"
        );
        assert_eq!(
            bmclapi.x + bmclapi.width,
            pane.x + pane.width - 1,
            "flushed against the pane's right border"
        );
    }

    #[test]
    fn the_source_switch_takes_effect_on_the_spot() {
        let mut app = tree_picker();
        render(&mut app, 110, 28);
        let title = app.source_label();
        assert!(title == "BMCLAPI", "{title}");

        app.handle_download_key(KeyCode::Char('s'), KeyModifiers::NONE);
        let screen = render(&mut app, 110, 28).join("\n");
        assert!(
            screen.contains("下载 官方源"),
            "title follows the switch:\n{screen}"
        );
    }

    #[test]
    fn the_version_title_says_that_something_is_folded_and_how_to_open_it() {
        let mut app = tree_picker();
        let screen = render(&mut app, 110, 28).join("\n");
        assert!(screen.contains("1 组折叠"), "{screen}");
        assert!(screen.contains("Space 展开"), "{screen}");

        // Opening it drops the hint, since nothing is folded any more.
        app.handle_download_key(KeyCode::Down, KeyModifiers::NONE);
        app.handle_download_key(KeyCode::Char(' '), KeyModifiers::NONE);
        assert_eq!(app.download.expanded.len(), 1);
        let screen = render(&mut app, 110, 28).join("\n");
        assert!(!screen.contains("组折叠"), "{screen}");

        // The flat views have nothing to fold, so no hint either.
        app.handle_download_key(KeyCode::Char('f'), KeyModifiers::NONE);
        assert_eq!(app.download.filter, crate::app::VersionFilter::Release);
        let screen = render(&mut app, 110, 28).join("\n");
        assert!(!screen.contains("组折叠"), "{screen}");
    }

    #[test]
    fn the_filter_bar_sits_above_both_panes() {
        let mut app = App::new("/mc", "/acc", vec![], vec![]);
        app.open_download();
        app.handle_install_event(InstallEvent::Versions(vec![
            crate::install::RemoteVersion {
                id: "1.20.1".into(),
                kind: "release".into(),
                released: "2023-06-12T00:00:00+00:00".into(),
            },
            crate::install::RemoteVersion {
                id: "23w31a".into(),
                kind: "snapshot".into(),
                released: "2023-08-02T00:00:00+00:00".into(),
            },
        ]));
        let screen = render(&mut app, 100, 28).join("\n");

        // The search box and every chip are drawn and recorded.
        assert!(screen.contains("搜索"), "{screen}");
        assert!(screen.contains("全部"), "{screen}");
        assert!(screen.contains("正式版"), "{screen}");
        assert!(screen.contains("快照"), "{screen}");
        assert!(screen.contains("旧版"), "{screen}");
        assert_eq!(app.layout.download_filters.len(), 4);
        assert!(app.layout.download_search.width > 0);

        // Both panes start below the filter row. Only the version table draws a
        // header, so its clickable rows begin one line lower than the loaders'.
        assert!(app.layout.download_versions.y > app.layout.download_search.y);
        assert!(app.layout.download_loaders.y > app.layout.download_search.y);
        assert_eq!(
            app.layout.download_versions.y,
            app.layout.download_loaders.y + 1,
            "the loader table has no header row to skip"
        );
    }

    #[test]
    fn clicking_a_loader_row_selects_that_row_and_not_the_one_above() {
        let mut app = App::new("/mc", "/acc", vec![], vec![]);
        app.open_download();
        app.handle_install_event(InstallEvent::Versions(vec![
            crate::install::RemoteVersion {
                id: "1.20.1".into(),
                kind: "release".into(),
                released: "2023-06-12T00:00:00+00:00".into(),
            },
        ]));
        render(&mut app, 100, 28);
        assert!(
            app.download.loaders.len() >= 4,
            "need a few rows to pick from: {:?}",
            app.download.loaders
        );
        let pane = app.layout.download_loaders;

        // Every row, middle of the line: row i must select loader i.
        for (index, loader) in app.download.loaders.clone().iter().enumerate() {
            // Start somewhere else, so a no-op click cannot look like success.
            app.download.loader_pick = (index + 1) % app.download.loaders.len();
            app.handle_mouse(click(pane.x + 3, pane.y + index as u16));
            assert_eq!(
                app.download.loader_pick, index,
                "clicking row {index} must select it, not the row above"
            );
            assert_eq!(app.download.picked_loader(), *loader);
        }

        // The rows end where the list ends: the hint below is not a row.
        let below = pane.y + app.download.loaders.len() as u16;
        app.download.loader_pick = 1;
        app.handle_mouse(click(pane.x + 3, below));
        assert_eq!(app.download.loader_pick, 1, "the hint row is not a loader");
    }

    #[test]
    fn a_search_with_no_match_says_so_instead_of_looking_empty() {
        let mut app = App::new("/mc", "/acc", vec![], vec![]);
        app.open_download();
        app.handle_install_event(InstallEvent::Versions(vec![
            crate::install::RemoteVersion {
                id: "1.20.1".into(),
                kind: "release".into(),
                released: "2023-06-12T00:00:00+00:00".into(),
            },
        ]));
        app.handle_download_key(KeyCode::Char('/'), KeyModifiers::NONE);
        for c in "zzz".chars() {
            app.handle_download_key(KeyCode::Char(c), KeyModifiers::NONE);
        }
        let screen = render(&mut app, 100, 28).join("\n");
        assert!(screen.contains("没有匹配的版本"), "{screen}");
        assert!(screen.contains("安装内容"), "the loader pane still draws");
        assert_eq!(
            app.layout.download_versions,
            Area::default(),
            "no rows to click"
        );
    }

    #[test]
    fn the_title_counts_the_filtered_list_against_the_total() {
        let mut app = App::new("/mc", "/acc", vec![], vec![]);
        app.open_download();
        app.handle_install_event(InstallEvent::Versions(vec![
            crate::install::RemoteVersion {
                id: "1.20.1".into(),
                kind: "release".into(),
                released: "2023-06-12T00:00:00+00:00".into(),
            },
            crate::install::RemoteVersion {
                id: "23w31a".into(),
                kind: "snapshot".into(),
                released: "2023-08-02T00:00:00+00:00".into(),
            },
        ]));
        app.handle_download_key(KeyCode::Char('f'), KeyModifiers::NONE);
        let screen = render(&mut app, 100, 28).join("\n");
        assert!(screen.contains("1 / 2"), "{screen}");
    }

    #[test]
    fn the_picker_records_data_rows_so_clicks_and_wheel_land_on_versions() {
        let mut app = App::new("/mc", "/acc", vec![], vec![]);
        app.open_download();
        app.handle_install_event(InstallEvent::Versions(
            (0..60)
                .map(|i| crate::install::RemoteVersion {
                    id: format!("1.{i}"),
                    kind: "release".into(),
                    released: "2024-01-01T00:00:00+00:00".into(),
                })
                .collect(),
        ));
        render(&mut app, 100, 28);

        let versions = app.layout.download_versions;
        let loaders = app.layout.download_loaders;
        assert!(versions.height > 0 && loaders.height > 0);
        assert_eq!(app.layout.download_offset, 0, "nothing scrolled yet");

        // Selecting the last version scrolls the pane and the offset follows.
        app.download.picked = 59;
        render(&mut app, 100, 28);
        assert!(app.layout.download_offset > 0, "pane scrolled to the pick");
        assert_eq!(
            app.layout.download_offset + versions.height as usize,
            60,
            "the picked row sits at the bottom of the pane"
        );
    }

    #[test]
    fn the_loading_state_is_not_an_empty_picker() {
        let mut app = App::new("/mc", "/acc", vec![], vec![]);
        app.open_download();
        let screen = render(&mut app, 100, 28).join("\n");
        assert!(screen.contains("正在获取版本列表"), "{screen}");
    }

    #[test]
    fn the_install_screen_shows_a_gauge_percentage_speed_and_counts() {
        let mut app = App::new("/mc", "/acc", vec![], vec![]);
        app.open_download();
        app.handle_install_event(crate::install::InstallEvent::Versions(vec![
            crate::install::RemoteVersion {
                id: "1.20.1".into(),
                kind: "release".into(),
                released: "2023-06-12T00:00:00+00:00".into(),
            },
        ]));
        app.handle_download_key(
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        );
        app.handle_install_event(crate::install::InstallEvent::Progress {
            done: 0,
            total: 40,
            remaining_bytes: 10 * 1024 * 1024,
            total_bytes: 10 * 1024 * 1024,
        });
        app.handle_install_event(crate::install::InstallEvent::Progress {
            done: 10,
            total: 40,
            remaining_bytes: 5 * 1024 * 1024,
            total_bytes: 0,
        });
        let screen = render(&mut app, 100, 28).join("\n");

        assert!(screen.contains("安装进度"), "{screen}");
        assert!(screen.contains("50%"), "percentage missing:\n{screen}");
        assert!(
            screen.contains("10 / 40 个文件"),
            "counts missing:\n{screen}"
        );
        assert!(screen.contains("剩余"), "{screen}");
        // The bar itself: the gauge fills half the row.
        assert!(screen.contains("█"), "no gauge bar:\n{screen}");
    }

    #[test]
    fn the_install_screen_shows_aot_training_progress() {
        let mut app = App::new("/mc", "/acc", vec![], vec![]);
        app.screen = Screen::Installing;
        app.download.stage = crate::app::DownloadStage::Training;
        app.download.installed = Some(("26.3".to_string(), None));
        app.download.aot_loaded = 1234;
        app.download.aot_total = 43246;
        let screen = render(&mut app, 100, 20).join("\n");
        assert!(screen.contains("AOT 训练"), "{screen}");
        assert!(screen.contains("1234 / 43246 个候选类"), "{screen}");

        app.download.aot_preparing = true;
        let screen = render(&mut app, 100, 20).join("\n");
        assert!(screen.contains("准备 Java 28 EA"), "{screen}");
        app.download.aot_preparing = false;

        app.download.aot_assembling = true;
        app.download.aot_loaded = 43246;
        let screen = render(&mut app, 100, 20).join("\n");
        assert!(screen.contains("组装 AOT 缓存"), "{screen}");
    }

    #[test]
    fn the_progress_label_turns_dark_where_the_bar_covers_it() {
        let mut app = App::new("/mc", "/acc", vec![], vec![]);
        app.open_download();
        app.handle_install_event(InstallEvent::Versions(vec![
            crate::install::RemoteVersion {
                id: "1.20.1".into(),
                kind: "release".into(),
                released: "2023-06-12T00:00:00+00:00".into(),
            },
        ]));
        app.handle_download_key(KeyCode::Enter, KeyModifiers::NONE);
        app.handle_install_event(InstallEvent::Progress {
            done: 50,
            total: 100,
            remaining_bytes: 512 * 1024,
            total_bytes: 1024 * 1024,
        });

        let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
        terminal
            .draw(|frame| {
                draw(frame, &mut app);
            })
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        let rows = buffer_lines(&buffer);
        let row = rows
            .iter()
            .position(|line| line.contains("%  ") && line.contains("/s"))
            .expect("the gauge label is drawn");
        let line = &rows[row];
        // `find` is a byte offset and the bar glyphs are three bytes each.
        let percent_at = line
            .find('%')
            .map(|at| line[..at].chars().count() as u16)
            .expect("a percentage");

        // The label is centred, so with half the bar filled its first characters
        // sit on the bar and its last ones on the trough.
        let start = line
            .find("50%")
            .map(|at| line[..at].chars().count() as u16)
            .expect("the label starts with the percentage");
        assert!(
            percent_at < buffer.area.width / 2,
            "half full means the bar reaches past the percentage: {line:?}"
        );

        let on_bar = buffer.cell((start, row as u16)).expect("cell");
        assert_eq!(on_bar.fg, Color::Black, "inside the bar the text is dark");
        assert_eq!(on_bar.bg, ACCENT, "and sits on the bar's colour");

        // "50%  0 B/s" is ten cells wide; its tail is past the bar.
        let on_trough = buffer.cell((start + 9, row as u16)).expect("cell");
        assert_eq!(on_trough.fg, Color::White, "past the bar the text is light");
        assert_eq!(on_trough.bg, Color::Rgb(0x26, 0x28, 0x2e), "on the trough");

        // At the extremes the whole label is on one side or the other.
        for (fraction, expected) in [(0.0, Color::White), (1.0, Color::Black)] {
            app.handle_install_event(InstallEvent::Progress {
                done: (100.0 * fraction) as usize,
                total: 100,
                remaining_bytes: if fraction > 0.5 { 0 } else { 1024 },
                total_bytes: 1024,
            });
            let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
            terminal
                .draw(|frame| {
                    draw(frame, &mut app);
                })
                .unwrap();
            let buffer = terminal.backend().buffer().clone();
            let rows = buffer_lines(&buffer);
            let row = rows
                .iter()
                .position(|line| line.contains("%  ") && line.contains("/s"))
                .expect("the gauge label is drawn");
            let start = rows[row]
                .find('%')
                .map(|at| rows[row][..at].chars().count() as u16 - 2)
                .expect("a percentage");
            let cell = buffer.cell((start + 8, row as u16)).expect("cell");
            assert_eq!(
                cell.fg, expected,
                "at {fraction} the label is drawn in {expected:?}"
            );
        }
    }

    #[test]
    fn the_install_screen_reports_completion_and_failure() {
        let mut app = App::new("/mc", "/acc", vec![], vec![]);
        app.open_download();
        app.handle_install_event(crate::install::InstallEvent::Versions(vec![
            crate::install::RemoteVersion {
                id: "1.20.1".into(),
                kind: "release".into(),
                released: "2023-06-12T00:00:00+00:00".into(),
            },
        ]));
        app.handle_download_key(
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        );

        app.handle_install_event(crate::install::InstallEvent::Failed("镜像超时".into()));
        let screen = render(&mut app, 100, 28).join("\n");
        assert!(screen.contains("安装失败"), "{screen}");
        assert!(screen.contains("镜像超时"), "{screen}");

        app.handle_install_event(crate::install::InstallEvent::Finished {
            version: "1.20.1".into(),
            loader: Some("Fabric 0.16.0".into()),
        });
        let screen = render(&mut app, 100, 28).join("\n");
        assert!(screen.contains("安装完成"), "{screen}");
        assert!(screen.contains("Fabric 0.16.0"), "{screen}");
    }

    #[test]
    fn empty_state_says_so_instead_of_showing_a_blank_panel() {
        let mut app = App::new("/mc", "/acc", vec![], vec![]);
        let screen = render(&mut app, 100, 28).join("\n");
        assert!(screen.contains("没有找到任何版本"), "{screen}");
        assert!(screen.contains("未选择版本"), "{screen}");
    }

    #[test]
    fn details_of_the_selected_row_are_shown() {
        let mut app = App::new(
            "/mc",
            "/acc",
            vec![
                version("1.20.1", "release", 17, None),
                version("1.8.9", "release", 8, None),
            ],
            vec![],
        );
        app.select_next();
        let screen = render(&mut app, 100, 28).join("\n");
        assert!(screen.contains("1.8.9"), "{screen}");
        // The 1.8.9 row is selected, so the details pane must describe it too.
        let details = screen
            .lines()
            .filter(|line| line.contains("Java") || line.contains("基础版本"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!details.is_empty(), "no detail rows:\n{screen}");
    }

    #[test]
    fn the_log_shows_the_newest_lines_when_it_overflows() {
        let mut app = App::new(
            "/mc",
            "/acc",
            vec![version("1.20.1", "release", 17, None)],
            vec![],
        );
        for i in 0..200 {
            app.log(format!("entry-{i:03}"));
        }
        let screen = render(&mut app, 100, 28).join("\n");
        assert!(
            screen.contains("entry-199"),
            "newest log line missing:\n{screen}"
        );
        assert!(
            !screen.contains("entry-000"),
            "oldest lines should be scrolled away"
        );
    }
}
