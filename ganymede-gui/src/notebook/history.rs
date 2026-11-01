//! Structural history retains editor entities (and their independent text undo stacks).
//! Only deleted/type-changed cells need copies of their persisted payloads.
use super::*;
use ganymede_core::notebook::{Cell, CellId};

struct Position {
    editor: Entity<CodeCell>,
    id: CellId,
    kind: CellKind,
}

pub(super) struct Entry {
    order: Vec<Position>,
    selected: usize,
    restored: Vec<Cell>,
}

impl Notebook {
    fn sync_history(&mut self, cx: &mut Context<Self>) -> bool {
        if let Err(error) = self.session.sync(&self.editors(cx)) {
            self.toasts.update(cx, |host, cx| {
                host.push(
                    crate::toast::Toast::new("Could not update notebook")
                        .description(error)
                        .kind(crate::toast::ToastKind::Error),
                    cx,
                );
            });
            return false;
        }
        true
    }

    fn history_entry(&self, affected: impl Fn(&CellId, CellKind) -> bool) -> Entry {
        let document = self.session.document();
        Entry {
            order: self
                .cells
                .iter()
                .zip(document.cells())
                .map(|(editor, cell)| Position {
                    editor: editor.clone(),
                    id: cell.id().clone(),
                    kind: cell.kind(),
                })
                .collect(),
            selected: self.selected,
            restored: document
                .cells()
                .iter()
                .filter(|cell| affected(cell.id(), cell.kind()))
                .cloned()
                .collect(),
        }
    }

    /// Call before a real structural edit; no-op commands must not erase redo.
    pub(super) fn record_structure(
        &mut self,
        affected: Option<usize>,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.sync_history(cx) {
            return false;
        }
        let id = affected.and_then(|index| {
            self.session
                .document()
                .cells()
                .get(index)
                .map(|c| c.id().clone())
        });
        let entry = self.history_entry(|cell, _| Some(cell) == id.as_ref());
        self.undo_structure.push(entry);
        if self.undo_structure.len() > 100 {
            self.undo_structure.remove(0);
        }
        self.redo_structure.clear();
        true
    }

