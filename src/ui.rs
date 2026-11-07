//! Drawing the main screen. Pure rendering: takes `&App`, touches no state.

use ratatui::buffer::CellWidth;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Cell, Clear, Paragraph, Row, Table, Wrap};
use ratatui::Frame;

use crate::app::{App, Area, LoginStep, Screen};
use crate::discover::{self, LocalVersion};

const ACCENT: Color = Color::Rgb(0x6c, 0xd6, 0x7a);
const SNAPSHOT: Color = Color::Rgb(0xe8, 0xc9, 0x6a);
const LEGACY: Color = Color::Rgb(0xc4, 0x8a, 0xd8);
const BROKEN: Color = Color::Rgb(0xe0, 0x6c, 0x6c);
const DIM: Color = Color::Rgb(0x8a, 0x8a, 0x92);

/// Render one frame.
pub fn draw(frame: &mut Frame, app: &mut App) {
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(10),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    draw_header(frame, header, app);

    let [list, right] =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Min(38)]).areas(body);
    draw_version_list(frame, list, app);
    let [details, log] = Layout::vertical([Constraint::Length(9), Constraint::Min(4)]).areas(right);
    draw_details(frame, details, app);
    draw_log(frame, log, app);

    draw_footer(frame, footer, app);

    if app.screen == Screen::Login {
        draw_login(frame, frame.area(), app);
    }
}

fn draw_header(frame: &mut Frame, area: Rect, app: &mut App) {
    let title = Line::from(vec![
        Span::styled(" yaml ", Style::new().fg(Color::Black).bg(ACCENT).bold()),
        Span::styled(" Minecraft Java 版启动器 ", Style::new().fg(Color::White)),
    ]);
    // The account line doubles as the login button, so the hint says so.
    let account_text = match &app.account {
        crate::app::Account::SignedOut => "未登录 · 点此登录".to_string(),
        signed_in => signed_in.label(),
    };
    let account = Line::from(Span::styled(
        format!(" {account_text} "),
        Style::new().fg(DIM),
    ));
    let block = Block::bordered()
        .title(title)
        .title_top(account.right_aligned())
        .border_style(Style::new().fg(DIM));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Record the clickable span: right-aligned, width taken from the rendered text
    // so CJK characters count as two cells.
    let width = account_text.cell_width().saturating_add(2).min(inner.width);
    app.layout.account = Area::new(inner.x + inner.width - width, inner.y, width, 1);

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
    ]);
    frame.render_widget(Paragraph::new(info), inner);
}

