//! Contiguous cell selection and atomic structural operations.
use super::*;
use ganymede_core::notebook::NotebookDocument;
use serde_json::{Value, json};
use std::ops::Range;

impl Notebook {
    pub(super) fn selected_range(&self) -> Range<usize> {
        self.selection_anchor.min(self.selected)..self.selection_anchor.max(self.selected) + 1
    }

    fn paint_selection(&self, cx: &mut Context<Self>) {
        let range = self.selected_range();
        for (index, cell) in self.cells.iter().enumerate() {
            cell.update(cx, |cell, cx| {
                cell.set_selection(
                    index == self.selected,
                    range.len() > 1 && range.contains(&index),
                    cx,
                );
            });
        }
    }

    pub(super) fn extend_selection(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selected = index.min(self.cells.len() - 1);
        self.type_menu_open = false;
        self.paint_selection(cx);
        self.cells[self.selected]
            .read(cx)
            .list_focus_handle()
            .focus(window, cx);
        self.reveal_cell = Some(self.selected);
        cx.notify();
    }

    pub(super) fn navigate_cells(
        &mut self,
        down: bool,
        extend: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = if down {
            (self.selected + 1).min(self.cells.len() - 1)
        } else {
            self.selected.saturating_sub(1)
        };
        if !extend {
            self.selection_anchor = target;
        }
        self.extend_selection(target, window, cx);
    }

    pub(super) fn selection_running(&self) -> bool {
        self.file_busy
            || self
                .runs
                .active
                .is_some_and(|index| self.selected_range().contains(&index))
    }

    pub(super) fn delete_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.selection_running() {
            return;
        }
        let range = self.selected_range();
        if !self.record_structure_range(range.clone(), cx) {
            return;
        }
        for index in range.clone().rev() {
            self.cells[index].update(cx, |cell, cx| cell.clear_execution(cx));
            self.runs.remove(index);
            self.cells.remove(index);
        }
        if self.cells.is_empty() {
            self.append_cell(window, cx);
        }
        self.select(range.start.min(self.cells.len() - 1), cx);
        self.cells[self.selected]
            .read(cx)
            .list_focus_handle()
            .focus(window, cx);
        self.reveal_cell = Some(self.selected);
        self.start_next_run(cx);
    }

    pub(super) fn move_selection(&mut self, down: bool, cx: &mut Context<Self>) {
        let range = self.selected_range();
        if self.file_busy || (down && range.end == self.cells.len()) || (!down && range.start == 0)
        {
            return;
        }
        if !self.record_structure(None, cx) {
            return;
        }
        if down {
            self.cells[range.start..=range.end].rotate_right(1);
        } else {
            self.cells[range.start - 1..range.end].rotate_left(1);
        }
        self.runs.remap(|index| moved_index(index, &range, down));
        self.selected = moved_index(self.selected, &range, down);
        self.selection_anchor = moved_index(self.selection_anchor, &range, down);
        self.paint_selection(cx);
        self.sync(cx);
        self.reveal_cell = Some(self.selected);
        cx.notify();
    }

    fn selected_payload(&mut self, cx: &mut Context<Self>) -> Option<Vec<Value>> {
        if !self.sync_history(cx) {
            return None;
        }
        let document = self.session.document().to_json();
        Some(document["cells"].as_array()?[self.selected_range()].to_vec())
    }

    pub(super) fn copy_selection(&mut self, cx: &mut Context<Self>) {
        let Some(cells) = self.selected_payload(cx) else {
            return;
        };
        let text = self.cells[self.selected_range()]
            .iter()
            .map(|cell| cell.read(cx).text())
            .collect::<Vec<_>>()
            .join("\n\n");
        let metadata = json!({"ganymede_cells": 1, "cells": cells}).to_string();
        cx.write_to_clipboard(gpui::ClipboardItem::new_string_with_metadata(
            text, metadata,
        ));
    }

    pub(super) fn duplicate_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(cells) = self.selected_payload(cx) else {
            return;
        };
        self.insert_payload(cells, window, cx);
    }

    pub(super) fn paste_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = cx.read_from_clipboard() else {
            return;
        };
        let cells = item
            .metadata()
            .and_then(|metadata| serde_json::from_str::<Value>(metadata).ok())
            .filter(|value| value["ganymede_cells"] == 1)
            .and_then(|value| value["cells"].as_array().cloned());
        let cells = cells.unwrap_or_else(|| {
            vec![json!({
                "cell_type": "code", "source": item.text().unwrap_or_default(),
                "metadata": {}, "outputs": [], "execution_count": null
            })]
        });
        self.insert_payload(cells, window, cx);
    }

    fn insert_payload(
        &mut self,
        mut cells: Vec<Value>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.file_busy || cells.is_empty() {
            return;
        }
        // Pasted cells always receive fresh identities, including across notebooks.
        for cell in &mut cells {
            let Some(object) = cell.as_object_mut() else {
                return;
            };
            object.insert("id".into(), uuid::Uuid::new_v4().to_string().into());
        }
        let Ok(document) = NotebookDocument::from_json(json!({
            "nbformat": 4, "nbformat_minor": 5, "metadata": {}, "cells": cells
        })) else {
            return;
        };
        if !self.record_structure(None, cx) {
            return;
        }
        let target = self.selected_range().end;
        let old_len = self.cells.len();
        let mut bindings: Vec<_> = self
            .cells
            .iter()
            .zip(self.session.document().cells())
            .map(|(editor, cell)| (editor.entity_id().as_u64(), cell.id().clone()))
            .collect();
        let mut inserted = vec![];
        for cell in document.cells() {
            self.append_cell(window, cx);
            let editor = self.cells.pop().unwrap();
            editor.update(cx, |editor, cx| editor.load(cell, cx));
            inserted.push(editor);
        }
        let count = inserted.len();
        bindings.splice(
            target..target,
            inserted
                .iter()
                .zip(document.cells())
                .map(|(editor, cell)| (editor.entity_id().as_u64(), cell.id().clone())),
        );
        if let Err(error) = self.session.restore_structure(&bindings, document.cells()) {
            self.save_error = Some(error);
            self.undo_structure.pop();
            return;
        }
        debug_assert_eq!(self.cells.len(), old_len);
        self.cells.splice(target..target, inserted);
        self.runs.remap(|i| if i >= target { i + count } else { i });
        self.selection_anchor = target;
        self.extend_selection(target + count - 1, window, cx);
        self.sync(cx);
    }
}

