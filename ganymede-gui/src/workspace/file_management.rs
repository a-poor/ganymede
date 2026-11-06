use super::*;
use ganymede_core::{files, text_file::TextFileController};

pub(super) struct FileMenu {
    pub path: String,
    pub directory: bool,
    pub position: gpui::Point<gpui::Pixels>,
    pub selected: Option<usize>,
    pub paths: Vec<String>,
    pub selection_count: usize,
}
#[derive(Clone, Copy)]
pub(super) enum MenuAction {
    Open,
    Rename,
    SaveAs,
    Delete,
    NewFolder,
    NewNotebook,
    NewText,
    Upload,
    Refresh,
}
impl FileMenu {
    fn actions(&self) -> Vec<(MenuAction, &'static str, Icon)> {
        if self.selection_count > 1 {
            return vec![(MenuAction::Delete, "Delete selected…", Icon::Trash)];
        }
        if self.directory {
            let mut items = vec![
                (MenuAction::NewNotebook, "New notebook", Icon::Notebook),
                (MenuAction::NewText, "New text file", Icon::FileText),
                (MenuAction::NewFolder, "New folder…", Icon::FolderPlus),
                (MenuAction::Upload, "Upload here…", Icon::Upload),
                (MenuAction::Refresh, "Refresh", Icon::Restart),
            ];
            if !self.path.is_empty() {
                items.extend([
                    (MenuAction::Rename, "Rename…", Icon::FileText),
                    (MenuAction::Delete, "Delete…", Icon::Trash),
                ]);
            }
            items
        } else {
            vec![
                (MenuAction::Open, "Open", Icon::FileText),
                (MenuAction::Rename, "Rename…", Icon::FileText),
                (MenuAction::SaveAs, "Save As…", Icon::Copy),
                (MenuAction::Delete, "Delete…", Icon::Trash),
            ]
        }
    }
}
#[derive(Clone)]
pub(super) struct DragFile {
    pub paths: Vec<String>,
}
impl Render for DragFile {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme();
        div()
            .px_2()
            .py_1()
            .rounded(t.radius.md())
            .bg(t.popover.bg)
            .text_color(t.popover.fg)
            .text_size(t.font.xs)
            .border_1()
            .border_color(t.popover.border)
            .child(if self.paths.len() == 1 {
                files::name(&self.paths[0]).to_owned()
            } else {
                format!("{} items", self.paths.len())
            })
    }
}
#[derive(Clone)]
enum Controller {
    Notebook(NotebookController),
    Text(TextFileController),
}
impl Controller {
    fn retarget(&self, path: String) {
        match self {
            Self::Notebook(c) => c.retarget_path(path),
            Self::Text(c) => c.retarget_path(path),
        }
    }
    async fn save_as(&self, path: String) -> Result<(), String> {
        match self {
            Self::Notebook(c) => c.save_as(path).await.map_err(|e| e.to_string()),
            Self::Text(c) => c.save_as(path).await.map_err(|e| e.to_string()),
        }
    }
    async fn sync_session(&self) -> Option<String> {
        if let Self::Notebook(c) = self {
            c.sync_session_path().await.err().map(|e| e.to_string())
        } else {
            None
        }
    }
}
#[derive(Clone)]
pub(super) enum Operation {
    Rename { from: String, to: String },
    SaveAs { from: String, to: String },
    Delete(String),
    NewFolder(String),
}

