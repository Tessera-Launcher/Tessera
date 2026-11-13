//! The download picker: the mirror's version list, search/kind filter, loader
//! choice, and the install progress screen.
//!
//! The state structs and the `App` handlers that drive them live here; `App` in
//! the parent module is still the only state machine, and this module never
//! touches the UI or a thread — it queues [`Effect`]s like every screen.

use super::*;

/// Which pane of the download picker has the keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerFocus {
    Versions,
    Loaders,
    Search,
}

impl PickerFocus {
    /// Tab order: versions → loaders → the filter bar → back.
    fn next(self) -> Self {
        match self {
            PickerFocus::Versions => PickerFocus::Loaders,
            PickerFocus::Loaders => PickerFocus::Search,
            PickerFocus::Search => PickerFocus::Versions,
        }
    }
}

/// The kind-of-release filter above the version list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionFilter {
    All,
    Release,
    Snapshot,
    Old,
}

impl VersionFilter {
    pub const ALL: [VersionFilter; 4] = [
        VersionFilter::All,
        VersionFilter::Release,
        VersionFilter::Snapshot,
        VersionFilter::Old,
    ];

    pub fn label(self) -> &'static str {
        match self {
            VersionFilter::All => "全部",
            VersionFilter::Release => "正式版",
            VersionFilter::Snapshot => "快照",
            VersionFilter::Old => "旧版",
        }
    }

    /// Cycles the way `f` does; the mouse picks one directly.
    fn next(self) -> Self {
        match self {
            VersionFilter::All => VersionFilter::Release,
            VersionFilter::Release => VersionFilter::Snapshot,
            VersionFilter::Snapshot => VersionFilter::Old,
            VersionFilter::Old => VersionFilter::All,
        }
    }

    fn matches(self, kind: crate::version::VersionKind) -> bool {
        use crate::version::VersionKind;
        match self {
            VersionFilter::All => true,
            VersionFilter::Release => kind == VersionKind::Release,
            VersionFilter::Snapshot => kind == VersionKind::Snapshot,
            VersionFilter::Old => kind.is_old(),
        }
    }
}

/// Where the download screen is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadStage {
    /// Waiting for the mirror's version list.
    LoadingVersions,
    Picking,
    Installing,
    Done,
    Failed,
}

/// The download picker and the progress screen share this state.
#[derive(Debug)]
pub struct DownloadUi {
    pub stage: DownloadStage,
    /// Every version the mirror listed.
    pub all: Vec<RemoteVersion>,
    /// Indices into [`DownloadUi::all`] that survive the search and kind filter,
    /// in display order. Keeping indices instead of cloned versions means a
    /// keystroke in the search box re-filters without allocating a second copy
    /// of the whole list.
    visible: Vec<usize>,
    /// Folded snapshot families, by key, that are currently opened.
    pub expanded: Vec<String>,
    /// The filtered list laid out as drawn lines (see [`crate::version_tree`]).
    pub rows: Vec<crate::version_tree::Row>,
    /// Folded groups in [`DownloadUi::rows`], cached so the pane title does not
    /// walk every row on every frame.
    folded: usize,
    /// Search text, matched against the version id.
    pub query: String,
    pub filter: VersionFilter,
    pub picked: usize,
    /// Loaders compatible with the picked version, in [`LOADERS`] order.
    pub loaders: Vec<Loader>,
    pub loader_pick: usize,
    pub focus: PickerFocus,
    /// Files finished / total in the current batch.
    pub done: usize,
    pub total: usize,
    /// Full size of the current batch, from the batch's opening event.
    pub batch_total_bytes: u64,
    pub remaining_bytes: u64,
    /// Smoothed bytes per second.
    pub speed: f64,
    pub(crate) sample: Option<(Instant, u64)>,
    pub error: Option<String>,
    pub installed: Option<(String, Option<String>)>,
    fetching: bool,
}

impl Default for DownloadUi {
    fn default() -> Self {
        DownloadUi {
            stage: DownloadStage::LoadingVersions,
            all: Vec::new(),
            visible: Vec::new(),
            expanded: Vec::new(),
            rows: Vec::new(),
            folded: 0,
            query: String::new(),
            filter: VersionFilter::All,
            picked: 0,
            loaders: vec![Loader::None],
            loader_pick: 0,
            focus: PickerFocus::Versions,
            done: 0,
            total: 0,
            batch_total_bytes: 0,
            remaining_bytes: 0,
            speed: 0.0,
            sample: None,
            error: None,
            installed: None,
            fetching: false,
        }
    }
}

