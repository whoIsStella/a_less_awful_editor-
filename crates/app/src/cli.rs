use ale_ui::OpenTarget;
use std::{ffi::OsString, path::PathBuf};

pub const HELP: &str = "A Less Awful Editor

Usage: a-less-awful-editor [--reuse-window] [--text] [--line N [--column N]] PATH...
       a-less-awful-editor [--binary | --folder] PATH
       a-less-awful-editor [--help]

  PATH           Open an existing UTF-8 text file (up to 16 MiB).
  --reuse-window Send text paths to an existing window (Unix).
  --text PATH... Open text files explicitly.
  --line N       Open text at line N (counting from 1).
  --column N     Open text at column N (counting from 1).
  --binary PATH  Inspect a binary (up to 64 MiB); never execute it.
  --folder PATH  Browse one folder without starting a shell or indexing it.
  --help, -h     Show this help without opening a window.

With no arguments, open an empty text buffer.
Relative paths use the caller's working directory. Put -- before a path
beginning with '-'. Symlinks and paths containing '..' are unsupported.
File-loading errors appear in the window; invalid arguments exit with status 2.
";

#[derive(Debug, PartialEq, Eq)]
pub enum Launch {
    Empty,
    Help,
    Open(OpenTarget),
    OpenText {
        paths: Vec<PathBuf>,
        line: usize,
        column: usize,
    },
}

/// Parse OS strings without opening files or initializing the graphical platform.
pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Launch, String> {
    let mut args = args.into_iter();
    let Some(first) = args.next() else {
        return Ok(Launch::Empty);
    };
    if first == "--help" || first == "-h" {
        return if args.next().is_none() {
            Ok(Launch::Help)
        } else {
            Err("--help must be used alone.".into())
        };
    }

    let mut pending = Some(first);
    let mut mode = "text";
    let mut mode_selected = false;
    let mut line = None;
    let mut column = None;
    let mut escaped = false;
    let mut paths = Vec::new();
    while let Some(arg) = pending.take().or_else(|| args.next()) {
        if !escaped && arg == "--" {
            escaped = true;
        } else if !escaped && paths.is_empty() && (arg == "--line" || arg == "--column") {
            let value = args
                .next()
                .and_then(|value| value.to_str()?.parse::<usize>().ok())
                .filter(|value| *value > 0)
                .ok_or_else(|| {
                    format!(
                        "Expected a positive number after {}.",
                        arg.to_string_lossy()
                    )
                })?;
            let slot = if arg == "--line" {
                &mut line
            } else {
                &mut column
            };
            if slot.replace(value).is_some() {
                return Err(format!(
                    "{} may be specified only once.",
                    arg.to_string_lossy()
                ));
            }
        } else if !escaped
            && paths.is_empty()
            && (arg == "--text" || arg == "--binary" || arg == "--folder")
        {
            if mode_selected {
                return Err("Specify only one opening mode.".into());
            }
            mode_selected = true;
            mode = if arg == "--binary" {
                "binary"
            } else if arg == "--folder" {
                "folder"
            } else {
                "text"
            };
        } else {
            if arg.is_empty() {
                return Err("The path must not be empty.".into());
            }
            if !escaped && arg.as_encoded_bytes().starts_with(b"-") {
                return Err(format!(
                    "Unexpected option '{}'. Use --help, or put -- before a path beginning with '-'.",
                    arg.to_string_lossy()
                ));
            }
            paths.push(PathBuf::from(arg));
        }
    }
    if paths.is_empty() {
        return Err("Expected a path. Use --help for usage.".into());
    }
    if paths.len() > 32 {
        return Err("At most 32 text paths can be opened at once.".into());
    }
    if mode != "text" {
        if paths.len() != 1 || line.is_some() || column.is_some() {
            return Err("Binary and folder modes accept one path without a text location.".into());
        }
        let path = paths.pop().unwrap();
        return Ok(Launch::Open(if mode == "binary" {
            OpenTarget::Binary(path)
        } else {
            OpenTarget::Folder(path)
        }));
    }
    if column.is_some() && line.is_none() {
        return Err("--column requires --line.".into());
    }
    if paths.len() == 1 && line.is_none() {
        return Ok(Launch::Open(OpenTarget::Text(paths.pop().unwrap())));
    }
    Ok(Launch::OpenText {
        paths,
        line: line.unwrap_or(1),
        column: column.unwrap_or(1),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Result<Launch, String> {
        parse(values.iter().map(OsString::from))
    }

    #[test]
    fn empty_and_help_need_no_target() {
        assert_eq!(args(&[]), Ok(Launch::Empty));
        assert_eq!(args(&["--help"]), Ok(Launch::Help));
        assert_eq!(args(&["-h"]), Ok(Launch::Help));
    }

    #[test]
    fn explicit_modes_preserve_paths() {
        assert_eq!(
            args(&["src/main.c"]),
            Ok(Launch::Open(OpenTarget::Text("src/main.c".into())))
        );
        assert_eq!(
            args(&["--text", "source λ.c"]),
            Ok(Launch::Open(OpenTarget::Text("source λ.c".into())))
        );
        assert_eq!(
            args(&["--binary", "/tmp/firmware.elf"]),
            Ok(Launch::Open(OpenTarget::Binary("/tmp/firmware.elf".into())))
        );
        assert_eq!(
            args(&["--folder", "."]),
            Ok(Launch::Open(OpenTarget::Folder(".".into())))
        );
        assert_eq!(
            args(&["--", "-source.c"]),
            Ok(Launch::Open(OpenTarget::Text("-source.c".into())))
        );
        assert_eq!(
            args(&["--binary", "--", "-image"]),
            Ok(Launch::Open(OpenTarget::Binary("-image".into())))
        );
    }

    #[test]
    fn invalid_or_ambiguous_invocations_are_errors() {
        for values in [
            vec!["--binary"],
            vec!["--folder"],
            vec!["--text"],
            vec!["--"],
            vec!["--binary", "--"],
            vec![""],
            vec!["--unknown"],
            vec!["--text", "--binary", "image"],
            vec!["--binary", "--folder"],
            vec!["--help", "one.c"],
        ] {
            assert!(args(&values).is_err(), "{values:?}");
        }
    }

    #[test]
    fn multiple_text_paths_and_location() {
        assert_eq!(
            args(&["--line", "2", "--column", "3", "one.c", "two λ.c"]),
            Ok(Launch::OpenText {
                paths: vec!["one.c".into(), "two λ.c".into()],
                line: 2,
                column: 3,
            })
        );
        for invalid in [
            &["--column", "2", "one.c"][..],
            &["--line", "0", "one.c"],
            &["--binary", "a", "b"],
        ] {
            assert!(args(invalid).is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn non_unicode_paths_are_not_replaced() {
        use std::os::unix::ffi::OsStringExt;
        let path = OsString::from_vec(b"file-\xff.c".to_vec());
        assert_eq!(
            parse([path.clone()]),
            Ok(Launch::Open(OpenTarget::Text(path.into())))
        );
    }
}
