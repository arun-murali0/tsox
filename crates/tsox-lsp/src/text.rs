//! Positions and edits over document text.
//!
//! LSP positions are zero-based lines and UTF-16 code unit columns. The checker works in
//! byte offsets into UTF-8 text, so every request crosses this boundary. A line ends at
//! `\n`, `\r\n` or a lone `\r`, as the LSP specification says; the terminator is not part
//! of the line's columns.

use tower_lsp_server::ls_types::{Position, Range, TextDocumentContentChangeEvent};

/// Where each line of one text starts, as byte offsets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineMap {
    starts: Vec<u32>,
}

impl LineMap {
    pub fn new(text: &str) -> Self {
        let bytes = text.as_bytes();
        let mut starts = vec![0u32];
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'\n' => starts.push(i as u32 + 1),
                b'\r' => {
                    if bytes.get(i + 1) == Some(&b'\n') {
                        i += 1;
                    }
                    starts.push(i as u32 + 1);
                }
                _ => {}
            }
            i += 1;
        }
        Self { starts }
    }

    pub fn line_count(&self) -> usize {
        self.starts.len()
    }

    /// The end of a line's content: the start of the next line less its terminator, or the
    /// end of the text for the last line.
    fn content_end(&self, text: &str, line: usize) -> usize {
        let Some(&next) = self.starts.get(line + 1) else {
            return text.len();
        };
        let bytes = text.as_bytes();
        let start = self.starts[line] as usize;
        let mut end = next as usize;
        if end > start && bytes[end - 1] == b'\n' {
            end -= 1;
        }
        if end > start && bytes[end - 1] == b'\r' {
            end -= 1;
        }
        end
    }

    /// The position of a byte offset. An offset past the end gives the end of the text, and
    /// one inside a character gives the start of that character.
    pub fn position(&self, text: &str, offset: usize) -> Position {
        let mut offset = offset.min(text.len());
        while !text.is_char_boundary(offset) {
            offset -= 1;
        }
        let line = self.starts.partition_point(|&s| s as usize <= offset) - 1;
        let start = self.starts[line] as usize;
        // An offset inside a terminator counts as the end of the line's content.
        let end = self.content_end(text, line);
        let upto = offset.min(end);
        let character = text[start..upto].encode_utf16().count() as u32;
        Position {
            line: line as u32,
            character,
        }
    }

    pub fn range(&self, text: &str, start: usize, end: usize) -> Range {
        Range {
            start: self.position(text, start),
            end: self.position(text, end),
        }
    }

    /// The byte offset of a position. A line past the end gives the end of the text; a column
    /// past the end of its line gives the end of the line, and one inside a surrogate pair
    /// gives the start of that character.
    pub fn offset(&self, text: &str, position: Position) -> usize {
        let line = position.line as usize;
        if line >= self.starts.len() {
            return text.len();
        }
        let start = self.starts[line] as usize;
        let end = self.content_end(text, line);
        let mut units = 0u32;
        for (index, ch) in text[start..end].char_indices() {
            if units >= position.character {
                return start + index;
            }
            units += ch.len_utf16() as u32;
            if units > position.character {
                return start + index;
            }
        }
        end
    }
}

/// One open document: its text and the version the editor last gave it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Document {
    pub version: i32,
    pub text: String,
}

impl Document {
    pub fn new(version: i32, text: String) -> Self {
        Self { version, text }
    }

