//! Background workspace search. File enumeration uses ripgrep's ignore rules.
use globset::{Glob, GlobSet, GlobSetBuilder};
use regex::RegexBuilder;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    io::Read,
    ops::Range,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

const MAX_FILES: usize = 50_000;
const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_RESULTS: usize = 10_000;
const MAX_WARNINGS: usize = 20;

#[derive(Clone, Default)]
pub(crate) struct SearchOptions {
    pub query: String,
    pub regex: bool,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub include: String,
    pub exclude: String,
}

pub(crate) struct Overlay {
    pub path: PathBuf,
    pub text: String,
}

#[derive(Clone, Debug)]
pub(crate) struct SearchMatch {
    pub path: PathBuf,
    pub line: usize,
    pub column: usize,
    pub range: Range<usize>,
    pub preview: String,
    pub digest: [u8; 32],
}

#[derive(Default)]
pub(crate) struct SearchReport {
    pub matches: Vec<SearchMatch>,
    pub total: usize,
    pub truncated: bool,
    pub cancelled: bool,
    pub warnings: Vec<String>,
}

fn patterns(source: &str) -> Result<GlobSet, String> {
    let mut builder = GlobSetBuilder::new();
    for item in source.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        builder.add(Glob::new(item).map_err(|e| format!("Invalid glob {item:?}: {e}"))?);
    }
    builder.build().map_err(|e| e.to_string())
}

fn warning(report: &mut SearchReport, message: String) {
    report.truncated = true;
    if report.warnings.len() < MAX_WARNINGS {
        report.warnings.push(message);
    }
}

