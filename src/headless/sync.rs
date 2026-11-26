//! Compare local and remote file listings and plan a bidirectional sync.
//!
//! The comparison is display-free and does no I/O: the GUI collects the two
//! listings, shows the plans and hands the selected ones to the transfer queue.
//! With no common ancestor available, the newer mtime decides a one-sided
//! change; different sizes at (almost) the same time are a conflict.

use std::collections::HashMap;

/// Size and modification time of one file on both sides. Zero means the file
/// is missing on that side.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyncItem {
    pub path: String,
    pub local_size: u64,
    pub remote_size: u64,
    pub local_mtime: i64,
    pub remote_mtime: i64,
}

/// What should happen to an item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncAction {
    Upload,
    Download,
    Same,
    Conflict,
}

/// A comparison result the user can accept or reject.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyncPlan {
    pub item: SyncItem,
    pub action: SyncAction,
    pub selected: bool,
}

/// Merge the two listings by path and decide an action for each file.
/// Results are sorted by path; duplicate paths keep the first entry.
///
/// Files present on one side only are uploaded or downloaded. When both sides
/// have the file, equal sizes and mtimes within `tolerance_secs` mean `Same`;
/// otherwise a newer local or remote mtime wins, and differing sizes with
/// mtimes inside the tolerance are a `Conflict`. Only `Upload` and `Download`
/// plans start out selected.
pub fn compare(local: &[SyncItem], remote: &[SyncItem], tolerance_secs: i64) -> Vec<SyncPlan> {
    let mut local_by_path: HashMap<&str, &SyncItem> = HashMap::new();
    let mut remote_by_path: HashMap<&str, &SyncItem> = HashMap::new();
    for item in local {
        local_by_path.entry(item.path.as_str()).or_insert(item);
    }
    for item in remote {
        remote_by_path.entry(item.path.as_str()).or_insert(item);
    }

    let mut paths: Vec<&str> = local_by_path
        .keys()
        .chain(remote_by_path.keys())
        .copied()
        .collect();
    paths.sort_unstable();
    paths.dedup();

    let tolerance = tolerance_secs.unsigned_abs();
    let mut plans = Vec::with_capacity(paths.len());
    for path in paths {
        let local_item = local_by_path.get(path).copied();
        let remote_item = remote_by_path.get(path).copied();
        let (item, action) = match (local_item, remote_item) {
            (Some(l), None) => (
                SyncItem {
                    path: path.to_string(),
                    local_size: l.local_size,
                    remote_size: 0,
                    local_mtime: l.local_mtime,
                    remote_mtime: 0,
                },
                SyncAction::Upload,
            ),
            (None, Some(r)) => (
                SyncItem {
                    path: path.to_string(),
                    local_size: 0,
                    remote_size: r.remote_size,
                    local_mtime: 0,
                    remote_mtime: r.remote_mtime,
                },
                SyncAction::Download,
            ),
            (Some(l), Some(r)) => {
                let delta = l.local_mtime.saturating_sub(r.remote_mtime).unsigned_abs();
                let action = if l.local_size == r.remote_size && delta <= tolerance {
                    SyncAction::Same
                } else if delta <= tolerance {
                    SyncAction::Conflict
                } else if l.local_mtime > r.remote_mtime {
                    SyncAction::Upload
                } else {
                    SyncAction::Download
                };
                (
                    SyncItem {
                        path: path.to_string(),
                        local_size: l.local_size,
                        remote_size: r.remote_size,
                        local_mtime: l.local_mtime,
                        remote_mtime: r.remote_mtime,
                    },
                    action,
                )
            }
            // Unreachable for paths collected from the two maps, but skipping
            // is safer than panicking if this function is ever changed.
            (None, None) => continue,
        };
        plans.push(SyncPlan {
            item,
            action,
            selected: matches!(action, SyncAction::Upload | SyncAction::Download),
        });
    }
    plans
}

/// Select or unselect every plan with this path. Returns whether one was found.
pub fn set_selected(plan: &mut [SyncPlan], path: &str, selected: bool) -> bool {
    let mut found = false;
    for entry in plan.iter_mut().filter(|entry| entry.item.path == path) {
        entry.selected = selected;
        found = true;
    }
    found
}

/// Select every plan, including `Same` and `Conflict` ones.
pub fn select_all(plan: &mut [SyncPlan]) {
    for entry in plan {
        entry.selected = true;
    }
}