fn moved_index(index: usize, range: &Range<usize>, down: bool) -> usize {
    if range.contains(&index) {
        if down { index + 1 } else { index - 1 }
    } else if down && index == range.end {
        range.start
    } else if !down && index + 1 == range.start {
        range.end - 1
    } else {
        index
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ganymede_core::{jupyter::ServerConfig, notebook::NotebookController};

    fn fixture(
        cx: &mut gpui::VisualTestContext,
        runtime: &tokio::runtime::Runtime,
    ) -> Entity<Notebook> {
        cx.update(|window, cx| {
            cx.set_global(crate::theme::ThemeSpec::light().resolve(&crate::theme::Derivation::default()));
            cx.set_global(crate::autosave::AutosavePreference(false));
            crate::code_cell::init(cx);
            super::super::init(cx);
            let document = NotebookDocument::from_json(json!({"nbformat":4,"nbformat_minor":5,"metadata":{},"cells":[
                {"id":"a","cell_type":"code","source":"a = 1","metadata":{"tags":["keep"]},"execution_count":7,"outputs":[{"output_type":"stream","name":"stdout","text":"saved\n"}]},
                {"id":"b","cell_type":"markdown","source":"# Notes","metadata":{},"attachments":{"image":{"image/png":"abc"}}},
                {"id":"c","cell_type":"raw","source":"raw text","metadata":{},"vendor":true},
                {"id":"d","cell_type":"code","source":"d = 4","metadata":{},"execution_count":null,"outputs":[]}
            ]})).unwrap();
            let config = ServerConfig::new("http://127.0.0.1:9").unwrap();
            let controller = NotebookController::new(config.clone(), "test.ipynb", document).unwrap();
            let bridge = crate::execution::Runtime { tunnel:None, handle:runtime.handle().clone(), config:Ok(config), kernel:"python3".into() };
            let session = crate::execution::Session::from_controller(controller, &bridge);
            let toasts = cx.new(|cx| crate::toast::ToastHost::new(window, cx));
            cx.new(|cx| Notebook::new(session, toasts, window, cx))
        })
    }

    #[gpui::test]
    fn range_edits_preserve_payloads_and_restore_selection_in_one_undo(
        cx: &mut gpui::TestAppContext,
    ) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let cx = cx.add_empty_window();
        let book = fixture(cx, &runtime);
        cx.run_until_parked();
        cx.update(|window, cx| {
            book.update(cx, |book, cx| {
                let original = book.session.document().to_json();
                book.select(2, cx);
                book.extend_selection(0, window, cx);
                assert_eq!(book.selected_range(), 0..3);
                book.copy_selection(cx);
                assert!(
                    cx.read_from_clipboard()
                        .unwrap()
                        .text()
                        .unwrap()
                        .contains("# Notes")
                );
                book.paste_selection(window, cx);
                assert_eq!(book.cells.len(), 7);
                assert_eq!(book.selected_range(), 3..6);
                let pasted = book.session.document().to_json();
                for i in 0..3 {
                    let mut expected = original["cells"][i].clone();
                    expected["id"] = pasted["cells"][i + 3]["id"].clone();
                    assert_eq!(pasted["cells"][i + 3], expected);
                    assert_ne!(pasted["cells"][i + 3]["id"], original["cells"][i]["id"]);
                }
                book.restore_history(false, window, cx);
                assert_eq!(book.session.document().to_json(), original);
                assert_eq!(book.selected_range(), 0..3);
                assert_eq!(book.selected, 0);
                book.delete_selection(window, cx);
                assert_eq!(book.cells.len(), 1);
                book.restore_history(false, window, cx);
                assert_eq!(book.session.document().to_json(), original);
                book.restore_history(true, window, cx);
                assert_eq!(book.cells.len(), 1);
                book.restore_history(false, window, cx);
                book.choose_cell_type(CellKind::Code, window, cx);
                assert!(
                    book.cells[..3]
                        .iter()
                        .all(|c| c.read(cx).kind() == CellKind::Code)
                );
                book.restore_history(false, window, cx);
                assert_eq!(book.session.document().to_json(), original);
                book.select(0, cx);
                book.extend_selection(3, window, cx);
                book.delete_selection(window, cx);
                assert_eq!(book.cells.len(), 1);
                assert_eq!(book.cells[0].read(cx).text(), "");
                book.restore_history(false, window, cx);
                assert_eq!(book.session.document().to_json(), original);
                assert_eq!(book.selected_range(), 0..4);
            })
        });
    }

    #[gpui::test]
    fn range_moves_remap_runs_and_running_deletion_is_atomic(cx: &mut gpui::TestAppContext) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let cx = cx.add_empty_window();
        let book = fixture(cx, &runtime);
        cx.run_until_parked();
        cx.update(|window, cx| {
            book.update(cx, |book, cx| {
                book.select(1, cx);
                book.extend_selection(2, window, cx);
                book.runs.active = Some(2);
                book.runs.pending.push_back((3, 1, "d = 4".into()));
                book.delete_selection(window, cx);
                assert_eq!(book.cells.len(), 4);
                assert!(book.undo_structure.is_empty());
                book.move_selection(false, cx);
                assert_eq!(book.selected_range(), 0..2);
                assert_eq!(book.runs.active, Some(1));
                assert_eq!(book.runs.pending[0].0, 3);
                assert_eq!(book.cells[0].read(cx).kind(), CellKind::Markdown);
                book.move_selection(false, cx);
                assert_eq!(book.undo_structure.len(), 1);
                book.move_selection(true, cx);
                assert_eq!(book.runs.active, Some(2));
                assert_eq!(book.selected_range(), 1..3);
                book.runs.active = None;
                book.connecting = true; // inspect the queued group without contacting a server
                book.runs.pending.clear();
                book.select(0, cx);
                book.extend_selection(3, window, cx);
                book.run_range(book.selected_range(), cx);
                assert_eq!(
                    book.runs.pending.iter().map(|r| r.0).collect::<Vec<_>>(),
                    vec![0, 3]
                );
                book.navigate_cells(false, true, window, cx);
                assert_eq!(book.selected_range(), 0..3);
                book.navigate_cells(false, true, window, cx);
                assert_eq!(book.selected_range(), 0..2);
                book.navigate_cells(false, false, window, cx);
                assert_eq!(book.selected_range(), 0..1);
            })
        });
    }
}