/// Counts include all matches scanned, including those beyond the display limit.
/// Truncation means enumeration, reading, or result storage was incomplete.
pub(crate) fn search(
    root: &Path,
    overlays: Vec<Overlay>,
    options: SearchOptions,
    cancel: &AtomicBool,
) -> Result<SearchReport, String> {
    if options.query.is_empty() {
        return Err("Enter a search query.".into());
    }
    let include = patterns(&options.include)?;
    let exclude = patterns(&options.exclude)?;
    let expression = if options.regex {
        options.query.clone()
    } else {
        regex::escape(&options.query)
    };
    let expression = if options.whole_word {
        format!(r"\b(?:{expression})\b")
    } else {
        expression
    };
    let matcher = RegexBuilder::new(&expression)
        .case_insensitive(!options.case_sensitive)
        .size_limit(4 * 1024 * 1024)
        .build()
        .map_err(|error| format!("Invalid search expression: {error}"))?;
    let root = root
        .canonicalize()
        .map_err(|e| format!("Cannot open workspace: {e}"))?;
    let overlays: HashMap<PathBuf, String> = overlays
        .into_iter()
        .map(|item| {
            let path = item.path.canonicalize().unwrap_or(item.path);
            (path, item.text)
        })
        .collect();
    let mut exclusions = ignore::overrides::OverrideBuilder::new(&root);
    for pattern in ["!**/.git", "!**/target", "!**/node_modules"] {
        exclusions.add(pattern).map_err(|e| e.to_string())?;
    }
    let mut builder = ignore::WalkBuilder::new(&root);
    builder
        .hidden(false)
        .require_git(false)
        .add_custom_ignore_filename(".rgignore")
        .overrides(exclusions.build().map_err(|e| e.to_string())?);
    let mut rules = builder.build_matchers().remove(0);
    let open_files = overlays.keys().filter_map(|path| {
        let relative = path.strip_prefix(&root).ok()?;
        (!rules.matched(relative, false).is_ignore()).then(|| Ok(path.clone()))
    });
    let disk_files = builder.build().filter_map(|entry| match entry {
        Ok(entry)
            if entry.file_type().is_some_and(|kind| kind.is_file())
                && !overlays.contains_key(entry.path()) =>
        {
            Some(Ok(entry.into_path()))
        }
        Ok(_) => None,
        Err(error) => Some(Err(error)),
    });
    let mut report = SearchReport::default();
    for (index, entry) in open_files.chain(disk_files).enumerate() {
        if cancel.load(Ordering::Relaxed) {
            report.cancelled = true;
            report.truncated = true;
            break;
        }
        if index == MAX_FILES {
            warning(&mut report, format!("Stopped after {MAX_FILES} files."));
            break;
        }
        let path = match entry {
            Ok(path) => path,
            Err(error) => {
                warning(&mut report, format!("File listing failed: {error}"));
                continue;
            }
        };
        let relative = path.strip_prefix(&root).expect("workspace-relative path");
        if (!options.include.trim().is_empty() && !include.is_match(relative))
            || exclude.is_match(relative)
        {
            continue;
        }
        let text = if let Some(text) = overlays.get(&path) {
            text.clone()
        } else {
            let metadata = match std::fs::symlink_metadata(&path) {
                Ok(meta) if meta.is_file() && !meta.file_type().is_symlink() => meta,
                Ok(_) => continue,
                Err(e) => {
                    warning(
                        &mut report,
                        format!("Cannot inspect {}: {e}", relative.display()),
                    );
                    continue;
                }
            };
            if metadata.len() > MAX_FILE_BYTES {
                warning(
                    &mut report,
                    format!("Skipped large file {}.", relative.display()),
                );
                continue;
            }
            let mut open = std::fs::OpenOptions::new();
            open.read(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                open.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
            }
            let mut bytes = Vec::new();
            match open
                .open(&path)
                .and_then(|file| file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes))
            {
                Ok(_) if bytes.len() as u64 <= MAX_FILE_BYTES => {}
                Ok(_) => {
                    warning(
                        &mut report,
                        format!("Skipped large file {}.", relative.display()),
                    );
                    continue;
                }
                Err(e) => {
                    warning(
                        &mut report,
                        format!("Cannot read {}: {e}", relative.display()),
                    );
                    continue;
                }
            }
            if bytes.contains(&0) {
                continue;
            }
            match String::from_utf8(bytes) {
                Ok(text) => text,
                Err(_) => continue,
            }
        };
        if text
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
        {
            continue;
        }
        // Each match range is reported in Unicode scalar offsets, as used by the editor.
        let digest: [u8; 32] = Sha256::digest(text.as_bytes()).into();
        let mut last_byte = 0;
        let mut scalar = 0;
        let mut line = 0;
        let mut line_start = 0;
        let mut column = 0;
        for found in matcher.find_iter(&text) {
            if cancel.load(Ordering::Relaxed) {
                report.cancelled = true;
                report.truncated = true;
                break;
            }
            let prefix = &text[last_byte..found.start()];
            scalar += prefix.chars().count();
            line += prefix.bytes().filter(|byte| *byte == b'\n').count();
            if let Some(pos) = prefix.rfind('\n') {
                line_start = last_byte + pos + 1;
                column = prefix[pos + 1..].chars().count();
            } else {
                column += prefix.chars().count();
            }
            let end = scalar + found.as_str().chars().count();
            report.total += 1;
            if report.matches.len() < MAX_RESULTS {
                let preview = text[line_start..]
                    .chars()
                    .take_while(|c| *c != '\n')
                    .take(300)
                    .collect();
                report.matches.push(SearchMatch {
                    path: path.clone(),
                    line,
                    column,
                    range: scalar..end,
                    preview,
                    digest,
                });
            } else {
                report.truncated = true;
            }
            line += found.as_str().bytes().filter(|byte| *byte == b'\n').count();
            if let Some(pos) = found.as_str().rfind('\n') {
                line_start = found.start() + pos + 1;
                column = found.as_str()[pos + 1..].chars().count();
            } else {
                column += found.as_str().chars().count();
            }
            scalar = end;
            last_byte = found.end();
        }
        if report.cancelled {
            break;
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn deleted_buffers_follow_workspace_ignore_rules() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(".gitignore"), "ignored*.rs").unwrap();
        let overlays = ["gone.rs", "ignored-gone.rs", "target/gone.rs"]
            .into_iter()
            .map(|name| Overlay {
                path: dir.path().join(name),
                text: "needle".into(),
            })
            .collect();
        let report = search(
            dir.path(),
            overlays,
            SearchOptions {
                query: "needle".into(),
                ..Default::default()
            },
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(report.total, 1);
        assert_eq!(report.matches[0].path, dir.path().join("gone.rs"));
        assert!(!report.truncated);
    }

    #[test]
    fn searches_unsaved_overlays_and_respects_ignore_filters_and_unicode() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join(".gitignore"), "ignored.rs\n").unwrap();
        fs::write(root.join("ignored.rs"), "needle").unwrap();
        fs::create_dir(root.join("target")).unwrap();
        fs::write(root.join("target/generated.rs"), "needle").unwrap();
        fs::write(root.join("binary.rs"), b"needle\0needle").unwrap();
        fs::write(root.join("main.rs"), "old contents").unwrap();
        fs::write(root.join("other.txt"), "needle").unwrap();
        let report = search(
            root,
            vec![Overlay {
                path: root.join("main.rs"),
                text: "λ needle\nNEEDLE\nneedles".into(),
            }],
            SearchOptions {
                query: "needle".into(),
                whole_word: true,
                include: "*.rs".into(),
                ..Default::default()
            },
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(report.total, 2);
        assert!(!report.truncated, "{:?}", report.warnings);
        assert_eq!(report.matches[0].range, 2..8);
        assert_eq!((report.matches[1].line, report.matches[1].column), (1, 0));
        assert!(
            report
                .matches
                .iter()
                .all(|m| m.path == root.join("main.rs"))
        );
    }

    #[test]
    fn regex_multiline_positions_filters_and_cancellation() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("file.txt"), "αx\nβx\nγx").unwrap();
        let options = SearchOptions {
            query: "x\\n.".into(),
            regex: true,
            case_sensitive: true,
            ..Default::default()
        };
        let report = search(dir.path(), vec![], options.clone(), &AtomicBool::new(false)).unwrap();
        assert_eq!(report.total, 2);
        assert_eq!(
            (
                report.matches[1].line,
                report.matches[1].column,
                report.matches[1].range.clone()
            ),
            (1, 1, 4..7)
        );
        let excluded = search(
            dir.path(),
            vec![],
            SearchOptions {
                exclude: "*.txt".into(),
                ..options
            },
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(excluded.total, 0);
        let stopped = search(
            dir.path(),
            vec![],
            SearchOptions {
                query: "x".into(),
                ..Default::default()
            },
            &AtomicBool::new(true),
        )
        .unwrap();
        assert!(stopped.cancelled && stopped.truncated);
    }

    #[test]
    fn result_limit_reports_truncation_and_total() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("many"), "a".repeat(MAX_RESULTS + 1)).unwrap();
        let result = search(
            dir.path(),
            vec![],
            SearchOptions {
                query: "a".into(),
                ..Default::default()
            },
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(result.total, MAX_RESULTS + 1);
        assert_eq!(result.matches.len(), MAX_RESULTS);
        assert!(result.truncated);
    }
}
