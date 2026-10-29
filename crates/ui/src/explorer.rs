//! Bounded, read-only directory listings for the workspace sidebar.
use std::path::{Path, PathBuf};

pub const MAX_ENTRIES: usize = 500;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub directory: bool,
}

pub fn list(root: &Path, directory: &Path) -> Result<(Vec<Entry>, bool), String> {
    let relative = directory
        .strip_prefix(root)
        .map_err(|_| "Directory is outside the selected folder.")?;
    if root
        .components()
        .chain(relative.components())
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err("Parent-directory traversal is not supported.".into());
    }
    // Do not follow symlinks, including any ancestor between root and directory.
    for path in directory.ancestors() {
        let metadata = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
        if metadata.file_type().is_symlink() {
            return Err("Symbolic-link folders are not supported.".into());
        }
    }
    let mut entries = Vec::new();
    let mut truncated = false;
    for (index, item) in std::fs::read_dir(directory)
        .map_err(|e| e.to_string())?
        .enumerate()
    {
        if index == 5000 {
            truncated = true;
            break;
        }
        let item = item.map_err(|e| e.to_string())?;
        let kind = item.file_type().map_err(|e| e.to_string())?;
        if !kind.is_dir() && !kind.is_file() {
            continue;
        }
        if entries.len() == MAX_ENTRIES {
            truncated = true;
            break;
        }
        let name = item.file_name().to_string_lossy().into_owned();
        entries.push(Entry {
            path: item.path(),
            name,
            directory: kind.is_dir(),
        });
    }
    entries.sort_by(|a, b| {
        b.directory
            .cmp(&a.directory)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok((entries, truncated))
}

const MAX_FOLDERS: usize = 128;
const MAX_VISIBLE_ROWS: usize = 2500;
const MAX_DEPTH: usize = 32;

#[derive(Default)]
struct Folder {
    expanded: bool,
    entries: Option<Vec<Entry>>,
    request: Option<(u64, u64)>,
    note: Option<String>,
}

#[derive(Clone)]
pub struct Row {
    pub path: PathBuf,
    pub name: String,
    pub directory: bool,
    pub expanded: bool,
    pub depth: usize,
    pub note: bool,
}

#[derive(Default)]
pub struct Tree {
    pub root: Option<PathBuf>,
    folders: std::collections::BTreeMap<PathBuf, Folder>,
    epoch: u64,
    sequence: u64,
}

impl Tree {
    pub fn reset(&mut self, root: PathBuf) {
        self.epoch = self.epoch.wrapping_add(1);
        self.folders.clear();
        self.folders.insert(
            root.clone(),
            Folder {
                expanded: true,
                ..Folder::default()
            },
        );
        self.root = Some(root);
    }

    pub fn toggle(&mut self, path: &Path) -> bool {
        if let Some(folder) = self.folders.get_mut(path) {
            folder.expanded = !folder.expanded;
            return folder.expanded && folder.entries.is_none() && folder.request.is_none();
        }
        if self.folders.len() >= MAX_FOLDERS {
            if let Some(root) = self
                .root
                .as_ref()
                .and_then(|root| self.folders.get_mut(root))
            {
                root.note =
                    Some("Folder limit reached; collapse folders and Refresh to load more.".into());
            }
            return false;
        }
        self.folders.insert(
            path.to_path_buf(),
            Folder {
                expanded: true,
                ..Folder::default()
            },
        );
        true
    }

    pub fn begin(&mut self, path: &Path) -> Option<(u64, u64)> {
        let folder = self.folders.get_mut(path)?;
        self.sequence = self.sequence.wrapping_add(1);
        let token = (self.epoch, self.sequence);
        folder.request = Some(token);
        folder.note = None;
        Some(token)
    }

    pub fn finish(
        &mut self,
        path: &Path,
        token: (u64, u64),
        result: Result<(Vec<Entry>, bool), String>,
    ) {
        if token.0 != self.epoch {
            return;
        }
        let Some(folder) = self.folders.get_mut(path) else {
            return;
        };
        if folder.request != Some(token) {
            return;
        }
        folder.request = None;
        match result {
            Ok((entries, limited)) => {
                folder.note = if limited {
                    Some("Listing limited; some entries are omitted.".into())
                } else if entries.is_empty() {
                    Some("Empty folder".into())
                } else {
                    None
                };
                folder.entries = Some(entries);
            }
            Err(error) => {
                folder.entries = None;
                folder.note = Some(format!("Cannot read folder: {error}"));
            }
        }
    }

    pub fn refresh(&mut self) -> Vec<PathBuf> {
        self.epoch = self.epoch.wrapping_add(1);
        self.folders
            .retain(|path, folder| folder.expanded || self.root.as_ref() == Some(path));
        for folder in self.folders.values_mut() {
            folder.entries = None;
            folder.request = None;
            folder.note = None;
        }
        self.folders
            .iter()
            .filter(|(_, folder)| folder.expanded)
            .map(|(path, _)| path.clone())
            .collect()
    }

    pub fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        let Some(root) = self.root.as_ref() else {
            return rows;
        };
        self.append_folder(root, 0, &mut rows);
        rows
    }

    fn append_folder(&self, path: &Path, depth: usize, rows: &mut Vec<Row>) {
        if rows.len() >= MAX_VISIBLE_ROWS {
            return;
        }
        let folder = self.folders.get(path);
        let expanded = folder.is_some_and(|folder| folder.expanded);
        rows.push(Row {
            path: path.to_path_buf(),
            name: path
                .file_name()
                .unwrap_or(path.as_os_str())
                .to_string_lossy()
                .into_owned(),
            directory: true,
            expanded,
            depth,
            note: false,
        });
        if !expanded {
            return;
        }
        let folder = folder.unwrap();
        if let Some(entries) = folder.entries.as_ref() {
            for entry in entries {
                if rows.len() >= MAX_VISIBLE_ROWS {
                    return;
                }
                if rows.len() == MAX_VISIBLE_ROWS - 1 {
                    rows.push(Row {
                        path: path.to_path_buf(),
                        name: "Tree display limit reached; collapse folders to see more.".into(),
                        directory: false,
                        expanded: false,
                        depth: depth + 1,
                        note: true,
                    });
                    return;
                }
                if entry.directory {
                    if depth < MAX_DEPTH {
                        self.append_folder(&entry.path, depth + 1, rows);
                    } else {
                        rows.push(Row {
                            path: entry.path.clone(),
                            name: format!("{} (depth limit)", entry.name),
                            directory: false,
                            expanded: false,
                            depth: depth + 1,
                            note: true,
                        });
                    }
                } else {
                    rows.push(Row {
                        path: entry.path.clone(),
                        name: entry.name.clone(),
                        directory: false,
                        expanded: false,
                        depth: depth + 1,
                        note: false,
                    });
                }
            }
        }
        let note = if folder.request.is_some() {
            Some("Loading…")
        } else {
            folder.note.as_deref()
        };
        if rows.len() < MAX_VISIBLE_ROWS {
            if let Some(note) = note {
                rows.push(Row {
                    path: path.to_path_buf(),
                    name: note.into(),
                    directory: false,
                    expanded: false,
                    depth: depth + 1,
                    note: true,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn entry(root: &Path, name: &str, directory: bool) -> Entry {
        Entry {
            path: root.join(name),
            name: name.into(),
            directory,
        }
    }

    #[test]
    fn nested_expansion_keeps_siblings_and_collapse_preserves_cached_children() {
        let root = Path::new("/project");
        let child = root.join("src");
        let mut tree = Tree::default();
        tree.reset(root.into());
        let token = tree.begin(root).unwrap();
        tree.finish(
            root,
            token,
            Ok((
                vec![entry(root, "src", true), entry(root, "README", false)],
                false,
            )),
        );
        assert!(tree.toggle(&child));
        let token = tree.begin(&child).unwrap();
        tree.finish(
            &child,
            token,
            Ok((vec![entry(&child, "main.rs", false)], false)),
        );
        let rows = tree.rows();
        assert_eq!(
            rows.iter()
                .map(|r| (r.name.as_str(), r.depth))
                .collect::<Vec<_>>(),
            [("project", 0), ("src", 1), ("main.rs", 2), ("README", 1)]
        );
        assert!(!tree.toggle(&child));
        assert_eq!(tree.rows().len(), 3);
        assert!(!tree.toggle(&child));
        assert_eq!(tree.rows().len(), 4);
    }

    #[test]
    fn stale_results_cannot_replace_new_root_or_newer_refresh() {
        let root = Path::new("/project");
        let mut tree = Tree::default();
        tree.reset(root.into());
        let old = tree.begin(root).unwrap();
        tree.reset(root.into());
        let current = tree.begin(root).unwrap();
        tree.finish(root, old, Ok((vec![entry(root, "old", false)], false)));
        assert!(!tree.rows().iter().any(|r| r.name == "old"));
        tree.finish(root, current, Ok((vec![entry(root, "new", false)], false)));
        let paths = tree.refresh();
        assert_eq!(paths, vec![root.to_path_buf()]);
        let refresh = tree.begin(root).unwrap();
        tree.finish(
            root,
            current,
            Ok((vec![entry(root, "stale", false)], false)),
        );
        tree.finish(
            root,
            refresh,
            Ok((vec![entry(root, "fresh", false)], false)),
        );
        assert!(tree.rows().iter().any(|r| r.name == "fresh"));
        assert!(!tree.rows().iter().any(|r| r.name == "stale"));
    }

    #[test]
    fn collapse_during_load_does_not_reopen_folder() {
        let root = Path::new("/project");
        let child = root.join("src");
        let mut tree = Tree::default();
        tree.reset(root.into());
        let token = tree.begin(root).unwrap();
        tree.finish(root, token, Ok((vec![entry(root, "src", true)], false)));
        assert!(tree.toggle(&child));
        let token = tree.begin(&child).unwrap();
        assert!(!tree.toggle(&child));
        tree.finish(
            &child,
            token,
            Ok((vec![entry(&child, "hidden", false)], false)),
        );
        assert!(!tree.rows().iter().any(|r| r.name == "hidden"));
        assert!(!tree.toggle(&child));
        assert!(tree.rows().iter().any(|r| r.name == "hidden"));
    }

    #[test]
    fn refresh_retains_open_branches_and_closed_root_can_reopen() {
        let root = Path::new("/project");
        let child = root.join("src");
        let mut tree = Tree::default();
        tree.reset(root.into());
        tree.toggle(&child);
        let paths = tree.refresh();
        assert_eq!(paths, vec![root.to_path_buf(), child]);
        tree.toggle(root);
        assert_eq!(tree.rows().len(), 1);
        tree.refresh();
        assert!(tree.toggle(root));
    }

    #[test]
    fn folder_errors_are_visible_under_their_branch() {
        let root = Path::new("/project");
        let mut tree = Tree::default();
        tree.reset(root.into());
        let token = tree.begin(root).unwrap();
        tree.finish(root, token, Err("Permission denied".into()));
        let rows = tree.rows();
        assert!(rows[1].note);
        assert!(rows[1].name.contains("Permission denied"));
    }

    #[test]
    fn visible_rows_and_folder_cache_are_bounded() {
        let root = Path::new("/project");
        let mut tree = Tree::default();
        tree.reset(root.into());
        let folders: Vec<_> = (0..6)
            .map(|i| entry(root, &format!("dir{i}"), true))
            .collect();
        let token = tree.begin(root).unwrap();
        tree.finish(root, token, Ok((folders.clone(), false)));
        for folder in folders {
            tree.toggle(&folder.path);
            let token = tree.begin(&folder.path).unwrap();
            let files = (0..MAX_ENTRIES)
                .map(|i| entry(&folder.path, &format!("file{i}"), false))
                .collect();
            tree.finish(&folder.path, token, Ok((files, false)));
        }
        assert_eq!(tree.rows().len(), MAX_VISIBLE_ROWS);
        assert!(tree.rows().last().unwrap().note);
        for i in 0..MAX_FOLDERS * 2 {
            tree.toggle(&root.join(format!("extra{i}")));
        }
        assert_eq!(tree.folders.len(), MAX_FOLDERS);
    }

    #[test]
    fn lists_folders_first_and_rejects_paths_outside_root() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        std::fs::create_dir(root.join("folder")).unwrap();
        std::fs::write(root.join("z.txt"), b"z").unwrap();
        std::fs::write(root.join("a.txt"), b"a").unwrap();
        let (entries, truncated) = list(root, root).unwrap();
        assert!(!truncated);
        assert_eq!(
            entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
            ["folder", "a.txt", "z.txt"]
        );
        assert!(list(root, root.parent().unwrap()).is_err());
        assert!(list(root, &root.join("../outside")).is_err());
    }
    #[test]
    fn caps_large_folders_and_reports_omissions() {
        let temp = tempfile::tempdir().unwrap();
        for index in 0..=MAX_ENTRIES {
            std::fs::write(temp.path().join(format!("file-{index:04}.txt")), b"x").unwrap();
        }
        let (entries, truncated) = list(temp.path(), temp.path()).unwrap();
        assert!(truncated);
        assert_eq!(entries.len(), MAX_ENTRIES);
    }
    #[cfg(unix)]
    #[test]
    fn excludes_symbolic_links() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        std::os::unix::fs::symlink("/tmp", root.join("link")).unwrap();
        assert!(list(root, root).unwrap().0.is_empty());
        assert!(list(root, &root.join("link")).is_err());
    }
}
