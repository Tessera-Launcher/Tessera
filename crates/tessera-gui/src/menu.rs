//! Context menu items for the version list.

use super::*;

// ---- context menu items ----

/// Build one menu item that runs `action` against the launcher and repaints.
fn item(
    label: &str,
    weak: WeakEntity<Launcher>,
    action: impl Fn(&mut Launcher) + 'static,
) -> PopupMenuItem {
    PopupMenuItem::new(label).on_click(move |_, _, app| {
        let _ = weak.update(app, |this, cx| {
            action(this);
            this.refresh(cx);
        });
    })
}

/// Point the selection at `dir`, or say why it cannot be acted on.
///
/// A menu belongs to the row it was opened on, not to whatever was selected
/// before the right-click, and a refresh can reorder the list while the menu is
/// open. Both lookups therefore go through the directory, never the index.
fn select_row(this: &mut Launcher, dir: &str, action: &str) -> bool {
    match this
        .state
        .versions
        .iter()
        .position(|version| version.dir == dir)
    {
        Some(index) => {
            this.state.selected = index;
            true
        }
        None => {
            this.state
                .log(format!("版本 {dir} 已不在列表中，无法{action}"));
            false
        }
    }
}

pub(crate) fn download_menu_item(weak: WeakEntity<Launcher>) -> PopupMenuItem {
    item("下载游戏…", weak, |this| this.state.open_download())
}

/// Select the row the menu was opened on, then launch it, like a double click.
pub(crate) fn launch_menu_item(weak: WeakEntity<Launcher>, dir: String) -> PopupMenuItem {
    item("启动", weak, move |this| {
        if !select_row(this, &dir, "启动") {
            return;
        }
        this.state.request_launch();
    })
}

pub(crate) fn refresh_menu_item(weak: WeakEntity<Launcher>) -> PopupMenuItem {
    item("刷新", weak, |this| this.state.reload())
}

pub(crate) fn uninstall_menu_item(weak: WeakEntity<Launcher>, dir: String) -> PopupMenuItem {
    item("卸载", weak, move |this| {
        if !select_row(this, &dir, "卸载") {
            return;
        }
        this.state.request_uninstall();
    })
}
