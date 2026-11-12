//! Context menu items for the version list.

use super::*;

// ---- context menu items ----

pub(crate) fn download_menu_item(weak: WeakEntity<Launcher>) -> PopupMenuItem {
    PopupMenuItem::new("下载游戏…").on_click(move |_, _, app| {
        let _ = weak.update(app, |this, cx| {
            this.state.open_download();
            this.refresh(cx);
        });
    })
}

/// Select the row the menu was opened on, then launch it, like a double click.
pub(crate) fn launch_menu_item(weak: WeakEntity<Launcher>, dir: String) -> PopupMenuItem {
    PopupMenuItem::new("启动").on_click(move |_, _, app| {
        let _ = weak.update(app, |this, cx| {
            let Some(index) = this
                .state
                .versions
                .iter()
                .position(|version| version.dir == dir)
            else {
                this.state
                    .log(format!("版本 {dir} 已不在列表中，无法启动"));
                this.refresh(cx);
                return;
            };
            this.state.selected = index;
            this.state.request_launch();
            this.refresh(cx);
        });
    })
}

pub(crate) fn refresh_menu_item(weak: WeakEntity<Launcher>) -> PopupMenuItem {
    PopupMenuItem::new("刷新").on_click(move |_, _, app| {
        let _ = weak.update(app, |this, cx| {
            this.state.reload();
            this.refresh(cx);
        });
    })
}

pub(crate) fn uninstall_menu_item(weak: WeakEntity<Launcher>, dir: String) -> PopupMenuItem {
    PopupMenuItem::new("卸载").on_click(move |_, _, app| {
        let _ = weak.update(app, |this, cx| {
            // The menu belongs to the row it was opened on, not to whatever row
            // happened to be selected before the right-click. Look it up by
            // directory because a refresh can reorder the list while the menu is
            // open.
            let Some(index) = this
                .state
                .versions
                .iter()
                .position(|version| version.dir == dir)
            else {
                this.state
                    .log(format!("版本 {dir} 已不在列表中，无法卸载"));
                this.refresh(cx);
                return;
            };
            this.state.selected = index;
            this.state.request_uninstall();
            this.refresh(cx);
        });
    })
}
