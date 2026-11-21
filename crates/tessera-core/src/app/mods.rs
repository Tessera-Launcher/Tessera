//! The Modrinth page: Fabric client mods for the selected instance.
//!
//! Phase 1 is a browser and installer, not a mod-profile manager. It searches
//! the public Modrinth index for the instance's exact Minecraft version, shows
//! the versions Modrinth marks as Fabric/client-compatible, and installs the
//! chosen jar plus its required Fabric dependencies into that instance's
//! `mods/`. Optional dependencies are reported but not installed; modpacks and
//! every non-Fabric loader are out of scope.

use super::*;
use crate::modrinth;

/// The Modrinth page's state.
#[derive(Debug, Clone, Default)]
pub struct ModsUi {
    /// Instance directory under `versions/`, once a Fabric instance is selected.
    pub dir: Option<String>,
    /// The instance's vanilla version, used for every API query.
    pub game_version: String,
    pub query: String,
    pub hits: Vec<modrinth::SearchHit>,
    pub selected: usize,
    pub versions: Vec<modrinth::Version>,
    pub versions_for: Option<String>,
    pub version_pick: usize,
    pub searching: bool,
    pub loading_versions: bool,
    pub installing: bool,
    /// Jars the launcher owns in this instance, from the sidecar.
    pub installed: Vec<modrinth::TrackedMod>,
    /// A page-level explanation ("正在搜索…", "安装完成…").
    pub message: Option<String>,
    pub error: Option<String>,
}

impl ModsUi {
    pub fn selected_hit(&self) -> Option<&modrinth::SearchHit> {
        self.hits.get(self.selected)
    }

    pub fn selected_version(&self) -> Option<&modrinth::Version> {
        self.versions.get(self.version_pick)
    }

    pub fn has_fabric_target(&self) -> bool {
        self.dir.is_some()
    }
}

impl App {
    // ---- Modrinth page ----

    /// Open the Modrinth page for the instance selected on the home screen.
    ///
    /// The page follows the home selection: switching to another instance and
    /// coming back replaces the search context instead of installing into the
    /// previous instance behind the player's back.
    pub fn open_mods(&mut self) {
        if self.screen == Screen::Login {
            self.close_login();
        }
        self.screen = Screen::Mods;

        let target = self.mods_target();
        match target {
            Err(reason) => {
                // Keep the query: the text field keeps showing it, and the next
                // Fabric instance should not look as if the search was lost.
                let query = self.mods.query.clone();
                self.mods = ModsUi {
                    query,
                    error: Some(reason.clone()),
                    ..ModsUi::default()
                };
                self.status = reason.clone();
                self.log(format!("Modrinth：{reason}"));
            }
            Ok((dir, game_version)) => {
                let changed = self.mods.dir.as_deref() != Some(dir.as_str())
                    || self.mods.game_version != game_version;
                if changed {
                    // Keep the search text: the same query is usually what the
                    // player wants when comparing another Fabric instance.
                    let query = self.mods.query.clone();
                    self.mods = ModsUi {
                        dir: Some(dir.clone()),
                        game_version: game_version.clone(),
                        query,
                        ..ModsUi::default()
                    };
                    self.log(format!(
                        "Modrinth：{dir} · Minecraft {game_version} · 仅 Fabric 客户端模组"
                    ));
                }
                self.mods.installed = modrinth::installed(&self.root.join("versions").join(&dir));
                self.mods.error = None;
                if self.mods.hits.is_empty() && !self.mods.searching {
                    self.search_mods();
                } else {
                    self.status = format!("Modrinth：{} 个结果", self.mods.hits.len());
                }
            }
        }
    }

    /// The selected instance's directory and vanilla version, when it is a
    /// Fabric install this page may modify.
    fn mods_target(&self) -> std::result::Result<(String, String), String> {
        let Some(version) = self.selected_version() else {
            return Err("先在主页选择一个版本".to_string());
        };
        if !version.has_profile {
            return Err(format!("{} 的版本文件不完整，无法管理模组", version.dir));
        }
        let profile = crate::profile::read(&self.root, &version.dir);
        let marker = profile
            .as_ref()
            .and_then(|profile| profile.loader_id.as_deref());
        let fabric = marker.is_some_and(|id| id.eq_ignore_ascii_case("fabric"))
            || version
                .main_class
                .as_deref()
                .is_some_and(|class| class.to_ascii_lowercase().contains("fabricmc"));
        if !fabric {
            return Err(format!(
                "{} 不是 Fabric 实例；Modrinth 页面目前只支持 Fabric",
                version.dir
            ));
        }
        Ok((version.dir.clone(), version.base_version().to_string()))
    }