    pub(super) fn restore_history(
        &mut self,
        redo: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.sync_history(cx) {
            return;
        }
        let stack = if redo {
            &mut self.redo_structure
        } else {
            &mut self.undo_structure
        };
        let Some(entry) = stack.pop() else {
            return;
        };
        let inverse = self.history_entry(|id, kind| {
            entry
                .order
                .iter()
                .find(|position| &position.id == id)
                .is_none_or(|position| position.kind != kind)
        });
        let bindings: Vec<_> = entry
            .order
            .iter()
            .map(|position| (position.editor.entity_id().as_u64(), position.id.clone()))
            .collect();
        if let Err(error) = self.session.restore_structure(&bindings, &entry.restored) {
            let stack = if redo {
                &mut self.redo_structure
            } else {
                &mut self.undo_structure
            };
            stack.push(entry);
            self.toasts.update(cx, |host, cx| {
                host.push(
                    crate::toast::Toast::new(if redo {
                        "Could not redo cell change"
                    } else {
                        "Could not undo cell change"
                    })
                    .description(error)
                    .kind(crate::toast::ToastKind::Error),
                    cx,
                );
            });
            return;
        }
        // Preserve active/queued runs by identity, dropping queued work for removed cells.
        let index_map: Vec<_> = self
            .cells
            .iter()
            .map(|cell| entry.order.iter().position(|p| p.editor == *cell))
            .collect();
        self.runs.active = self.runs.active.and_then(|index| index_map[index]);
        self.runs.pending = self
            .runs
            .pending
            .drain(..)
            .filter_map(|(index, count, source)| {
                let next = index_map[index]?;
                let position = &entry.order[next];
                (position.kind == self.cells[index].read(cx).kind())
                    .then_some((next, count, source))
            })
            .collect();
        self.cells = entry
            .order
            .iter()
            .map(|position| position.editor.clone())
            .collect();
        let document = self.session.document();
        for position in &entry.order {
            if entry.restored.iter().any(|cell| cell.id() == &position.id) {
                let cell = document.cell(&position.id).unwrap();
                position.editor.update(cx, |editor, cx| {
                    editor.clear_execution(cx);
                    editor.load(cell, cx);
                });
            }
        }
        if redo {
            self.undo_structure.push(inverse);
        } else {
            self.redo_structure.push(inverse);
        }
        self.select(entry.selected.min(self.cells.len() - 1), cx);
        self.reveal_cell = Some(self.selected);
        self.find_dirty = true;
        self.cells[self.selected]
            .read(cx)
            .list_focus_handle()
            .focus(window, cx);
        self.start_next_run(cx);
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ganymede_core::{
        jupyter::ServerConfig,
        notebook::{NotebookController, NotebookDocument},
    };
    use serde_json::json;

    #[gpui::test]
    fn structural_history_restores_payloads_preserves_edits_and_branches(
        cx: &mut gpui::TestAppContext,
    ) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let cx = cx.add_empty_window();
        let document = NotebookDocument::from_json(json!({"nbformat":4,"nbformat_minor":5,"metadata":{},"cells":[
            {"id":"a","cell_type":"code","source":"print(1)","metadata":{"tag":"keep"},"execution_count":7,"vendor":true,
             "outputs":[{"output_type":"stream","name":"stdout","text":"1\n","vendor":12}]},
            {"id":"b","cell_type":"markdown","source":"hello","metadata":{},"attachments":{"x":{"image/png":"abc"}}}
        ]})).unwrap();
        let original = document.to_json();
        let notebook = cx.update(|window, cx| {
            cx.set_global(
                crate::theme::ThemeSpec::light().resolve(&crate::theme::Derivation::default()),
            );
            cx.set_global(crate::autosave::AutosavePreference(false));
            let config = ServerConfig::new("http://127.0.0.1:9").unwrap();
            let controller =
                NotebookController::new(config.clone(), "test.ipynb", document).unwrap();
            let bridge = crate::execution::Runtime {
                handle: runtime.handle().clone(),
                config: Ok(config),
                kernel: "python3".into(),
            };
            let session = crate::execution::Session::from_controller(controller, &bridge);
            let toasts = cx.new(|cx| crate::toast::ToastHost::new(window, cx));
            cx.new(|cx| Notebook::new(session, toasts, window, cx))
        });
        cx.update(|window, cx| {
            notebook.update(cx, |book, cx| {
                book.toolbar_action(ToolbarAction::Cut, window, cx);
                assert_eq!(book.cells.len(), 1);
                book.restore_history(false, window, cx);
                assert_eq!(book.session.document().to_json(), original);
                assert!(book.session.document().is_dirty());
                assert!(
                    book.cells[0]
                        .read(cx)
                        .list_focus_handle()
                        .is_focused(window)
                );
                book.restore_history(true, window, cx);
                assert_eq!(book.cells.len(), 1);
                book.restore_history(false, window, cx);
                book.choose_cell_type(CellKind::Markdown, window, cx);
                book.cells[0].update(cx, |cell, cx| cell.set_text("new source".into(), cx));
                book.restore_history(false, window, cx);
                let doc = book.session.document();
                assert_eq!(doc.cells()[0].kind(), CellKind::Code);
                assert_eq!(doc.cells()[0].source(), "new source");
                assert_eq!(doc.cells()[0].outputs().len(), 1);
                assert_eq!(doc.cells()[0].execution_count(), Some(7));
                book.restore_history(true, window, cx);
                assert_eq!(
                    book.session.document().cells()[0].kind(),
                    CellKind::Markdown
                );
                assert!(book.session.document().cells()[0].outputs().is_empty());
                book.restore_history(false, window, cx);
                book.toolbar_action(ToolbarAction::Add, window, cx);
                assert!(book.redo_structure.is_empty());
                let added = book.cells[1].entity_id();
                book.cells[1].update(cx, |cell, cx| cell.set_text("retained text".into(), cx));
                book.restore_history(false, window, cx);
                assert_eq!(book.cells.len(), 2);
                book.restore_history(true, window, cx);
                assert_eq!(book.cells[1].entity_id(), added);
                assert_eq!(book.cells[1].read(cx).text(), "retained text");
                // Deleting the last cell creates one replacement, as one undo unit.
                book.select(2, cx);
                book.toolbar_action(ToolbarAction::Cut, window, cx);
                book.select(1, cx);
                book.toolbar_action(ToolbarAction::Cut, window, cx);
                book.select(0, cx);
                let last = book.cells[0].entity_id();
                book.toolbar_action(ToolbarAction::Cut, window, cx);
                assert_eq!(book.cells.len(), 1);
                assert_ne!(book.cells[0].entity_id(), last);
                book.restore_history(false, window, cx);
                assert_eq!(book.cells.len(), 1);
                assert_eq!(book.cells[0].entity_id(), last);
            })
        });
        // Menu actions use the same history path and keep their editor identities.
        let emit = |event, cx: &mut gpui::VisualTestContext| {
            cx.update(|_, cx| {
                let cell = notebook.read(cx).cells[0].clone();
                cell.update(cx, |_, cx| cx.emit(event));
            });
        };
        emit(CellEvent::Duplicate, cx);
        cx.update(|window, cx| {
            notebook.update(cx, |book, cx| {
                assert_eq!(book.cells.len(), 2);
                book.restore_history(false, window, cx);
                assert_eq!(book.cells.len(), 1);
                book.restore_history(true, window, cx);
                assert_eq!(book.cells.len(), 2);
            })
        });
        let first = cx.update(|_, cx| notebook.read(cx).cells[0].entity_id());
        emit(CellEvent::MoveDown, cx);
        cx.update(|window, cx| {
            notebook.update(cx, |book, cx| {
                assert_eq!(book.cells[1].entity_id(), first);
                book.restore_history(false, window, cx);
                assert_eq!(book.cells[0].entity_id(), first);
            })
        });
        emit(CellEvent::DeleteCell, cx);
        cx.update(|window, cx| {
            notebook.update(cx, |book, cx| {
                assert_eq!(book.cells.len(), 1);
                book.restore_history(false, window, cx);
                assert_eq!(book.cells[0].entity_id(), first);
                assert_eq!(book.session.document().cells()[0].outputs().len(), 1);
            })
        });
    }
}
