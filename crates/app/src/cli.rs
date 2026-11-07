use ale_ui::OpenTarget;
use std::{ffi::OsString, path::PathBuf};

pub const HELP: &str = "A Less Awful Editor

Usage: a-less-awful-editor [--text | --binary | --folder] PATH
       a-less-awful-editor [--help]

  PATH           Open an existing UTF-8 text file (up to 16 MiB).
  --text PATH    Open a text file explicitly.
  --binary PATH  Inspect a binary (up to 64 MiB); never execute it.
  --folder PATH  Browse one folder without starting a shell or indexing it.
  --help, -h     Show this help without opening a window.

With no arguments, open an empty text buffer. One target per launch.
Relative paths use the caller's working directory. Put -- before a path
beginning with '-'. Symlinks and paths containing '..' are unsupported.
File-loading errors appear in the window; invalid arguments exit with status 2.
";

#[derive(Debug, PartialEq, Eq)]
pub enum Launch {
    Empty,
    Help,
    Open(OpenTarget),
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
    let (mode, mut path) = if first == "--text" {
        (OpenTarget::Text as fn(PathBuf) -> OpenTarget, args.next())
    } else if first == "--binary" {
        (OpenTarget::Binary as fn(PathBuf) -> OpenTarget, args.next())
    } else if first == "--folder" {
        (OpenTarget::Folder as fn(PathBuf) -> OpenTarget, args.next())
    } else {
        (OpenTarget::Text as fn(PathBuf) -> OpenTarget, Some(first))
    };
    let escaped = path.as_deref() == Some(std::ffi::OsStr::new("--"));
    if escaped {
        path = args.next();
    }
    let path = path.ok_or("Expected a path. Use --help for usage.")?;
    if path.is_empty() {
        return Err("The path must not be empty.".into());
    }
    if !escaped && path.as_encoded_bytes().starts_with(b"-") {
        return Err(format!(
            "Unexpected option '{}'. Use --help, or put -- before a path beginning with '-'.",
            path.to_string_lossy()
        ));
    }
    if args.next().is_some() {
        return Err("Open one target per launch. Use --help for usage.".into());
    }
    Ok(Launch::Open(mode(path.into())))
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
            vec!["one.c", "two.c"],
            vec!["--text", "--binary", "image"],
            vec!["--binary", "--folder"],
            vec!["--help", "one.c"],
        ] {
            assert!(args(&values).is_err(), "{values:?}");
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
