use lsp_types::{Position, Url};
use std::fs;

/// Extract the identifier word under the cursor position.
pub fn word_at_position(uri: &Url, position: Position) -> Option<String> {
    let content = read_uri_content(uri)?;
    let lines: Vec<&str> = content.lines().collect();
    let line_content = lines.get(position.line as usize)?;
    let char_idx = position.character as usize;

    if char_idx >= line_content.len() {
        return None;
    }

    let ident_chars = |c: char| c.is_alphanumeric() || c == '_';
    let chars: Vec<char> = line_content.chars().collect();
    if char_idx >= chars.len() || !ident_chars(chars[char_idx]) {
        return None;
    }

    let mut start = char_idx;
    while start > 0 && ident_chars(chars[start - 1]) {
        start -= 1;
    }

    let mut end = char_idx;
    while end < chars.len() && ident_chars(chars[end]) {
        end += 1;
    }

    if start < end {
        Some(chars[start..end].iter().collect())
    } else {
        None
    }
}

fn read_uri_content(uri: &Url) -> Option<String> {
    let path = uri.to_file_path().ok()?;
    fs::read_to_string(path).ok()
}
