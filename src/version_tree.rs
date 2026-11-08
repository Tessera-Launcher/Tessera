//! The version list's tree view.
//!
//! The "全部" pane is mostly snapshots, and a snapshot cycle is dozens of rows
//! that differ only by a trailing number (`1.21.9-pre9` … `1.21.9-pre1`,
//! `25w37a` … `25w31a`). Drawing every one of them buries the releases, so the
//! list folds each run of the same family into a single row:
//!
//! ```text
//! ▶ 1.21.8
//! ▶ ▸ 1.21.9-pre9 ~ 1.21.9-pre1      ← collapsed: one line
//!   1.21.7
//! ```
//!
//! Expanding a folded row shows the members indented under it:
//!
//! ```text
//! ▶ ▾ 1.21.9-pre9 ~ 1.21.9-pre1
//! ▶   1.21.9-pre9
//!     1.21.9-pre8
//! ```
//!
//! Nothing here touches the filtered list itself: [`Row`] only indexes into it,
//! so `picked` stays a version index and installing, loader compatibility and the
//! search box keep working on concrete versions.

use crate::install::RemoteVersion;

/// How deep in the tree a row sits.
pub const CHILD_DEPTH: u8 = 1;

/// What one drawn line stands for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowKind {
    /// A single version: `index` into the filtered list.
    Version(usize),
    /// A run `first..=last` of snapshots of one family. `first` is the newest
    /// member, because the list is ordered newest first.
    Group {
        key: String,
        first: usize,
        last: usize,
        expanded: bool,
    },
}

/// One line of the version pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub kind: RowKind,
    pub depth: u8,
}

impl Row {
    /// The version this line picks when it is chosen. A folded group stands for
    /// its newest member, which is what someone collapsing a cycle wants.
    pub fn anchor(&self) -> usize {
        match self.kind {
            RowKind::Version(index) => index,
            RowKind::Group { first, .. } => first,
        }
    }

    /// Whether the keyboard stops on this row. An unfolded group's header is a
    /// toggle, not a version, so it is skipped; its children are stops instead.
    pub fn is_stop(&self) -> bool {
        match &self.kind {
            RowKind::Version(_) => true,
            RowKind::Group { expanded, .. } => !expanded,
        }
    }
}

/// The family a snapshot belongs to, or `None` when it stands on its own.
///
/// Only the shapes Mojang actually ships are folded; anything unrecognised keeps
/// its own row, which is the safe direction to be wrong in.
pub fn family(version: &RemoteVersion) -> Option<String> {
    if version.kind != "snapshot" {
        return None;
    }
    let id = version.id.as_str();

    // A base plus a trailing counter: `1.21.9-pre9`, `1.21.9-rc1`,
    // `26.4-snapshot-7`, and the dashed spelling mirrors hand out
    // (`26.3-pre-3`, `26.3-rc-3`). The separator before the number is optional.
    let stem = id.trim_end_matches(|c: char| c.is_ascii_digit());
    if stem.len() < id.len() {
        let stem = stem.strip_suffix('-').unwrap_or(stem);
        for suffix in ["-pre", "-rc", "-snapshot"] {
            if stem.len() > suffix.len() && stem.ends_with(suffix) {
                return Some(stem.to_string());
            }
        }
    }

    // Weekly snapshots: 25w37a. Folded per year, so one run can never swallow
    // several years of them.
    let bytes = id.as_bytes();
    if bytes.len() == 6
        && bytes[0].is_ascii_digit()
        && bytes[1].is_ascii_digit()
        && bytes[2] == b'w'
        && bytes[3].is_ascii_digit()
        && bytes[4].is_ascii_digit()
        && bytes[5].is_ascii_alphabetic()
    {
        return Some(format!("weekly-{}", &id[..2]));
    }

    None
}

/// The label of a folded row: `newest ~ oldest`.
pub fn title(versions: &[RemoteVersion], first: usize, last: usize) -> String {
    match (versions.get(first), versions.get(last)) {
        (Some(newest), Some(oldest)) => format!("{} ~ {}", newest.id, oldest.id),
        _ => String::new(),
    }
}

