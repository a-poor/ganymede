//! Small, language-independent pairing rules for direct keyboard input.
use super::CodeCell;

fn closing(open: char) -> Option<char> {
    match open {
        '(' => Some(')'),
        '[' => Some(']'),
        '{' => Some('}'),
        '\'' | '"' => Some(open),
        _ => None,
    }
}

fn escaped(prefix: &str) -> bool {
    prefix.chars().rev().take_while(|c| *c == '\\').count() % 2 == 1
}

impl CodeCell {
    /// Returns false when ordinary insertion should handle the input.
    pub(super) fn insert_pair(&mut self, text: &str) -> bool {
        let mut chars = text.chars();
        let Some(ch) = chars.next() else { return false };
        if chars.next().is_some() {
            return false;
        }
        let range = self.buffer.selection();
        let prefix = &self.buffer.text[..range.start];
        let next = self.buffer.text[range.end..].chars().next();
        let quote = matches!(ch, '\'' | '"');
        if escaped(prefix) {
            return false;
        }
        if range.is_empty() && matches!(ch, ')' | ']' | '}' | '\'' | '"') && next == Some(ch) {
            self.buffer.head += ch.len_utf8();
            self.buffer.anchor = self.buffer.head;
            return true;
        }
        let Some(close) = closing(ch) else {
            return false;
        };
        if range.is_empty() {
            // Quotes beside words are commonly apostrophes or string endings.
            if quote
                && prefix
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_alphanumeric() || c == '_')
            {
                return false;
            }
            if next.is_some_and(|c| {
                !c.is_whitespace() && !matches!(c, ')' | ']' | '}' | ',' | ':' | ';')
            }) {
                return false;
            }
        }
        let reversed = self.buffer.head < self.buffer.anchor;
        let replacement = format!("{ch}{}{close}", &self.buffer.text[range.clone()]);
        self.edit(range.clone(), &replacement);
        let inner = range.start + ch.len_utf8()..range.end + ch.len_utf8();
        self.buffer.anchor = if reversed { inner.end } else { inner.start };
        self.buffer.head = if reversed { inner.start } else { inner.end };
        true
    }

    pub(super) fn delete_pair(&mut self) -> bool {
        let caret = self.buffer.head;
        let prefix = &self.buffer.text[..caret];
        let Some(open) = prefix.chars().next_back() else {
            return false;
        };
        let Some(close) = closing(open) else {
            return false;
        };
        let start = caret - open.len_utf8();
        if escaped(&self.buffer.text[..start]) || !self.buffer.text[caret..].starts_with(close) {
            return false;
        }
        self.edit(start..caret + close.len_utf8(), "");
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AppContext, EntityInputHandler, TestAppContext};

    #[gpui::test]
    fn typing_pairs_wraps_unicode_and_undo_restores_selection(cx: &mut TestAppContext) {
        cx.update(|cx| {
            cx.set_global(
                crate::theme::ThemeSpec::light().resolve(&crate::theme::Derivation::default()),
            )
        });
        let cx = cx.add_empty_window();
        cx.update(|window, cx| {
            cx.new(|cx| {
                let mut cell = CodeCell::new_document(window, cx);
                cell.replace_text_in_range(None, "(", window, cx);
                assert_eq!(cell.buffer.text, "()");
                assert_eq!(cell.buffer.head, 1);
                cell.replace_text_in_range(None, ")", window, cx);
                assert_eq!(cell.buffer.text, "()");
                assert_eq!(cell.buffer.head, 2);
                assert_eq!(cell.undo.len(), 1);
                cell.buffer.text = "🌙 café".into();
                cell.buffer.anchor = cell.buffer.text.len();
                cell.buffer.head = 0;
                cell.replace_text_in_range(None, "[", window, cx);
                assert_eq!(cell.buffer.text, "[🌙 café]");
                assert_eq!(&cell.buffer.text[cell.buffer.selection()], "🌙 café");
                assert!(cell.buffer.head < cell.buffer.anchor);
                cell.buffer = cell.undo.pop().unwrap();
                assert_eq!(cell.buffer.text, "🌙 café");
                assert_eq!(cell.buffer.head, 0);
                cell
            })
        });
    }

    #[gpui::test]
    fn pairing_respects_literal_input_context_and_preferences(cx: &mut TestAppContext) {
        cx.update(|cx| {
            cx.set_global(
                crate::theme::ThemeSpec::light().resolve(&crate::theme::Derivation::default()),
            )
        });
        let cx = cx.add_empty_window();
        cx.update(|window, cx| {
            cx.new(|cx| {
                let mut cell = CodeCell::new_document(window, cx);
                for (source, input, expected) in [
                    ("can", "'", "can'"),
                    ("\\", "\"", "\\\""),
                    ("", "()", "()"),
                    ("", "\"", "\"\""),
                ] {
                    cell.buffer.text = source.into();
                    cell.buffer.head = source.len();
                    cell.buffer.anchor = source.len();
                    cell.replace_text_in_range(None, input, window, cx);
                    assert_eq!(cell.buffer.text, expected);
                }
                cell.buffer.text.clear();
                cell.buffer.head = 0;
                cell.buffer.anchor = 0;
                cell.replace_text_in_range(Some(0..0), "(", window, cx);
                assert_eq!(cell.buffer.text, "("); // explicit replacement / paste
                cell.buffer.text.clear();
                cell.buffer.head = 0;
                cell.buffer.anchor = 0;
                cell.replace_and_mark_text_in_range(None, "[", None, window, cx);
                cell.replace_text_in_range(None, "[", window, cx);
                assert_eq!(cell.buffer.text, "["); // IME commit
                cx.set_global(crate::preferences::EditorPreferences {
                    auto_close_brackets: false,
                    ..Default::default()
                });
                cell.replace_text_in_range(None, "(", window, cx);
                assert_eq!(cell.buffer.text, "[(");
                cell
            })
        });
    }

    #[gpui::test]
    fn empty_pair_deletion_is_one_undoable_edit(cx: &mut TestAppContext) {
        cx.update(|cx| {
            cx.set_global(
                crate::theme::ThemeSpec::light().resolve(&crate::theme::Derivation::default()),
            )
        });
        let cx = cx.add_empty_window();
        cx.update(|window, cx| {
            cx.new(|cx| {
                let mut cell = CodeCell::new_document(window, cx);
                for pair in ["()", "[]", "{}", "\"\"", "''"] {
                    cell.buffer.text = pair.into();
                    cell.buffer.head = 1;
                    cell.buffer.anchor = 1;
                    assert!(cell.delete_pair());
                    assert_eq!(cell.buffer.text, "");
                    cell.buffer = cell.undo.pop().unwrap();
                    assert_eq!(cell.buffer.text, pair);
                    assert_eq!(cell.buffer.head, 1);
                }
                cell
            })
        });
    }
}
