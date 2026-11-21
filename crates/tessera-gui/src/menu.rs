//! Context menu items for the version list.

use super::*;

// ---- context menu items ----

/// Build one menu item that runs `action` against the launcher and repaints.
///
/// The label is drawn here rather than left to `PopupMenuItem::new`, which
/// styles the item with `text_sm()` (0.875 rem = 14 px). The embedded Unifont
/// only carries 16 px and 32 px bitmap strikes, so a 14 px run has no strike to
/// pick and swash falls back to the outline face: grey antialiased edges next
/// to the crisp 16 px text everywhere else. `PopupMenuItem::element` keeps the
/// row, hover, focus and click handling and only asks for the label, so the
/// text can stay at the body size the design tokens define.
fn item(
    label: &'static str,
    weak: WeakEntity<Launcher>,
    action: impl Fn(&mut Launcher) + 'static,
) -> PopupMenuItem {
    PopupMenuItem::element(move |_, _| div().text_size(px(font::BODY)).child(label)).on_click(
        move |_, _, app| {
            let _ = weak.update(app, |this, cx| {
                action(this);
                this.refresh(cx);
            });
        },
    )
}

/// Point the selection at `dir`, or say why it cannot be acted on.
///
/// A menu belongs to the row it was opened on, not to whatever was selected
/// before the right-click, and a refresh can reorder the list while the menu is
/// open. Both lookups therefore go through the directory, never the index.
fn select_row(this: &mut Launcher, dir: &str, action: &str) -> bool {
    match this
        .state
        .all_versions
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

/// Show the instance folder in the desktop's file manager.
///
/// Unlike launch and uninstall there is nothing to select and nothing to
/// confirm: the menu already opened on one row, and the answer is a window the
/// launcher does not own.
pub(crate) fn open_folder_menu_item(weak: WeakEntity<Launcher>, dir: String) -> PopupMenuItem {
    item("打开文件夹", weak, move |this| {
        if !select_row(this, &dir, "打开文件夹") {
            return;
        }
        this.state.request_open_folder();
    })
}

pub(crate) fn uninstall_menu_item(weak: WeakEntity<Launcher>, dir: String) -> PopupMenuItem {
    item("卸载", weak, move |this| {
        if !select_row(this, &dir, "卸载") {
            return;
        }
        this.state.request_uninstall();
    })
}