impl DownloadUi {
    /// `0.0..=1.0` for the progress bar.
    pub fn fraction(&self) -> f64 {
        if self.batch_total_bytes > 0 && self.remaining_bytes <= self.batch_total_bytes {
            let done_bytes = self.batch_total_bytes - self.remaining_bytes;
            return (done_bytes as f64 / self.batch_total_bytes as f64).clamp(0.0, 1.0);
        }
        if self.total > 0 {
            return (self.done as f64 / self.total as f64).clamp(0.0, 1.0);
        }
        0.0
    }

    pub fn percent(&self) -> u16 {
        (self.fraction() * 100.0).round() as u16
    }

    pub fn picked_version(&self) -> Option<&RemoteVersion> {
        self.visible.get(self.picked).map(|&index| &self.all[index])
    }

    /// How many of [`DownloadUi::all`] the current filter lets through.
    pub fn matched(&self) -> usize {
        self.visible.len()
    }

    /// The visible version at `index` (a position in the filtered list).
    pub fn version(&self, index: usize) -> &RemoteVersion {
        &self.all[self.visible[index]]
    }

    /// The filtered list, in display order.
    #[cfg(test)]
    pub fn versions(&self) -> impl Iterator<Item = &RemoteVersion> {
        self.visible.iter().map(|&index| &self.all[index])
    }

    /// The indices behind `DownloadUi::versions`; the tree layout and the
    /// title need both the list and what it points at.
    pub fn visible(&self) -> &[usize] {
        &self.visible
    }

    /// Folded groups currently drawn.
    pub fn folded_rows(&self) -> usize {
        self.folded
    }

    /// The drawn row the highlight is on.
    ///
    /// `picked` is a version index, and one version can be drawn twice (as the
    /// newest member of an opened group, which is also that group's header). The
    /// child row wins, because that is the line that stands for the version.
    pub fn cursor(&self) -> usize {
        self.rows
            .iter()
            .position(|row| matches!(row.kind, crate::version_tree::RowKind::Version(index) if index == self.picked))
            .or_else(|| self.rows.iter().position(|row| row.anchor() == self.picked))
            .unwrap_or(0)
    }

    /// Rows the keyboard walks: versions, and groups that are folded.
    pub fn stops(&self) -> Vec<usize> {
        (0..self.rows.len())
            .filter(|index| self.rows[*index].is_stop())
            .collect()
    }

    /// Re-lay the filtered list out. Folding only happens in the "全部" view with
    /// no search: a search must never hide the version it matched.
    pub fn rebuild_rows(&mut self) {
        let tree = self.filter == VersionFilter::All && self.query.trim().is_empty();
        self.rows = crate::version_tree::build(&self.all, &self.visible, &self.expanded, tree);
        self.folded = self
            .rows
            .iter()
            .filter(|row| {
                matches!(
                    row.kind,
                    crate::version_tree::RowKind::Group {
                        expanded: false,
                        ..
                    }
                )
            })
            .count();
    }

    /// The drawn row of the folded group the cursor is in, if any: the header
    /// itself when the header is selected, or the one above an opened member.
    fn group_row_at_cursor(&self) -> Option<usize> {
        let cursor = self.cursor();
        let rows = &self.rows;
        if matches!(
            rows.get(cursor)?.kind,
            crate::version_tree::RowKind::Group { .. }
        ) {
            return Some(cursor);
        }
        rows[..cursor]
            .iter()
            .rposition(|row| matches!(row.kind, crate::version_tree::RowKind::Group { .. }))
    }

    pub fn picked_loader(&self) -> Loader {
        self.loaders
            .get(self.loader_pick)
            .copied()
            .unwrap_or(Loader::None)
    }

    /// Rebuild the visible list from `all`, keeping the picked id when it survives.
    fn apply_filter(&mut self) {
        let keep = self.picked_version().map(|version| version.id.clone());
        let query = self.query.trim().to_lowercase();
        let filter = self.filter;
        // Reuse the allocation and only store indices: no version is cloned, so a
        // keystroke costs a scan, not a full copy of the mirror's list.
        self.visible.clear();
        for (index, version) in self.all.iter().enumerate() {
            if !filter.matches(version.kind) {
                continue;
            }
            if !query.is_empty() && !version.id.to_lowercase().contains(&query) {
                continue;
            }
            self.visible.push(index);
        }

        self.picked = keep
            .and_then(|id| {
                self.visible
                    .iter()
                    .position(|&index| self.all[index].id == id)
            })
            .unwrap_or(0)
            .min(self.visible.len().saturating_sub(1));
        // A new list folds back up: keeping families open across a filter change
        // would leave rows that are no longer on screen opened.
        self.expanded.clear();
        self.rebuild_rows();
        self.refresh_loaders();
    }