/// Lay the filtered list out as rows.
///
/// `tree` is false for the other filters (and for a search), where every version
/// keeps its own line exactly as before.
pub fn build(versions: &[RemoteVersion], expanded: &[String], tree: bool) -> Vec<Row> {
    let mut rows = Vec::with_capacity(versions.len().max(1));
    let mut index = 0;
    while index < versions.len() {
        let key = if tree { family(&versions[index]) } else { None };
        if let Some(key) = key {
            let mut last = index;
            while last + 1 < versions.len() && family(&versions[last + 1]).as_deref() == Some(&key)
            {
                last += 1;
            }
            // A "group" of one is just a version; folding it would only add a
            // pointless level to click through.
            if last > index {
                let expanded = expanded.iter().any(|open| open == &key);
                rows.push(Row {
                    kind: RowKind::Group {
                        key,
                        first: index,
                        last,
                        expanded,
                    },
                    depth: 0,
                });
                if expanded {
                    rows.extend((index..=last).map(|child| Row {
                        kind: RowKind::Version(child),
                        depth: CHILD_DEPTH,
                    }));
                }
                index = last + 1;
                continue;
            }
        }
        rows.push(Row {
            kind: RowKind::Version(index),
            depth: 0,
        });
        index += 1;
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(id: &str, kind: &str) -> RemoteVersion {
        RemoteVersion {
            id: id.to_string(),
            kind: kind.to_string(),
            released: "2025-01-01T00:00:00+00:00".to_string(),
        }
    }

    fn snapshot(id: &str) -> RemoteVersion {
        version(id, "snapshot")
    }

    /// The real shape of the tail of Mojang's list: a release, a pre-release
    /// cycle, an older release, then weekly snapshots.
    fn list() -> Vec<RemoteVersion> {
        ["1.21.8", "1.21.8-rc1"]
            .map(|id| version(id, "release"))
            .into_iter()
            .chain((1..=3).rev().map(|n| snapshot(&format!("1.21.9-pre{n}"))))
            .chain(["1.21.7"].map(|id| version(id, "release")))
            .chain((31..=33).rev().map(|n| snapshot(&format!("25w{n}a"))))
            .collect()
    }

    #[test]
    fn families_are_recognised_for_the_shapes_mojang_ships() {
        let cases = [
            ("1.21.9-pre9", Some("1.21.9-pre")),
            ("1.21.9-rc2", Some("1.21.9-rc")),
            ("26.4-snapshot-7", Some("26.4-snapshot")),
            // Mirrors that spell the counter with a separator.
            ("26.3-pre-3", Some("26.3-pre")),
            ("26.3-rc-3", Some("26.3-rc")),
            ("26.3-pre-12", Some("26.3-pre")),
            ("25w37a", Some("weekly-25")),
            ("24w14a", Some("weekly-24")),
            // Not snapshots, or not a shape worth folding.
            ("1.21.8", None),
            ("a1.2.6", None),
            ("pre", None),
            ("1.21.9-pre", None),
            ("snapshot-7", None),
        ];
        for (id, expected) in cases {
            assert_eq!(
                family(&snapshot(id)).as_deref(),
                expected,
                "{id} should fold as {expected:?}"
            );
        }
        // A release of the same name never folds, whatever it looks like.
        assert_eq!(family(&version("1.21.9", "release")), None);
    }

    #[test]
    fn a_snapshot_run_folds_into_one_row_newest_first() {
        let versions = list();
        let rows = build(&versions, &[], true);
        let ids: Vec<String> = rows
            .iter()
            .map(|row| match &row.kind {
                RowKind::Version(index) => versions[*index].id.clone(),
                RowKind::Group { first, last, .. } => title(&versions, *first, *last),
            })
            .collect();
        assert_eq!(
            ids,
            [
                "1.21.8",
                "1.21.8-rc1",
                "1.21.9-pre3 ~ 1.21.9-pre1",
                "1.21.7",
                "25w33a ~ 25w31a",
            ]
        );
        assert!(rows.iter().all(|row| row.depth == 0 && row.is_stop()));
    }

    #[test]
    fn the_flat_view_keeps_every_version_on_its_own_row() {
        let versions = list();
        let rows = build(&versions, &[], false);
        assert_eq!(rows.len(), versions.len());
        assert!(rows
            .iter()
            .all(|row| matches!(row.kind, RowKind::Version(_))));
    }

    #[test]
    fn expanding_a_group_indents_its_members_under_it() {
        let versions = list();
        let rows = build(&versions, &["1.21.9-pre".to_string()], true);
        let shape: Vec<(u8, String)> = rows
            .iter()
            .map(|row| {
                let label = match &row.kind {
                    RowKind::Version(index) => versions[*index].id.clone(),
                    RowKind::Group { first, last, .. } => title(&versions, *first, *last),
                };
                (row.depth, label)
            })
            .collect();
        assert_eq!(
            shape,
            [
                (0, "1.21.8".to_string()),
                (0, "1.21.8-rc1".to_string()),
                (0, "1.21.9-pre3 ~ 1.21.9-pre1".to_string()),
                (1, "1.21.9-pre3".to_string()),
                (1, "1.21.9-pre2".to_string()),
                (1, "1.21.9-pre1".to_string()),
                (0, "1.21.7".to_string()),
                (0, "25w33a ~ 25w31a".to_string()),
            ]
        );
        // The header is a toggle; its children are what the keyboard stops on.
        assert!(!rows[2].is_stop(), "the unfolded header is not a version");
        assert!(rows[3].is_stop());
        assert_eq!(
            rows[2].anchor(),
            2,
            "the header stands for its newest member"
        );
    }

    #[test]
    fn a_single_snapshot_is_not_folded() {
        let versions = vec![
            version("1.21.8", "release"),
            snapshot("1.21.9-pre1"),
            version("1.21.7", "release"),
        ];
        let rows = build(&versions, &[], true);
        assert_eq!(rows.len(), 3);
        assert!(rows
            .iter()
            .all(|row| matches!(row.kind, RowKind::Version(_))));
    }

    #[test]
    fn runs_are_split_by_anything_that_is_not_the_same_family() {
        let versions = vec![
            snapshot("25w33a"),
            snapshot("25w32a"),
            version("1.21.8", "release"),
            snapshot("25w31a"),
            snapshot("24w14a"),
        ];
        // Same family, but a release in between: two runs, not one.
        let rows = build(&versions, &[], true);
        assert_eq!(rows.len(), 4);
        // A different year is a different family even when adjacent.
        let versions = vec![snapshot("25w31a"), snapshot("24w14a")];
        let rows = build(&versions, &[], true);
        assert_eq!(rows.len(), 2, "years do not fold together");
    }

    #[test]
    fn an_unknown_shape_keeps_its_own_row() {
        let versions = vec![
            snapshot("1.21.9-pre9"),
            snapshot("1.21.9-pre8"),
            snapshot("weird-build"),
        ];
        let rows = build(&versions, &[], true);
        assert_eq!(rows.len(), 2, "{rows:?}");
    }
}
