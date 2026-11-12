//! Sidebar pages and the switch between the five screens.

use super::*;

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
        let active = Page::of(self.state.screen);
        let mut nav = div().v_flex().gap_1();
        for page in Page::ALL {
            let picked = page == active;
            nav = nav.child(
                div()
                    .id(page.id())
                    .h_flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_2()
                    .rounded_md()
                    .bg(rgb(if picked { ROW } else { PANEL }))
                    .child(
                        div()
                            .w(px(3.0))
                            .h(px(14.0))
                            .rounded_full()
                            .bg(rgb(if picked { ACCENT } else { PANEL })),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(if picked { TEXT } else { DIM }))
                            .child(page.label()),
                    )
                    .on_click(cx.listener(move |this: &mut Launcher, _event, _window, cx| {
                        this.navigate(page, cx);
                    })),
            );
        }

        div()
            .v_flex()
            .w(px(168.0))
            .h_full()
            .p_2()
            .gap_3()
            .rounded_md()
            .bg(rgb(PANEL))
            .border_1()
            .border_color(rgb(BORDER))
            .child(
                div()
                    .v_flex()
                    .gap_0p5()
                    .px_2()
                    .py_1()
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgb(ACCENT))
                            .child("Tessera"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(DIM))
                            .child("Minecraft Java 版启动器"),
                    ),
            )
            .child(nav)
            .child(div().flex_1())
            .child(
                div()
                    .v_flex()
                    .gap_0p5()
                    .px_2()
                    .py_1()
                    .child(div().text_xs().text_color(rgb(DIM)).child("当前账户"))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(ACCENT))
                            .child(self.state.account.label()),
                    ),
            )
    }
}