/// Selected transfers as `(direction, path)` pairs, in plan order.
/// `Same` and `Conflict` entries are never executable, even when selected.
pub fn to_operations(plan: &[SyncPlan]) -> Vec<(String, String)> {
    plan.iter()
        .filter(|entry| entry.selected)
        .filter_map(|entry| {
            let direction = match entry.action {
                SyncAction::Upload => "upload",
                SyncAction::Download => "download",
                SyncAction::Same | SyncAction::Conflict => return None,
            };
            Some((direction.to_string(), entry.item.path.clone()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(
        path: &str,
        local_size: u64,
        remote_size: u64,
        local_mtime: i64,
        remote_mtime: i64,
    ) -> SyncItem {
        SyncItem {
            path: path.to_string(),
            local_size,
            remote_size,
            local_mtime,
            remote_mtime,
        }
    }

    #[test]
    fn one_sided_files_are_uploaded_or_downloaded_in_path_order() {
        let local = [item("z.txt", 10, 0, 100, 0)];
        let remote = [item("a.txt", 0, 20, 0, 200)];
        let plans = compare(&local, &remote, 2);
        assert_eq!(plans.len(), 2);
        assert_eq!(plans[0].item.path, "a.txt");
        assert_eq!(plans[0].action, SyncAction::Download);
        assert_eq!(plans[0].item.remote_size, 20);
        assert!(plans[0].selected);
        assert_eq!(plans[1].item.path, "z.txt");
        assert_eq!(plans[1].action, SyncAction::Upload);
        assert_eq!(plans[1].item.local_size, 10);
        assert!(plans[1].selected);
    }

    #[test]
    fn newer_mtime_decides_a_one_sided_change() {
        let both = [
            item("newer.txt", 10, 10, 500, 100),
            item("older.txt", 10, 10, 100, 500),
        ];
        let plans = compare(&both, &both, 2);
        assert_eq!(plans[0].action, SyncAction::Upload);
        assert_eq!(plans[1].action, SyncAction::Download);
    }

    #[test]
    fn two_second_tolerance_counts_as_same() {
        let newer = [item("a.txt", 10, 10, 102, 100)];
        assert_eq!(compare(&newer, &newer, 2)[0].action, SyncAction::Same);
        assert_eq!(compare(&newer, &newer, -2)[0].action, SyncAction::Same);
        assert_eq!(compare(&newer, &newer, 1)[0].action, SyncAction::Upload);
        let older = [item("a.txt", 10, 10, 98, 100)];
        assert_eq!(compare(&older, &older, 2)[0].action, SyncAction::Same);
        assert_eq!(compare(&older, &older, 1)[0].action, SyncAction::Download);
    }

    #[test]
    fn differing_sizes_with_close_times_are_a_conflict() {
        let both = [item("a.txt", 10, 7, 101, 100)];
        let plans = compare(&both, &both, 2);
        assert_eq!(plans[0].action, SyncAction::Conflict);
        assert!(!plans[0].selected);

        // A clear mtime difference still picks the newer side.
        let newer = [item("a.txt", 10, 7, 200, 100)];
        assert_eq!(compare(&newer, &newer, 2)[0].action, SyncAction::Upload);
        let older = [item("a.txt", 10, 7, 100, 200)];
        assert_eq!(compare(&older, &older, 2)[0].action, SyncAction::Download);
    }

    #[test]
    fn selection_controls_the_generated_operations() {
        let local = [item("a.txt", 10, 0, 100, 0), item("b.txt", 0, 20, 0, 200)];
        let both = [
            item("c.txt", 10, 10, 100, 100),
            item("d.txt", 10, 7, 100, 100),
        ];
        let mut plans = compare(&local, &local, 2);
        plans.extend(compare(&both, &both, 2));
        plans.sort_by(|a, b| a.item.path.cmp(&b.item.path));
        assert_eq!(
            plans.iter().map(|plan| plan.selected).collect::<Vec<_>>(),
            vec![true, true, false, false]
        );
        assert_eq!(
            to_operations(&plans),
            vec![
                ("upload".to_string(), "a.txt".to_string()),
                ("download".to_string(), "b.txt".to_string()),
            ]
        );

        assert!(set_selected(&mut plans, "a.txt", false));
        assert!(!set_selected(&mut plans, "missing.txt", true));
        assert_eq!(
            to_operations(&plans),
            vec![("download".to_string(), "b.txt".to_string())]
        );

        select_all(&mut plans);
        assert!(plans.iter().all(|plan| plan.selected));
        // Same and Conflict plans stay non-executable when selected.
        assert_eq!(
            to_operations(&plans),
            vec![
                ("upload".to_string(), "a.txt".to_string()),
                ("download".to_string(), "b.txt".to_string()),
            ]
        );
    }
}
