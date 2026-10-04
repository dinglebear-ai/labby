//! Lossless rewriting of one credential assignment in an installation dotenv file.

use std::cell::Cell;
use std::io::{self, Read};
use std::rc::Rc;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidDotenv;

/// Replace/remove complete assignments for one key, preserving unrelated bytes.
/// `replacement` is a caller-encoded complete assignment without a line ending.
/// Invalid input is rejected before a caller writes, without retaining secrets
/// in an error. Parsed values are discarded; the original bytes are preserved.
pub fn rewrite_key(
    raw: &str,
    key: &str,
    replacement: Option<&str>,
) -> Result<String, InvalidDotenv> {
    let position = Rc::new(Cell::new(0));
    let reader = PhysicalLineReader {
        bytes: raw.as_bytes(),
        position: Rc::clone(&position),
    };
    let mut output = String::with_capacity(raw.len());
    let mut previous_end = 0;
    let mut inserted = false;
    for entry in dotenvy::from_read_iter(reader) {
        let (name, _) = entry.map_err(|_| InvalidDotenv)?;
        let end = position.get();
        let consumed = &raw[previous_end..end];
        // The iterator skips leading comments/blank lines before yielding an
        // assignment. Retain that prefix independently of the selected key.
        let prefix_len = consumed
            .split_inclusive('\n')
            .take_while(|line| {
                let trimmed = line.trim_start();
                trimmed.is_empty() || trimmed.starts_with('#')
            })
            .map(str::len)
            .sum::<usize>();
        output.push_str(&consumed[..prefix_len]);
        let assignment = &consumed[prefix_len..];
        if crate::helpers::environment_names_equal(&name, key) {
            if !inserted && let Some(replacement) = replacement {
                output.push_str(replacement);
                if assignment.ends_with("\r\n") {
                    output.push_str("\r\n");
                } else if assignment.ends_with('\n') {
                    output.push('\n');
                }
                inserted = true;
            }
        } else {
            output.push_str(assignment);
        }
        previous_end = end;
    }
    output.push_str(&raw[previous_end..]);
    if !inserted && let Some(replacement) = replacement {
        if !output.is_empty() && !output.ends_with('\n') {
            output.push('\n');
        }
        output.push_str(replacement);
        output.push('\n');
    }
    Ok(output)
}

/// Prevent BufReader prefetch from advancing the recorded position into the
/// next physical line. dotenvy's own logical-line parser decides how many
/// physical lines belong to an assignment, including its quote/comment rules.
struct PhysicalLineReader<'a> {
    bytes: &'a [u8],
    position: Rc<Cell<usize>>,
}

impl Read for PhysicalLineReader<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let start = self.position.get();
        let available = (self.bytes.len() - start).min(buffer.len());
        let remaining = &self.bytes[start..start + available];
        let line_len = remaining
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(remaining.len(), |end| end + 1);
        let count = line_len;
        buffer[..count].copy_from_slice(&remaining[..count]);
        self.position.set(start + count);
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logical_dotenv_restore_uses_parser_comment_and_quote_boundaries() {
        let unrelated = "NOTE=value  # '\nTOKEN=inside\n'\n";
        let raw = format!("{unrelated}TOKEN=old\n");
        assert_eq!(
            rewrite_key(&raw, "TOKEN", Some("TOKEN=new")).unwrap(),
            format!("{unrelated}TOKEN=new\n")
        );
    }

    #[test]
    fn logical_dotenv_restore_recognizes_literal_export_key() {
        assert_eq!(rewrite_key("export =old\n", "export", None).unwrap(), "");
        assert_eq!(
            rewrite_key("export =old\n", "export", Some("export=new")).unwrap(),
            "export=new\n"
        );
    }

    #[test]
    fn logical_dotenv_restore_rejects_unfinished_substitution() {
        assert_eq!(
            rewrite_key("NOTE=${UNFINISHED\nTOKEN=old\n", "TOKEN", Some("TOKEN=new")),
            Err(InvalidDotenv)
        );
    }

    #[test]
    fn logical_dotenv_restore_preserves_line_endings_and_trailing_comments() {
        let raw = "# prefix\r\nTOKEN=abc\r\n# trailing ' ";
        assert_eq!(rewrite_key(raw, "TOKEN", Some("TOKEN=abc")).unwrap(), raw);
        assert_eq!(
            rewrite_key("TOKEN=abc", "TOKEN", Some("TOKEN=abc")).unwrap(),
            "TOKEN=abc"
        );
        assert_eq!(
            rewrite_key(raw, "TOKEN", None).unwrap(),
            "# prefix\r\n# trailing ' "
        );
    }

    #[test]
    fn logical_dotenv_restore_handles_large_unicode_lines_without_prefetch() {
        let unrelated = format!("NOTE='{}'\n", "é".repeat(10000));
        let raw = format!("{unrelated}TOKEN=old\nKEEP=last");
        assert_eq!(
            rewrite_key(&raw, "TOKEN", Some("TOKEN=new")).unwrap(),
            format!("{unrelated}TOKEN=new\nKEEP=last")
        );
    }

    #[cfg(windows)]
    #[test]
    fn logical_dotenv_restore_canonicalizes_windows_key_aliases() {
        assert_eq!(
            rewrite_key(
                "Labby_Token=old\nLABBY_TOKEN=duplicate\n",
                "LABBY_TOKEN",
                Some("LABBY_TOKEN=new")
            )
            .unwrap(),
            "LABBY_TOKEN=new\n"
        );
    }
}