    /// Applies the changes of one didChange, in order, and takes the new version. A change
    /// with no range replaces the whole text. Each ranged change is read against the text
    /// the changes before it left, as the specification requires.
    pub fn apply_changes(&mut self, version: i32, changes: Vec<TextDocumentContentChangeEvent>) {
        for change in changes {
            match change.range {
                None => self.text = change.text,
                Some(range) => {
                    let map = LineMap::new(&self.text);
                    let start = map.offset(&self.text, range.start);
                    let end = map.offset(&self.text, range.end).max(start);
                    self.text.replace_range(start..end, &change.text);
                }
            }
        }
        self.version = version;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos(line: u32, character: u32) -> Position {
        Position { line, character }
    }

    fn ranged(sl: u32, sc: u32, el: u32, ec: u32, text: &str) -> TextDocumentContentChangeEvent {
        TextDocumentContentChangeEvent {
            range: Some(Range {
                start: pos(sl, sc),
                end: pos(el, ec),
            }),
            range_length: None,
            text: text.to_owned(),
        }
    }

    #[test]
    fn positions_are_zero_based() {
        let text = "let a = 1;\nlet b = 2;";
        let map = LineMap::new(text);
        assert_eq!(map.position(text, 0), pos(0, 0));
        assert_eq!(map.position(text, text.find('b').unwrap()), pos(1, 4));
    }

    #[test]
    fn columns_count_utf16_units() {
        let text = "const s = \"\u{1F600}\"; const after = 1;";
        let map = LineMap::new(text);
        let offset = text.find("after").unwrap();
        // The emoji is one char in Rust, two UTF-16 units, four UTF-8 bytes.
        assert_eq!(map.position(text, offset).character, offset as u32 - 2);
        assert_eq!(map.offset(text, map.position(text, offset)), offset);
    }

    #[test]
    fn crlf_and_lone_cr_end_lines() {
        let text = "a\r\nbb\rccc\nd";
        let map = LineMap::new(text);
        assert_eq!(map.line_count(), 4);
        assert_eq!(map.position(text, text.find("bb").unwrap()), pos(1, 0));
        assert_eq!(map.position(text, text.find("ccc").unwrap()), pos(2, 0));
        assert_eq!(map.position(text, text.find('d').unwrap()), pos(3, 0));
        // The offset of the `\n` of a `\r\n` is still the end of line 0.
        assert_eq!(map.position(text, 2), pos(0, 1));
    }

    #[test]
    fn offset_clamps_column_and_line() {
        let text = "ab\ncd";
        let map = LineMap::new(text);
        assert_eq!(map.offset(text, pos(0, 99)), 2);
        assert_eq!(map.offset(text, pos(1, 99)), 5);
        assert_eq!(map.offset(text, pos(9, 0)), text.len());
    }

    #[test]
    fn offset_inside_a_surrogate_pair_is_the_start_of_the_character() {
        let text = "\u{1F600}x";
        let map = LineMap::new(text);
        assert_eq!(map.offset(text, pos(0, 1)), 0);
        assert_eq!(map.offset(text, pos(0, 2)), 4);
    }

    #[test]
    fn position_inside_a_character_rounds_down() {
        let text = "\u{e9}x";
        let map = LineMap::new(text);
        assert_eq!(map.position(text, 1), pos(0, 0));
        assert_eq!(map.position(text, 2), pos(0, 1));
    }

    #[test]
    fn position_round_trips_for_every_boundary() {
        let text = "a\u{1F600}b\r\n\u{e9}\nlast";
        let map = LineMap::new(text);
        for offset in (0..=text.len()).filter(|o| text.is_char_boundary(*o)) {
            let back = map.offset(text, map.position(text, offset));
            // Offsets inside a terminator come back at the end of the line's content.
            if text.as_bytes().get(offset).is_some_and(|b| *b == b'\n')
                && offset > 0
                && text.as_bytes()[offset - 1] == b'\r'
            {
                assert_eq!(back, offset - 1);
            } else {
                assert_eq!(back, offset, "offset {offset}");
            }
        }
    }

    #[test]
    fn a_change_without_a_range_replaces_the_text() {
        let mut doc = Document::new(1, "old".to_owned());
        doc.apply_changes(
            2,
            vec![TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: "new".to_owned(),
            }],
        );
        assert_eq!(doc, Document::new(2, "new".to_owned()));
    }

    #[test]
    fn ranged_changes_apply_in_order_against_the_text_before_them() {
        let mut doc = Document::new(1, "let a = 1;\nlet b = 2;".to_owned());
        doc.apply_changes(
            2,
            vec![
                ranged(0, 8, 0, 9, "100"),
                // Line 1 moved nothing; this column is read after the first edit.
                ranged(1, 8, 1, 9, "200"),
                ranged(0, 0, 0, 0, "// hi\n"),
            ],
        );
        assert_eq!(doc.text, "// hi\nlet a = 100;\nlet b = 200;");
        assert_eq!(doc.version, 2);
    }

    #[test]
    fn a_reversed_range_does_not_panic() {
        let mut doc = Document::new(1, "abcdef".to_owned());
        doc.apply_changes(2, vec![ranged(0, 4, 0, 1, "X")]);
        assert_eq!(doc.text, "abcdXef");
    }
}