    /// Update the search box without touching the network; the search button
    /// calls [`App::search_mods`].
    pub fn set_mods_query(&mut self, query: &str) {
        self.mods.query = query.to_string();
    }

    /// Ask Modrinth for Fabric mods for this instance's game version.
    pub fn search_mods(&mut self) {
        if self.mods.dir.is_none() || self.mods.searching {
            return;
        }
        self.mods.searching = true;
        self.mods.error = None;
        self.mods.versions.clear();
        self.mods.versions_for = None;
        self.mods.version_pick = 0;
        let query = self.mods.query.clone();
        self.status = if query.trim().is_empty() {
            "Modrinth：正在获取热门 Fabric 模组".to_string()
        } else {
            format!("Modrinth：正在搜索「{query}」")
        };
        self.request(Effect::SearchMods {
            game_version: self.mods.game_version.clone(),
            query,
        });
    }

    /// Select a search result and immediately ask for its compatible versions.
    pub fn select_mods_hit(&mut self, index: usize) {
        let Some(hit) = self.mods.hits.get(index).cloned() else {
            return;
        };
        self.mods.selected = index;
        self.load_mod_versions(hit.project_id);
    }

    fn load_mod_versions(&mut self, project_id: String) {
        if self.mods.dir.is_none() {
            return;
        }
        if self.mods.loading_versions && self.mods.versions_for.as_deref() == Some(&project_id) {
            return;
        }
        self.mods.versions.clear();
        self.mods.version_pick = 0;
        self.mods.versions_for = Some(project_id.clone());
        self.mods.loading_versions = true;
        self.mods.error = None;
        self.status = "Modrinth：正在读取版本".to_string();
        self.request(Effect::FetchModVersions {
            project_id,
            game_version: self.mods.game_version.clone(),
        });
    }

    /// Choose one version from the selected project's list.
    pub fn select_mod_version(&mut self, index: usize) {
        if index < self.mods.versions.len() {
            self.mods.version_pick = index;
        }
    }

    /// Install the version under the cursor, resolving required dependencies.
    pub fn install_selected_mod(&mut self) {
        if self.mods.installing {
            self.status = "Modrinth：已有安装正在进行".to_string();
            return;
        }
        let Some(dir) = self.mods.dir.clone() else {
            return;
        };
        let Some(version) = self.mods.selected_version().cloned() else {
            self.status = "Modrinth：先选择一个可安装版本".to_string();
            return;
        };
        if version.primary_file().is_none() {
            self.status = "Modrinth：这个版本没有可安装的 jar".to_string();
            return;
        }
        let title = self
            .mods
            .selected_hit()
            .filter(|hit| hit.project_id == version.project_id)
            .map(|hit| hit.title.clone())
            .unwrap_or_else(|| version.name.clone());
        self.mods.installing = true;
        self.mods.error = None;
        self.mods.message = Some(format!("正在安装 {title} {}…", version.version_number));
        self.status = format!("Modrinth：安装 {title} {}", version.version_number);
        self.log(format!(
            "安装模组 {title} {}（Minecraft {}）",
            version.version_number, self.mods.game_version
        ));
        self.request(Effect::InstallMod {
            version_dir: self.root.join("versions").join(&dir),
            game_version: self.mods.game_version.clone(),
            version: Box::new(version),
            title,
        });
    }

    /// Remove a jar this launcher installed. Untracked files are never touched.
    pub fn remove_mod(&mut self, filename: &str) {
        if self.mods.installing {
            self.status = "Modrinth：安装进行中，暂时不能移除".to_string();
            return;
        }
        let Some(dir) = self.mods.dir.clone() else {
            return;
        };
        if !self
            .mods
            .installed
            .iter()
            .any(|entry| entry.filename == filename)
        {
            return;
        }
        self.log(format!("移除模组 mods/{filename}"));
        self.request(Effect::RemoveMod {
            version_dir: self.root.join("versions").join(&dir),
            filename: filename.to_string(),
        });
    }

    // ---- worker reports ----