impl Workspace {
    pub(super) fn show_file_menu(
        &mut self,
        path: String,
        directory: bool,
        position: gpui::Point<gpui::Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.file_busy || window.has_active_prompt() {
            return;
        }
        self.files_menu_open = false;
        let mut rows = Vec::new();
        self.tree_rows("", 0, &mut rows);
        self.file_selection
            .paths
            .retain(|path| rows.iter().any(|(entry, _)| &entry.path == path));
        if !self.file_selection.paths.contains(&path) {
            self.select_file_path(&path, directory);
        }
        let paths = selection_roots(self.file_selection.paths.clone());
        self.file_menu = Some(FileMenu {
            path,
            directory,
            position,
            selected: None,
            selection_count: self.file_selection.paths.len(),
            paths,
        });
        self.file_menu_focus.focus(window, cx);
        cx.notify();
    }
    pub(super) fn select_file_path(&mut self, path: &str, directory: bool) {
        self.file_selection.single(path);
        self.file_directory = if directory {
            path.to_owned()
        } else {
            files::parent(path).to_owned()
        };
    }
    pub(super) fn run_file_menu_action(
        &mut self,
        action: MenuAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(menu) = self.file_menu.take() else {
            return;
        };
        if menu.selection_count <= 1 {
            self.select_file_path(&menu.path, menu.directory);
        }
        match action {
            MenuAction::Open => {
                if menu.path.ends_with(".ipynb") {
                    self.open_notebook(Some(menu.path), window, cx);
                } else {
                    self.open_text(
                        Some(menu.path.clone()),
                        DocumentKind::for_text_path(&menu.path),
                        window,
                        cx,
                    );
                }
            }
            MenuAction::Rename => self.prompt_rename(menu.path, menu.directory, window, cx),
            MenuAction::SaveAs => self.prompt_save_as(menu.path, window, cx),
            MenuAction::Delete => {
                let directory = menu.paths.len() == 1
                    && self.directories.values().flatten().any(|entry| {
                        entry.path == menu.paths[0] && entry.content_type == "directory"
                    });
                self.prompt_delete(menu.paths, directory, window, cx);
            }
            MenuAction::NewFolder => self.prompt_new_folder(window, cx),
            MenuAction::NewNotebook => self.create(DocumentKind::Notebook, window, cx),
            MenuAction::NewText => self.create(DocumentKind::Text, window, cx),
            MenuAction::Upload => self.upload(window, cx),
            MenuAction::Refresh => self.refresh_files(cx),
        }
        cx.notify();
    }
    pub(super) fn prompt_new_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.file_busy || window.has_active_prompt() {
            return;
        }
        let parent = self.file_directory.clone();
        let detail = format!(
            "Create a folder in {}.",
            if parent.is_empty() {
                "the server root"
            } else {
                &parent
            }
        );
        let response = crate::confirm::name_prompt(
            crate::confirm::NamePrompt {
                title: "New folder",
                detail: &detail,
                label: "Name",
                initial: "Untitled Folder".into(),
                allow_path: false,
                action: "Create",
            },
            window,
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if let Some(name) = response.await {
                this.update_in(cx, |this, window, cx| {
                    if let Ok(path) = files::child_path(&parent, &name) {
                        this.manage(Operation::NewFolder(path), window, cx);
                    }
                })
                .ok();
            }
        })
        .detach();
    }
    pub(super) fn prompt_rename(
        &mut self,
        path: String,
        directory: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if window.has_active_prompt() {
            return;
        }
        let response = crate::confirm::name_prompt(
            crate::confirm::NamePrompt {
                title: if directory {
                    "Rename folder"
                } else {
                    "Rename file"
                },
                detail: "Choose a new name in the same folder.",
                label: "Name",
                initial: files::name(&path).to_owned(),
                allow_path: false,
                action: "Rename",
            },
            window,
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if let Some(name) = response.await {
                this.update_in(cx, |this, window, cx| {
                    let Ok(to) = files::child_path(files::parent(&path), &name) else {
                        return;
                    };
                    if to == path {
                        return;
                    }
                    if !directory && !compatible_extension(&path, &to) {
                        this.file_toast(
                            "Could not rename file",
                            Err(
                                "Keep .ipynb for notebooks; use another extension for text files."
                                    .into(),
                            ),
                            cx,
                        );
                        return;
                    }
                    this.manage(Operation::Rename { from: path, to }, window, cx);
                })
                .ok();
            }
        })
        .detach();
    }
    pub(super) fn prompt_save_as(
        &mut self,
        path: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.file_busy || window.has_active_prompt() {
            return;
        }
        let response = crate::confirm::name_prompt(
            crate::confirm::NamePrompt {
                title: "Save As",
                detail: "Enter a new path relative to the server root. Existing files won't be replaced.",
                label: "File path",
                initial: suggested_copy_path(&path),
                allow_path: true,
                action: "Save",
            },
            window,
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if let Some(to) = response.await {
                this.update_in(cx, |this, window, cx| {
                    if !compatible_extension(&path, &to) {
                        this.file_toast(
                            "Could not save file",
                            Err(
                                "Keep .ipynb for notebooks; use another extension for text files."
                                    .into(),
                            ),
                            cx,
                        );
                        return;
                    }
                    this.manage(Operation::SaveAs { from: path, to }, window, cx);
                })
                .ok();
            }
        })
        .detach();
    }
    fn prompt_delete(
        &mut self,
        paths: Vec<String>,
        directory: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if window.has_active_prompt() || paths.is_empty() {
            return;
        }
        let dirty = self.documents.iter().any(|doc| {
            doc.path
                .as_ref()
                .is_some_and(|p| paths.iter().any(|path| files::is_within(p, path)))
                && doc.content.is_dirty(cx)
        });
        let items = if paths.len() == 1 {
            paths[0].clone()
        } else {
            format!("{} selected items", paths.len())
        };
        let detail = format!(
            "Delete {items} from the Jupyter server?{}",
            if dirty {
                " Unsaved changes in these items will also be discarded."
            } else {
                " This cannot be undone in Ganymede."
            }
        );
        let response = window.prompt(
            gpui::PromptLevel::Warning,
            if paths.len() > 1 {
                "Delete selected items?"
            } else if directory {
                "Delete folder?"
            } else {
                "Delete file?"
            },
            Some(&detail),
            &["Cancel", "Delete"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if response.await.ok() == Some(1) {
                this.update_in(cx, |this, window, cx| {
                    this.manage_many(
                        paths.into_iter().map(Operation::Delete).collect(),
                        window,
                        cx,
                    )
                })
                .ok();
            }
        })
        .detach();
    }
    pub(super) fn move_files(
        &mut self,
        paths: Vec<String>,
        directory: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.file_busy || window.has_active_prompt() {
            return;
        }
        let paths = selection_roots(paths);
        if paths.iter().any(|from| files::is_within(&directory, from)) {
            return;
        }
        let operations = paths
            .into_iter()
            .filter_map(|from| {
                let to = files::child_path(&directory, files::name(&from)).ok()?;
                (to != from).then_some(Operation::Rename { from, to })
            })
            .collect();
        self.manage_many(operations, window, cx);
    }
    fn manage_many(
        &mut self,
        mut operations: std::collections::VecDeque<Operation>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.file_busy {
            return;
        }
        if let Some(first) = operations.pop_front() {
            self.file_operations = operations;
            self.manage(first, window, cx);
        }
    }
    fn set_file_busy(&mut self, busy: bool, cx: &mut Context<Self>) {
        self.file_busy = busy;
        for doc in &self.documents {
            match &doc.content {
                Content::Notebook(view) => view.update(cx, |view, _| view.set_file_busy(busy)),
                Content::Text(view) => view.update(cx, |view, _| view.set_file_busy(busy)),
                _ => (),
            }
        }
        cx.notify();
    }
    fn manage(&mut self, operation: Operation, window: &mut Window, cx: &mut Context<Self>) {
        if self.file_busy {
            return;
        }
        self.set_file_busy(true, cx);
        cx.spawn_in(window, async move |this, cx| {
            // Let pending saves and opens finish before changing their paths. New
            // saves/opens are gated until all local paths have been retargeted.
            loop {
                let ready = this.update(cx, |this, cx| {
                    !this.uploading
                        && !this.creating
                        && this.opening.is_empty()
                        && !this.documents.iter().any(|doc| match &doc.content {
                            Content::Notebook(view) => view.read(cx).file_operation_pending(),
                            _ => doc.content.is_saving(cx),
                        })
                });
                match ready {
                    Ok(true) => break,
                    Ok(false) => {
                        cx.background_executor()
                            .timer(Duration::from_millis(50))
                            .await
                    }
                    Err(_) => return,
                }
            }
            let job = operation.clone();
            let task = this.update(cx, |this, cx| {
                let controllers: Vec<_> = this
                    .documents
                    .iter()
                    .filter_map(|doc| {
                        let path = doc.path.clone()?;
                        let controller = match &doc.content {
                            Content::Notebook(view) => Controller::Notebook(
                                view.update(cx, |view, cx| view.file_controller(cx)),
                            ),
                            Content::Text(view) => {
                                Controller::Text(view.read(cx).file_controller())
                            }
                            _ => return None,
                        };
                        Some((path, controller))
                    })
                    .collect();
                let runtime = &this.runtime;
                let config = runtime.config.clone();
                runtime.handle.spawn(async move {
                    let client = JupyterRestClient::new(config?).map_err(|e| e.to_string())?;
                    let mut warnings = Vec::new();
                    match job {
                        Operation::Rename { from, to } => {
                            client
                                .rename_contents(&from, &to)
                                .await
                                .map_err(|e| e.to_string())?;
                            for (path, controller) in &controllers {
                                if files::is_within(path, &from) {
                                    controller.retarget(files::remap(path, &from, &to));
                                    if let Some(error) = controller.sync_session().await {
                                        warnings.push(error);
                                    }
                                }
                            }
                        }
                        Operation::SaveAs { from, to } => {
                            if let Some((_, controller)) =
                                controllers.iter().find(|(path, _)| path == &from)
                            {
                                controller.save_as(to).await?;
                                if let Some(error) = controller.sync_session().await {
                                    warnings.push(error);
                                }
                            } else {
                                client
                                    .copy_contents(&from, &to)
                                    .await
                                    .map_err(|e| e.to_string())?;
                            }
                        }
                        Operation::Delete(path) => client
                            .delete_contents(&path)
                            .await
                            .map_err(|e| e.to_string())?,
                        Operation::NewFolder(path) => {
                            client
                                .create_named_directory(&path)
                                .await
                                .map_err(|e| e.to_string())?;
                        }
                    }
                    Ok::<_, String>(warnings)
                })
            });
            let Ok(task) = task else {
                return;
            };
            let result = task
                .await
                .unwrap_or_else(|_| Err("File operation failed".into()));
            this.update_in(cx, |this, window, cx| {
                this.set_file_busy(false, cx);
                match result {
                    Err(error) => this.file_toast("File operation failed", Err(error), cx),
                    Ok(warnings) => {
                        match operation {
                            Operation::Rename { from, to } => {
                                let directory = this.directories.values().flatten().any(|entry| {
                                    entry.path == from && entry.content_type == "directory"
                                });
                                this.retarget_documents(&from, &to, false, cx);
                                this.expanded = this
                                    .expanded
                                    .iter()
                                    .map(|p| files::remap(p, &from, &to))
                                    .collect();
                                this.file_directory = if directory {
                                    to.clone()
                                } else {
                                    files::parent(&to).to_owned()
                                };
                                this.reveal_file_parent(&to);
                                this.file_selection.remap(&from, &to);
                            }
                            Operation::SaveAs { from, to } => {
                                let open = this.retarget_documents(&from, &to, true, cx);
                                this.file_directory = files::parent(&to).to_owned();
                                this.file_selection.single(&to);
                                this.reveal_file_parent(&to);
                                if let Some(id) = open {
                                    this.activate(id, window, cx);
                                } else {
                                    // Closed files may be binary; keep the saved copy selected
                                    // without attempting to load it into a text editor.
                                    this.file_toast("File saved", Ok(to), cx);
                                }
                            }
                            Operation::Delete(path) => {
                                let ids: Vec<_> = this
                                    .documents
                                    .iter()
                                    .filter(|doc| {
                                        doc.path
                                            .as_ref()
                                            .is_some_and(|p| files::is_within(p, &path))
                                    })
                                    .map(|doc| doc.id)
                                    .collect();
                                for id in ids {
                                    this.discard_recovery(super::CloseTarget::Tab(id));
                                    this.tabs.close(id);
                                    this.documents.retain(|doc| doc.id != id);
                                }
                                this.expanded.retain(|p| !files::is_within(p, &path));
                                this.file_directory = files::parent(&path).to_owned();
                                this.file_selection.remove(&path);
                                let selection = this.file_selection.clone();
                                this.activate(this.tabs.active, window, cx);
                                this.file_selection = selection;
                            }
                            Operation::NewFolder(path) => {
                                this.file_directory = path.clone();
                                this.file_selection.single(&path);
                                this.reveal_file_parent(&path);
                                this.expanded.insert(path);
                            }
                        }
                        // Ignore outstanding pre-mutation listings; never reintroduce old paths.
                        this.tree_generation += 1;
                        this.loading.clear();
                        for error in warnings {
                            this.file_toast(
                                "File updated, but kernel session path could not be updated",
                                Err(error),
                                cx,
                            );
                        }
                    }
                }
                if let Some(next) = this.file_operations.pop_front() {
                    this.manage(next, window, cx);
                } else {
                    this.directories.clear();
                    this.filter_requested.clear();
                    this.refresh_files(cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
    pub(super) fn reveal_file_parent(&mut self, path: &str) {
        let mut parent = files::parent(path);
        while !parent.is_empty() {
            self.expanded.insert(parent.to_owned());
            parent = files::parent(parent);
        }
    }
    fn retarget_documents(
        &mut self,
        from: &str,
        to: &str,
        saved_as: bool,
        cx: &mut Context<Self>,
    ) -> Option<u64> {
        let mut active = None;
        for doc in &mut self.documents {
            if let Some(path) = &doc.path
                && files::is_within(path, from)
            {
                let path = files::remap(path, from, to);
                doc.name = files::name(&path).to_owned().into();
                if doc.kind != DocumentKind::Notebook {
                    doc.kind = DocumentKind::for_text_path(&path);
                }
                doc.path = Some(path);
                active = Some(doc.id);
                if saved_as {
                    match &doc.content {
                        Content::Notebook(view) => {
                            view.update(cx, |view, cx| view.file_saved_as(cx))
                        }
                        Content::Text(view) => view.update(cx, |view, cx| view.file_saved_as(cx)),
                        _ => (),
                    }
                }
            }
        }
        active
    }
    pub(super) fn file_context_menu(entity: &Entity<Self>, cx: &App) -> Option<gpui::AnyElement> {
        let this = entity.read(cx);
        let menu = this.file_menu.as_ref()?;
        let t = cx.theme();
        let outside = entity.clone();
        let key = entity.clone();
        Some(
            gpui::deferred(
                gpui::anchored()
                    .position(menu.position)
                    .snap_to_window_with_margin(px(8.))
                    .child(
                        div()
                            .id("file-context-menu")
                            .occlude()
                            .track_focus(&this.file_menu_focus)
                            .w(px(170.))
                            .p_1()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .rounded(t.radius.md())
                            .bg(t.popover.bg)
                            .text_color(t.popover.fg)
                            .border_1()
                            .border_color(t.popover.border)
                            .shadow(vec![t.toast_shadow.clone()])
                            .on_mouse_down_out(move |_, _, cx| {
                                outside.update(cx, |this, cx| {
                                    this.file_menu = None;
                                    cx.notify();
                                });
                            })
                            .on_key_down(move |event, window, cx| {
                                key.update(cx, |this, cx| {
                                    let Some(menu) = &mut this.file_menu else {
                                        return;
                                    };
                                    let actions = menu.actions();
                                    match event.keystroke.key.as_str() {
                                        "escape" | "tab" => {
                                            this.file_menu = None;
                                            window.blur();
                                        }
                                        "down" => {
                                            menu.selected = Some(
                                                menu.selected
                                                    .map_or(0, |i| (i + 1) % actions.len()),
                                            )
                                        }
                                        "up" => {
                                            menu.selected =
                                                Some(menu.selected.map_or(actions.len() - 1, |i| {
                                                    (i + actions.len() - 1) % actions.len()
                                                }))
                                        }
                                        "enter" | "space" => {
                                            if let Some(i) = menu.selected {
                                                this.run_file_menu_action(actions[i].0, window, cx);
                                            }
                                        }
                                        _ => return,
                                    }
                                    cx.stop_propagation();
                                    cx.notify();
                                });
                            })
                            .children(menu.actions().into_iter().enumerate().map(
                                |(index, (action, label, icon))| {
                                    let click = entity.clone();
                                    let hover = entity.clone();
                                    let destructive = matches!(action, MenuAction::Delete);
                                    let fg = if destructive {
                                        t.dialog_destructive_fg
                                    } else if menu.selected == Some(index) {
                                        t.popover.accent_fg
                                    } else {
                                        t.popover.fg
                                    };
                                    div()
                                        .id(("file-context-action", index))
                                        .h(px(26.))
                                        .px_2()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .rounded(t.radius.sm())
                                        .text_size(t.font.xs)
                                        .text_color(fg)
                                        .cursor_pointer()
                                        .when(menu.selected == Some(index), |view| {
                                            view.bg(t.popover.accent)
                                        })
                                        .child(icon.svg().size(px(14.)).text_color(fg))
                                        .child(label)
                                        .on_hover(move |hovered, _, cx| {
                                            hover.update(cx, |this, cx| {
                                                if let Some(menu) = &mut this.file_menu {
                                                    if *hovered {
                                                        menu.selected = Some(index);
                                                    } else if menu.selected == Some(index) {
                                                        menu.selected = None;
                                                    }
                                                    cx.notify();
                                                }
                                            });
                                        })
                                        .on_click(move |_, window, cx| {
                                            cx.stop_propagation();
                                            click.update(cx, |this, cx| {
                                                this.run_file_menu_action(action, window, cx)
                                            });
                                        })
                                },
                            )),
                    ),
            )
            .into_any_element(),
        )
    }
}
fn compatible_extension(from: &str, to: &str) -> bool {
    from.ends_with(".ipynb") == to.ends_with(".ipynb")
}
fn suggested_copy_path(path: &str) -> String {
    let name = files::name(path);
    let name = if let Some((stem, ext)) = name.rsplit_once('.')
        && !stem.is_empty()
    {
        format!("{stem}-copy.{ext}")
    } else {
        format!("{name}-copy")
    };
    files::child_path(files::parent(path), &name).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn save_as_suggestions_preserve_extensions_and_parent_folders() {
        assert_eq!(
            suggested_copy_path("project/notes.ipynb"),
            "project/notes-copy.ipynb"
        );
        assert_eq!(suggested_copy_path(".env"), ".env-copy");
        assert_eq!(suggested_copy_path("README"), "README-copy");
        assert!(compatible_extension("x.py", "x.md"));
        assert!(!compatible_extension("x.ipynb", "x.py"));
    }

    #[tokio::test]
    #[ignore = "requires the Jupyter server configured in .env"]
    async fn live_file_management_preserves_open_documents_and_kernel_paths() {
        let runtime = Runtime::new(tokio::runtime::Handle::current());
        let config = runtime.config.unwrap();
        let client = JupyterRestClient::new(config.clone()).unwrap();
        let root = client.create_directory("").await.unwrap().path;
        let folder = root.clone();
        let rest = client.clone();
        let session_id = Arc::new(Mutex::new(None));
        let cleanup_session = session_id.clone();
        let result = tokio::spawn(async move {
            let left = format!("{folder}/left folder");
            let right = format!("{folder}/right");
            rest.create_named_directory(&left).await.unwrap();
            rest.create_named_directory(&right).await.unwrap();
            assert!(rest.create_named_directory(&right).await.is_err());
            let path = format!("{left}/notes.toml");
            rest.upload_file(&path, b"version = 1\n").await.unwrap();
            let text = TextFileController::open(config.clone(), &path)
                .await
                .unwrap();
            text.update_text("version = 2\n".into());
            let renamed = format!("{folder}/renamed folder");
            rest.rename_contents(&left, &renamed).await.unwrap();
            let path = files::remap(&path, &left, &renamed);
            text.retarget_path(path.clone());
            assert!(text.status().1);
            text.save(false).await.unwrap();
            assert_eq!(
                TextFileController::open(config.clone(), &path)
                    .await
                    .unwrap()
                    .snapshot()
                    .text,
                "version = 2\n"
            );
            let moved = format!("{right}/moved.toml");
            rest.rename_contents(&path, &moved).await.unwrap();
            text.retarget_path(moved.clone());
            text.update_text("version = 3\n".into());
            text.save(false).await.unwrap();
            let copy = format!("{right}/copy.toml");
            text.update_text("version = 4\n".into());
            text.save_as(copy.clone()).await.unwrap();
            assert_eq!(text.path(), copy);
            assert_eq!(
                TextFileController::open(config.clone(), &moved)
                    .await
                    .unwrap()
                    .snapshot()
                    .text,
                "version = 3\n"
            );
            assert!(text.save_as(moved.clone()).await.is_err());
            assert!(
                rest.rename_contents(&renamed, &format!("{renamed}/child"))
                    .await
                    .is_err()
            );
            let binary = format!("{right}/upload.bin");
            rest.upload_file(&binary, &[0, 255, 17, 42]).await.unwrap();
            let binary_copy = format!("{right}/copy.bin");
            rest.copy_contents(&binary, &binary_copy).await.unwrap();
            assert_eq!(
                rest.get_contents(&binary, true).await.unwrap().content,
                rest.get_contents(&binary_copy, true).await.unwrap().content
            );

            let created = rest.create_notebook(&renamed).await.unwrap();
            let notebook = NotebookController::open(config, &created.path)
                .await
                .unwrap();
            notebook.connect("python3").await.unwrap();
            let id = notebook.snapshot().session.as_ref().unwrap().id.clone();
            *session_id.lock().unwrap() = Some(id.clone());
            notebook
                .insert_cell(0, ganymede_core::notebook::CellKind::Code, "1 + 1")
                .unwrap();
            let moved = format!("{right}/moved.ipynb");
            rest.rename_contents(&created.path, &moved).await.unwrap();
            notebook.retarget_path(moved.clone());
            notebook.sync_session_path().await.unwrap();
            assert_eq!(rest.get_session(&id).await.unwrap().path, moved);
            assert!(notebook.snapshot().document.is_dirty());
            notebook.save().await.unwrap();
            let copy = format!("{right}/copy.ipynb");
            notebook.save_as(copy.clone()).await.unwrap();
            notebook.sync_session_path().await.unwrap();
            assert_eq!(rest.get_session(&id).await.unwrap().path, copy);
            assert_eq!(notebook.snapshot().path, copy);
            assert!(!notebook.snapshot().document.is_dirty());
            notebook.shutdown_session().await.unwrap();
            *session_id.lock().unwrap() = None;
            rest.delete_contents(&binary_copy).await.unwrap();
            assert!(rest.get_contents(&binary_copy, false).await.is_err());
        })
        .await;
        let session = cleanup_session.lock().unwrap().take();
        if let Some(id) = session {
            client.delete_session(&id).await.ok();
        }
        // Remove only this test's server-allocated subtree, even if an assertion failed.
        let mut directories = vec![root];
        let mut index = 0;
        while index < directories.len() {
            let listing = client
                .get_contents(&directories[index], true)
                .await
                .unwrap();
            let entries: Vec<Contents> = serde_json::from_value(listing.content).unwrap();
            for entry in entries {
                if entry.content_type == "directory" {
                    directories.push(entry.path);
                } else {
                    client.delete_contents(&entry.path).await.unwrap();
                }
            }
            index += 1;
        }
        for directory in directories.into_iter().rev() {
            client.delete_contents(&directory).await.unwrap();
        }
        result.unwrap();
    }
}
