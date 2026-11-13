//! Sidebar pages and the switch between the five screens.

use super::*;
use crate::widgets::button;

/// The sidebar's pages, in display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    Home,
    Accounts,
    Download,
    Runtimes,
    Music,
}

impl Page {
    const ALL: [Page; 5] = [
        Page::Home,
        Page::Accounts,
        Page::Download,
        Page::Runtimes,
        Page::Music,
    ];

    fn label(self) -> &'static str {
        match self {
            Page::Home => "主页",
            Page::Accounts => "账户",
            Page::Download => "下载",
            Page::Runtimes => "运行时",
            Page::Music => "音乐",
        }
    }

    fn id(self) -> &'static str {
        match self {
            Page::Home => "page-home",
            Page::Accounts => "page-accounts",
            Page::Download => "page-download",
            Page::Runtimes => "page-runtimes",
            Page::Music => "page-music",
        }
    }

    /// Which page a screen belongs to; login keeps the page it was opened from.
    fn of(screen: Screen) -> Page {
        match screen {
            Screen::Main | Screen::Login => Page::Home,
            Screen::Accounts => Page::Accounts,
            Screen::Download => Page::Download,
            Screen::Java => Page::Runtimes,
            Screen::Music => Page::Music,
        }
    }
}

impl Launcher {
    /// Switch screens from the sidebar. Clicking the active page is a no-op so
    /// it never resets the screen's own state (a running install, a picker).
    fn navigate(&mut self, page: Page, cx: &mut Context<Self>) {
        // Leaving the login overlay by switching pages cancels that flow; every
        // other page keeps its state, which is the point of a sidebar.
        if self.state.screen == Screen::Login {
            self.state.close_login();
        }
        match page {
            Page::Home => self.state.show_main(),
            Page::Accounts => self.state.open_accounts(),
            Page::Download => self.state.open_download(),
            Page::Runtimes => self.state.open_java(),
            Page::Music => self.state.open_music(),
        }
        self.refresh(cx);
    }

    pub(crate) fn sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = theme::colors(cx);
        let active = Page::of(self.state.screen);
        let mut nav = div().v_flex().gap(px(space::XS));
        for page in Page::ALL {
            let picked = page == active;
            nav = nav.child(
                div()
                    .id(page.id())
                    .h_flex()
                    .items_center()
                    .gap(px(space::MD))
                    .px(px(space::MD))
                    .py(px(space::MD))
                    .rounded(px(radius::MD))
                    .bg(rgb(if picked {
                        colors.selected
                    } else {
                        colors.surface
                    }))
                    .min_h(px(layout::ROW_HEIGHT))
                    .cursor_pointer()
                    .hover(move |style| style.bg(rgb(colors.hover)))
                    .child(
                        div()
                            .w(px(layout::INDICATOR_WIDTH))
                            .h(px(layout::INDICATOR_HEIGHT))
                            .rounded(px(radius::PILL))
                            .bg(rgb(if picked {
                                colors.fill_primary
                            } else {
                                colors.surface
                            })),
                    )
                    .child(
                        div()
                            .text_size(px(font::BODY))
                            .text_color(rgb(if picked {
                                colors.text_primary
                            } else {
                                colors.text_secondary
                            }))
                            .child(page.label()),
                    )
                    .on_click(
                        cx.listener(move |this: &mut Launcher, _event, _window, cx| {
                            this.navigate(page, cx);
                        }),
                    ),
            );
        }

        div()
            .v_flex()
            .w(px(layout::SIDEBAR))
            .h_full()
            .p(px(space::MD))
            .gap(px(space::XL))
            .rounded(px(radius::MD))
            .bg(rgb(colors.surface))
            .border(px(stroke::THIN))
            .border_color(rgb(colors.border_default))
            .child(nav)
            .child(div().flex_1())
            .child(button(
                if gpui_kit::component::Theme::global(cx).is_dark() {
                    "切换到浅色"
                } else {
                    "切换到深色"
                },
                cx,
                |_, cx| theme::toggle(cx),
            ))
            .child(
                div()
                    .v_flex()
                    .gap(px(space::XXS))
                    .px(px(space::MD))
                    .py(px(space::SM))
                    .child(
                        div()
                            .text_size(px(font::CAPTION))
                            .text_color(rgb(colors.text_accent))
                            .child(self.state.account.label()),
                    ),
            )
    }
}