    pub fn handle_mod_search_event(&mut self, event: modrinth::SearchEvent) {
        match event {
            modrinth::SearchEvent::Loaded { game_version, hits } => {
                if game_version != self.mods.game_version {
                    return;
                }
                self.mods.searching = false;
                self.mods.hits = hits;
                self.mods.selected = 0;
                self.mods.error = None;
                self.mods.message = Some(format!("找到 {} 个 Fabric 模组", self.mods.hits.len()));
                self.status = format!("Modrinth：{} 个结果", self.mods.hits.len());
                self.log(format!("Modrinth：{} 个结果", self.mods.hits.len()));
                if let Some(project) = self.mods.hits.first().map(|hit| hit.project_id.clone()) {
                    self.load_mod_versions(project);
                } else {
                    self.mods.versions.clear();
                    self.mods.versions_for = None;
                }
            }
            modrinth::SearchEvent::Failed(message) => {
                self.mods.searching = false;
                self.mods.error = Some(message.clone());
                self.mods.message = Some("搜索失败".to_string());
                self.status = format!("Modrinth：搜索失败：{message}");
                self.log(format!("Modrinth 搜索失败：{message}"));
            }
        }
    }

    pub fn handle_mod_versions_event(&mut self, event: modrinth::VersionsEvent) {
        match event {
            modrinth::VersionsEvent::Loaded {
                project_id,
                game_version,
                versions,
            } => {
                if self.mods.versions_for.as_deref() != Some(project_id.as_str())
                    || game_version != self.mods.game_version
                {
                    return;
                }
                self.mods.loading_versions = false;
                self.mods.error = None;
                self.mods.version_pick = modrinth::Version::default_pick(&versions).unwrap_or(0);
                self.mods.versions = versions;
                self.mods.message = Some(if self.mods.versions.is_empty() {
                    "没有适用于这个实例的 Fabric 版本".to_string()
                } else {
                    format!("{} 个可安装版本", self.mods.versions.len())
                });
                self.status = self.mods.message.clone().unwrap_or_default();
            }
            modrinth::VersionsEvent::Failed(message) => {
                self.mods.loading_versions = false;
                self.mods.error = Some(message.clone());
                self.mods.message = Some("读取版本失败".to_string());
                self.status = format!("Modrinth：版本读取失败：{message}");
                self.log(format!("Modrinth 版本读取失败：{message}"));
            }
        }
    }

    pub fn handle_mod_install_event(&mut self, event: modrinth::InstallEvent) {
        match event {
            modrinth::InstallEvent::Log(line) => {
                self.status = line.clone();
                self.log(line);
            }
            modrinth::InstallEvent::Installed(entry) => {
                if let Some(existing) = self
                    .mods
                    .installed
                    .iter_mut()
                    .find(|mod_entry| mod_entry.project_id == entry.project_id)
                {
                    *existing = entry;
                } else {
                    self.mods.installed.push(entry);
                }
            }
            modrinth::InstallEvent::Finished { installed, skipped } => {
                self.mods.installing = false;
                self.mods.installed = self
                    .mods
                    .dir
                    .as_ref()
                    .map(|dir| modrinth::installed(&self.root.join("versions").join(dir)))
                    .unwrap_or_default();
                self.mods.message = Some(if installed == 0 {
                    format!("没有新安装的模组（跳过 {skipped} 个）")
                } else {
                    format!("安装完成：{installed} 个，跳过 {skipped} 个")
                });
                self.status = self.mods.message.clone().unwrap_or_default();
                self.log(self.status.clone());
            }
            modrinth::InstallEvent::Failed(message) => {
                self.mods.installing = false;
                self.mods.error = Some(message.clone());
                self.mods.message = Some("安装失败".to_string());
                self.status = "Modrinth：安装失败".to_string();
                self.log(format!("Modrinth 安装失败：{message}"));
            }
        }
    }

    pub fn handle_mod_remove_event(&mut self, event: modrinth::RemoveEvent) {
        match event {
            modrinth::RemoveEvent::Removed { filename } => {
                self.mods
                    .installed
                    .retain(|entry| entry.filename != filename);
                self.status = format!("已移除 mods/{filename}");
                self.log(self.status.clone());
            }
            modrinth::RemoveEvent::Failed(message) => {
                self.mods.error = Some(message.clone());
                self.status = "Modrinth：移除失败".to_string();
                self.log(format!("移除模组失败：{message}"));
            }
        }
    }
}
