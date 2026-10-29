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

#[cfg(test)]
mod tests {
    use super::*;
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
