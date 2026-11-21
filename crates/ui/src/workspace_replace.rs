//! Construct an immutable, reviewable replacement from a search snapshot.
use crate::workspace_search::{SearchMatch, SearchOptions};
use regex::Captures;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, ops::Range, path::PathBuf};

#[derive(Clone, Debug)]
pub(crate) struct Edit {
    pub range: Range<usize>,
    pub replacement: String,
}

#[derive(Clone, Debug)]
pub(crate) struct FilePlan {
    pub path: PathBuf,
    pub before: String,
    pub after: String,
    pub edits: Vec<Edit>,
}

pub(crate) fn plan(
    text: &str,
    selected: &[SearchMatch],
    options: &SearchOptions,
    replacement: &str,
) -> Result<FilePlan, String> {
    let first = selected.first().ok_or("Select at least one match.")?;
    let digest: [u8; 32] = Sha256::digest(text.as_bytes()).into();
    let mut wanted = BTreeMap::new();
    for item in selected {
        if item.path != first.path || item.digest != digest {
            return Err("The file changed since the search. Search again.".into());
        }
        if wanted
            .insert((item.range.start, item.range.end), ())
            .is_some()
        {
            return Err("A match was selected more than once.".into());
        }
    }
    const LIMIT: usize = 16 * 1024 * 1024;
    if text.len() > LIMIT || replacement.len() > LIMIT {
        return Err("The replacement would exceed the 16 MiB text file limit.".into());
    }
    let matcher = crate::workspace_search::compile_matcher(options)?;
    let mut after = String::with_capacity(text.len());
    let mut edits = Vec::with_capacity(selected.len());
    let mut previous_byte = 0;
    let mut previous_scalar = 0;
    let mut copied_until = 0;
    for captures in matcher.captures_iter(text) {
        let found = captures.get(0).expect("regex match has group zero");
        let start = previous_scalar + text[previous_byte..found.start()].chars().count();
        let end = start + found.as_str().chars().count();
        previous_byte = found.end();
        previous_scalar = end;
        if wanted.remove(&(start, end)).is_none() {
            continue;
        }
        append_bounded(&mut after, &text[copied_until..found.start()], LIMIT)?;
        let mut value = String::new();
        if options.regex {
            expand_bounded(&captures, replacement, &mut value, LIMIT - after.len())?;
        } else {
            append_bounded(&mut value, replacement, LIMIT - after.len())?;
        }
        append_bounded(&mut after, &value, LIMIT)?;
        copied_until = found.end();
        edits.push(Edit {
            range: start..end,
            replacement: value,
        });
    }
    if !wanted.is_empty() {
        return Err("A selected match no longer corresponds to this search. Search again.".into());
    }
    append_bounded(&mut after, &text[copied_until..], LIMIT)?;
    Ok(FilePlan {
        path: first.path.clone(),
        before: text.into(),
        after,
        edits,
    })
}

fn append_bounded(out: &mut String, value: &str, limit: usize) -> Result<(), String> {
    if value.len() > limit.saturating_sub(out.len()) {
        return Err("The replacement would exceed the 16 MiB text file limit.".into());
    }
    out.push_str(value);
    Ok(())
}

// Follow regex replacement syntax for $name, ${name}, $1 and $$ while checking
// each fragment before allocation.
fn expand_bounded(
    captures: &Captures<'_>,
    mut template: &str,
    out: &mut String,
    limit: usize,
) -> Result<(), String> {
    while let Some(index) = template.find('$') {
        append_bounded(out, &template[..index], limit)?;
        template = &template[index + 1..];
        if let Some(rest) = template.strip_prefix('$') {
            append_bounded(out, "$", limit)?;
            template = rest;
            continue;
        }
        let (name, rest) = if let Some(braced) = template.strip_prefix('{') {
            match braced.find('}') {
                Some(end) => (&braced[..end], &braced[end + 1..]),
                None => {
                    append_bounded(out, "$", limit)?;
                    continue;
                }
            }
        } else {
            let end = template
                .bytes()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == b'_')
                .count();
            if end == 0 {
                append_bounded(out, "$", limit)?;
                continue;
            }
            (&template[..end], &template[end..])
        };
        let capture = match name.parse::<usize>() {
            Ok(index) => captures.get(index),
            Err(_) => captures.name(name),
        };
        if let Some(capture) = capture {
            append_bounded(out, capture.as_str(), limit)?;
        }
        template = rest;
    }
    append_bounded(out, template, limit)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(text: &str, start: usize, end: usize) -> SearchMatch {
        SearchMatch {
            path: "test.c".into(),
            line: 0,
            column: start,
            range: start..end,
            preview: String::new(),
            digest: Sha256::digest(text.as_bytes()).into(),
        }
    }

    #[test]
    fn preview_respects_selection_unicode_and_literal_dollars() {
        let text = "λ foo foo";
        let options = SearchOptions {
            query: "foo".into(),
            case_sensitive: true,
            ..Default::default()
        };
        let result = plan(text, &[hit(text, 6, 9)], &options, "$1").unwrap();
        assert_eq!(result.before, text);
        assert_eq!(result.after, "λ foo $1");
        assert_eq!(result.edits[0].range, 6..9);
        assert_eq!(result.edits[0].replacement, "$1");
        assert!(plan("λ bar foo", &[hit(text, 6, 9)], &options, "x").is_err());
    }

    #[test]
    fn expands_regex_captures_and_rejects_stale_search_options() {
        let text = "λ item42 item9";
        let options = SearchOptions {
            query: r"item(\d+)".into(),
            regex: true,
            ..Default::default()
        };
        let result = plan(
            text,
            &[hit(text, 2, 8), hit(text, 9, 14)],
            &options,
            "entry$1",
        )
        .unwrap();
        assert_eq!(result.after, "λ entry42 entry9");
        assert_eq!(result.edits.len(), 2);
        assert!(plan(text, &[hit(text, 2, 7)], &options, "x").is_err());
        assert!(plan(text, &[hit(text, 2, 8), hit(text, 2, 8)], &options, "x").is_err());
    }

    #[test]
    fn bounded_expansion_matches_regex_capture_syntax() {
        let matcher = regex::Regex::new(r"(?P<name>item)([0-9]+)").unwrap();
        let captures = matcher.captures("item42").unwrap();
        for template in [
            "$name-$2",
            "${1}x",
            "$1x",
            "$$",
            "$",
            "${missing}",
            "${name",
            "$_",
        ] {
            let mut expected = String::new();
            captures.expand(template, &mut expected);
            let mut actual = String::new();
            expand_bounded(&captures, template, &mut actual, 100).unwrap();
            assert_eq!(actual, expected, "{template}");
        }
    }

    #[test]
    fn repeated_capture_expansion_is_bounded_before_allocation() {
        let text = "x".repeat(9 * 1024 * 1024);
        let options = SearchOptions {
            query: "(x+)".into(),
            regex: true,
            ..Default::default()
        };
        assert!(plan(&text, &[hit(&text, 0, text.len())], &options, "$1$1").is_err());
    }

    #[test]
    fn zero_width_matches_can_be_replaced() {
        let text = "αβ";
        let options = SearchOptions {
            query: r"\b".into(),
            regex: true,
            ..Default::default()
        };
        let result = plan(text, &[hit(text, 0, 0), hit(text, 2, 2)], &options, "_").unwrap();
        assert_eq!(result.after, "_αβ_");
    }
}