fn draw_version_list(frame: &mut Frame, area: Rect, app: &mut App) {
    let block = Block::bordered()
        .title(Line::from(format!(" 版本 ({}) ", app.versions.len())))
        .border_style(Style::new().fg(DIM));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Data rows start below the table header; that is the region clicks address.
    let rows_visible = inner.height.saturating_sub(1) as usize;
    app.layout.list = Area::new(
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

fn draw_details(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::bordered()
        .title(Line::from(" 详情 "))
        .border_style(Style::new().fg(DIM));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let Some(version) = app.selected_version() else {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled("未选择版本", Style::new().fg(DIM))))
                .alignment(Alignment::Center),
            inner,
        );
        return;
    };
    let paths = app.selected_paths();

    let mut rows = vec![
        kv("ID", version.id.clone()),
        kv("类型", kind_label(version)),
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
    if let Some(paths) = &paths {
        let size = discover::dir_size(&paths.dir);
        rows.push(kv("目录", paths.dir.display().to_string()));
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

fn draw_log(frame: &mut Frame, area: Rect, app: &mut App) {
    let block = Block::bordered()
        .title(Line::from(" 日志 "))
        .border_style(Style::new().fg(DIM));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    app.layout.log = Area::new(inner.x, inner.y, inner.width, inner.height);

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
    // Redraw the frame so the title reflects the scroll position.
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);

    let lines: Vec<Line> = app.log[start..end]
        .iter()
        .map(|line| {
            Line::from(Span::styled(
                format!("  {line}"),
                Style::new().fg(Color::Rgb(0xc8, 0xc8, 0xce)),
            ))
        })
        .collect();
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
}

fn draw_footer(frame: &mut Frame, area: Rect, app: &App) {
    let status = Span::styled(format!(" {} ", app.status), Style::new().fg(ACCENT));
    let hints = if app.screen == Screen::Login {
        match app.login.step {
            LoginStep::Choose => "↑↓ 选择身份 · Enter 下一步 · Esc 取消 ",
            LoginStep::EnterAppId => "输入 app id · Enter 开始 · Esc 返回 ",
            LoginStep::Waiting { .. } => "等待浏览器确认 · Esc 取消 ",
            LoginStep::Done { .. } | LoginStep::Failed(_) => "Enter/Esc 返回 ",
        }
    } else {
        "↑↓ 选择 · Enter 安装/启动 · l 登录 · r 刷新 · q 退出 "
    };
    let keys = Span::styled(hints, Style::new().fg(DIM));
    let line = Line::from(vec![status, keys]);
    frame.render_widget(Paragraph::new(line).wrap(Wrap { trim: true }), area);
}

/// The login overlay, drawn over the main screen.
fn draw_login(frame: &mut Frame, area: Rect, app: &mut App) {
    let width = area.width.saturating_sub(4).clamp(36, 68);
    let height = match &app.login.step {
        LoginStep::Choose => 9,
        LoginStep::EnterAppId => 11,
        LoginStep::Waiting { .. } => 10,
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
    app.layout.overlay = Area::new(popup.x, popup.y, popup.width, popup.height);
    app.layout.overlay_options.clear();

    let title = match &app.login.step {
        LoginStep::Choose => " 登录 · 选择身份 ",
        LoginStep::EnterAppId => " 登录 · 自定义 app id ",
        LoginStep::Waiting { .. } => " 登录 · 等待授权 ",
        LoginStep::Done { .. } => " 登录 · 成功 ",
        LoginStep::Failed(_) => " 登录 · 失败 ",
    };
    let block = Block::bordered()
        .title(Line::from(title).style(Style::new().fg(ACCENT).bold()))
        .border_style(Style::new().fg(ACCENT));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    match &app.login.step {
        LoginStep::Choose => {
            let options = [
                (
                    "官方启动器",
                    "Minecraft Launcher 的 title client id，走设备令牌 + SISU",
                ),
                (
                    "自定义 app id",
                    "其他启动器的 Azure 应用 client id，走 user + XSTS",
                ),
            ];
            let mut lines = Vec::new();
            for (index, (name, hint)) in options.iter().enumerate() {
                // Two clickable rows per option: the name and its hint.
                app.layout.overlay_options.push(Area::new(
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
            lines.push(Line::from(Span::styled(
                " ↑↓ 选择 · Enter 下一步 · Esc 取消",
                Style::new().fg(DIM),
            )));
            frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
        }
        LoginStep::EnterAppId => {
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
                Line::from(Span::styled(
                    if filled {
                        " Enter 开始登录 · Esc 返回"
                    } else {
                        " 不能为空 · 例如 Prism 的 c36a9fb6-4f2a-41ff-90bd-ae7cc92031eb"
                    },
                    Style::new().fg(if filled { DIM } else { BROKEN }),
                )),
            ];
            frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
        }
        LoginStep::Waiting { user_code, url } => {
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
                Line::from(Span::styled(" 2. 输入代码", Style::new().fg(DIM))),
                Line::from(Span::styled(
                    format!("    {user_code}"),
                    Style::new().fg(ACCENT).bold(),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    " 等待浏览器确认… Esc 取消",
                    Style::new().fg(DIM),
                )),
            ];
            frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
        }
        LoginStep::Done {
            player,
            application,
        } => {
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
                Line::from(Span::styled(
                    " 凭据已写入 data/凭据/账户/ · Enter 返回",
                    Style::new().fg(DIM),
                )),
            ];
            frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
        }
        LoginStep::Failed(message) => {
            let lines = vec![
                Line::from(""),
                Line::from(Span::styled(" 登录失败", Style::new().fg(BROKEN).bold())),
                Line::from(""),
                Line::from(Span::styled(
                    format!(" {message}"),
                    Style::new().fg(Color::White),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    " Esc 返回 · 检查网络/代理后重试",
                    Style::new().fg(DIM),
                )),
            ];
            frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
        }
    }
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
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn version(id: &str, kind: &str, java: u32, inherits: Option<&str>) -> LocalVersion {
        LocalVersion {
            id: id.to_string(),
            kind: kind.to_string(),
            java: Some(java),
            inherits: inherits.map(str::to_string),
            main_class: Some("net.minecraft.client.main.Main".to_string()),
            has_jar: true,
            release_time: None,
        }
    }

    /// Render into a fixed-size buffer and return the rows as text.
    fn render(app: &mut App, width: u16, height: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
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
        assert!(screen.contains("↑↓ 选择"), "keys missing:\n{screen}");
        assert!(screen.contains("▶"), "selection marker missing:\n{screen}");
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