    /// Rebuild the loader list for the newly picked version, keeping the choice
    /// when that loader still exists.
    pub fn refresh_loaders(&mut self) {
        let Some(version) = self.picked_version() else {
            self.loaders = vec![Loader::None];
            self.loader_pick = 0;
            return;
        };
        let previous = self.picked_loader();
        self.loaders = LOADERS
            .into_iter()
            .filter(|loader| loader.compatible(&version.id))
            .collect();
        self.loader_pick = self
            .loaders
            .iter()
            .position(|loader| *loader == previous)
            .unwrap_or(0);
    }
}

impl App {
    // ---- download ----

    /// Label for the active download source.
    pub fn source_label(&self) -> &'static str {
        self.source.label()
    }

    /// Where the launcher's own Java runtimes live; launching looks there first.
    pub fn set_runtimes(&mut self, runtimes: PathBuf) {
        self.runtimes = runtimes.clone();
        self.launch_options.runtimes = Some(runtimes);
    }

    /// Switch where downloads come from. Deliberately not persisted: the next
    /// start falls back to the default, so a one-off switch does
    /// not quietly become the new normal.
    pub fn set_source(&mut self, source: crate::install::Source) {
        if self.source == source {
            return;
        }
        self.source = source;
        self.log(format!("下载源：{}", source.label()));
        self.status = format!("下载源：{}", source.label());
    }

    /// `s` in the picker: the other source.
    pub fn toggle_source(&mut self) {
        self.set_source(self.source.next());
    }

    /// Open the picker and ask the main loop to fetch the version list.
    /// Show the download page, keeping what it already holds.
    ///
    /// Switching to another page and back must land on the same search, filter,
    /// selection and — if one is running — the same install, so the page is only
    /// initialised when it has nothing to show yet.
    pub fn open_download(&mut self) {
        // An install owns the progress screen and its worker slot. Reopening
        // must not hide it behind a fresh picker.
        if self.install_running {
            self.screen = Screen::Download;
            self.status = "安装进行中，请等待完成".to_string();
            return;
        }
        self.screen = Screen::Download;
        // Never loaded, or the last fetch failed: the list is what the page is
        // for, so ask for it. A request still in flight is left alone.
        if self.download.all.is_empty() && !self.download.fetching {
            self.download.stage = DownloadStage::LoadingVersions;
            self.download.fetching = true;
            self.request(Effect::FetchVersions);
            self.status = "下载：正在获取版本列表".to_string();
            self.log("打开下载界面");
        }
    }

    pub fn close_download(&mut self) {
        self.screen = Screen::Main;
        // A finished run leaves its result page behind; closing it goes back to
        // the picker. A run still in progress keeps its stage, so coming back
        // shows where it is.
        if matches!(
            self.download.stage,
            DownloadStage::Done | DownloadStage::Failed
        ) {
            self.download.stage = DownloadStage::Picking;
            self.download.installed = None;
            self.download.error = None;
        }
    }

    pub fn handle_download_key(&mut self, code: Key, modifiers: Modifiers) -> bool {
        let _ = modifiers;
        match self.download.stage {
            DownloadStage::LoadingVersions => {
                if matches!(code, Key::Esc | Key::Char('q')) {
                    self.close_download();
                }
            }
            DownloadStage::Picking => {
                // The search box swallows printable keys, so it gets first say.
                if self.download.focus == PickerFocus::Search {
                    match code {
                        Key::Esc => {
                            if self.download.query.is_empty() {
                                self.download.focus = PickerFocus::Versions;
                            } else {
                                self.download.query.clear();
                                self.download.apply_filter();
                            }
                        }
                        Key::Backspace => {
                            if self.download.query.pop().is_some() {
                                self.download.apply_filter();
                            }
                        }
                        Key::Tab => self.download.focus = self.download.focus.next(),
                        // Arrows still walk the list while the caret is in the box.
                        Key::Up => self.picker_step(-1, true),
                        Key::Down => self.picker_step(1, true),
                        Key::Enter => self.start_install(),
                        Key::Char(c) if !c.is_control() => {
                            self.download.query.push(c);
                            self.download.apply_filter();
                        }
                        _ => {}
                    }
                    return true;
                }
                match code {
                    Key::Esc | Key::Char('q') => self.close_download(),
                    Key::Tab => self.download.focus = self.download.focus.next(),
                    Key::Char('/') => self.download.focus = PickerFocus::Search,
                    Key::Char('s') => self.toggle_source(),
                    Key::Char('f') => {
                        self.download.filter = self.download.filter.next();
                        self.download.apply_filter();
                        self.status = format!("下载：{} 版本", self.download.filter.label());
                    }
                    Key::Up | Key::Char('k') => self.picker_step(-1, true),
                    Key::Down | Key::Char('j') => self.picker_step(1, true),
                    // Tree controls: `←` closes, `→` opens, `Space` flips whichever
                    // group the cursor is in (its header, or one of its members).
                    Key::Left => self.toggle_group_at_cursor(Some(false)),
                    Key::Right => self.toggle_group_at_cursor(Some(true)),
                    Key::Char(' ') => self.toggle_group_at_cursor(None),
                    Key::Enter => self.start_install(),
                    _ => {}
                }
            }
            DownloadStage::Installing => {
                if matches!(code, Key::Esc | Key::Char('q')) {
                    self.screen = Screen::Main;
                    self.log("安装转为后台进行，完成后会重新扫描版本");
                }
            }
            DownloadStage::Done | DownloadStage::Failed => {
                if matches!(code, Key::Esc | Key::Enter | Key::Char('q')) {
                    self.reload();
                    self.screen = Screen::Main;
                }
            }
        }
        true
    }

    /// Move the picker selection by `delta` steps.
    ///
    /// The keyboard wraps (`↓` on the last row starts over); the picker stops at
    /// both ends; that is the only difference, so both share this walk.
    fn picker_step(&mut self, delta: i32, wrap: bool) {
        let step = |position: i32, count: usize| -> usize {
            let last = count as i32 - 1;
            let target = position + delta;
            (if wrap {
                target.rem_euclid(count as i32)
            } else {
                target.clamp(0, last)
            }) as usize
        };
        match self.download.focus {
            // Walk the drawn rows, not the filtered list: a folded group is one
            // stop, however many snapshots it hides, and its members are not
            // steps the keyboard may land on.
            PickerFocus::Versions | PickerFocus::Search => {
                let stops = self.download.stops();
                if stops.is_empty() {
                    return;
                }
                let cursor = self.download.cursor();
                let position = stops.iter().position(|row| *row == cursor).unwrap_or(0);
                let next = step(position as i32, stops.len());
                self.download.picked = self.download.rows[stops[next]].anchor();
                self.download.refresh_loaders();
            }
            PickerFocus::Loaders => {
                let count = self.download.loaders.len();
                if count == 0 {
                    return;
                }
                self.download.loader_pick = step(self.download.loader_pick as i32, count);
            }
        }
    }

    /// A text field changed the search box; re-filter in place.
    ///
    /// The keyboard path (`/` then typing) and a GUI input box both end up here,
    /// so the two front-ends cannot disagree about when the list is rebuilt.
    pub fn set_download_query(&mut self, query: &str) {
        if self.download.query == query {
            return;
        }
        self.download.query = query.to_string();
        self.download.apply_filter();
    }

    /// Pick a kind filter directly, for a front-end with a row of buttons.
    pub fn set_download_filter(&mut self, filter: VersionFilter) {
        if self.download.filter == filter {
            return;
        }
        self.download.filter = filter;
        self.download.apply_filter();
        self.status = format!("下载：{} 版本", filter.label());
    }

    /// Open or close the group drawn at `drawn`, for a front-end that has the
    /// row index rather than a keyboard cursor.
    pub fn toggle_row(&mut self, drawn: usize) {
        self.toggle_group(drawn, None);
    }

    /// Open or close the group drawn at `drawn`, or under the keyboard cursor when
    /// `drawn` is `None`.
    fn toggle_group(&mut self, drawn: usize, expand: Option<bool>) {
        let Some(crate::version_tree::RowKind::Group {
            key,
            first,
            expanded,
            ..
        }) = self.download.rows.get(drawn).map(|row| row.kind.clone())
        else {
            return;
        };
        let open = expand.unwrap_or(!expanded);
        self.download.expanded.retain(|open_key| open_key != &key);
        if open {
            self.download.expanded.push(key);
        }
        self.download.picked = first;
        self.download.rebuild_rows();
        self.download.refresh_loaders();
    }

    /// `←` folds the group the cursor is in, `→` unfolds it, `Space` flips it.
    fn toggle_group_at_cursor(&mut self, expand: Option<bool>) {
        let Some(drawn) = self.download.group_row_at_cursor() else {
            return;
        };
        self.toggle_group(drawn, expand);
    }

    pub fn start_install(&mut self) {
        if self.install_running {
            self.status = "已有安装正在进行".to_string();
            return;
        }
        let Some(version) = self.download.picked_version() else {
            return;
        };
        let version = version.id.clone();
        let loader = self.download.picked_loader();
        self.install_running = true;
        self.download.stage = DownloadStage::Installing;
        self.download.done = 0;
        self.download.total = 0;
        self.download.batch_total_bytes = 0;
        self.download.remaining_bytes = 0;
        self.download.speed = 0.0;
        self.download.sample = None;
        self.download.installed = None;
        self.download.error = None;
        self.screen = Screen::Download;
        self.status = format!("安装 {version} · {}", loader.label());
        self.log(format!("开始安装 {version}（{}）", loader.label()));
        self.request(Effect::Install { version, loader });
    }

    /// Apply an event from the download worker.
    pub fn handle_install_event(&mut self, event: InstallEvent) {
        match event {
            InstallEvent::Versions(versions) => {
                self.download.fetching = false;
                // A previous list failure is superseded by this answer.
                self.download.error = None;
                // A stale list request must not flip a running install back to
                // the picker or reset its progress.
                if self.install_running {
                    return;
                }
                self.log(format!("版本列表：{} 个", versions.len()));
                self.download.all = versions;
                self.download.stage = DownloadStage::Picking;
                self.download.apply_filter();
                self.status = "下载：选择版本与 Loader".to_string();
            }
            InstallEvent::Log(line) => {
                // Only the text: `start_install` already moved the stage, and the
                // worker also logs while fetching the *version list* ("Updating
                // BMCLAPI index"), which used to flip the picker into the
                // progress screen for one frame.
                self.log(line);
            }
            InstallEvent::Progress {
                done,
                total,
                remaining_bytes,
                total_bytes,
            } => {
                let now = Instant::now();
                if total_bytes > 0 {
                    self.download.batch_total_bytes = total_bytes;
                }
                // Bytes per second, smoothed so the number does not flicker.
                if let Some((previous_at, previous_remaining)) = self.download.sample {
                    let seconds = now.duration_since(previous_at).as_secs_f64();
                    if seconds > 0.01 {
                        let moved = previous_remaining.saturating_sub(remaining_bytes);
                        if moved > 0 {
                            let instant = moved as f64 / seconds;
                            self.download.speed = if self.download.speed > 0.0 {
                                self.download.speed * 0.6 + instant * 0.4
                            } else {
                                instant
                            };
                        }
                    }
                }
                self.download.sample = Some((now, remaining_bytes));
                self.download.done = done;
                self.download.total = total;
                self.download.remaining_bytes = remaining_bytes;
            }
            InstallEvent::Finished { version, loader } => {
                self.install_running = false;
                self.download.installed = Some((version.clone(), loader.clone()));
                self.download.stage = DownloadStage::Done;
                self.reload();
                self.log(format!(
                    "安装完成：{version}{}",
                    loader
                        .as_ref()
                        .map_or_else(String::new, |loader| format!(" + {loader}"))
                ));
                self.status = format!("安装完成：{version}");
            }
            InstallEvent::Failed(message) => {
                self.download.fetching = false;
                self.install_running = false;
                self.download.stage = DownloadStage::Failed;
                self.download.error = Some(message.clone());
                self.status = "下载失败".to_string();
                self.log(format!("下载失败：{message}"));
            }
        }
    }

    /// Called on every loop iteration: drop the speed reading when updates stop.
    ///
    /// Returns whether anything visible changed, so the main loop can skip a
    /// redraw for an idle launcher.
    pub fn tick(&mut self) -> bool {
        let mut changed = false;
        if let Some((at, _)) = self.download.sample {
            if at.elapsed().as_secs_f64() > 2.0 && self.download.speed > 0.0 {
                self.download.speed = 0.0;
                changed = true;
            }
        }
        changed
    }
}
