//! Server file browser, notebook tabs, and launcher.
use crate::execution::{Runtime, Session};
use crate::{
    code_cell::CodeCell,
    icon::Icon,
    notebook::Notebook,
    theme::{ActiveTheme, Theme},
};
use ganymede_core::{
    jupyter::{JupyterRestClient, models::Contents},
    notebook::NotebookController,
};
use gpui::{
    App, Context, Entity, Focusable, FontWeight, ScrollHandle, SharedString, Window, div,
    prelude::*, px,
};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DocumentKind {
    Notebook,
    Text,
    Markdown,
    Python,
    Terminal,
    Console,
}
impl DocumentKind {
    fn icon(self) -> Icon {
        match self {
            Self::Notebook => Icon::Notebook,
            Self::Terminal | Self::Console => Icon::Terminal,
            Self::Python => Icon::FileCode,
            _ => Icon::FileText,
        }
    }
    fn extension(self) -> &'static str {
        match self {
            Self::Notebook => "ipynb",
            Self::Text => "txt",
            Self::Markdown => "md",
            Self::Python => "py",
            Self::Terminal | Self::Console => "",
        }
    }
}
enum Content {
    Notebook(Entity<Notebook>),
    Text(Entity<CodeCell>),
    Terminal,
    Console,
}
struct Document {
    id: u64,
    name: SharedString,
    path: Option<String>,
    kind: DocumentKind,
    content: Content,
}
// Zero is reserved for the one launcher tab.
const LAUNCHER: u64 = 0;
#[derive(Debug)]
struct Tabs {
    open: Vec<u64>,
    active: u64,
}
impl Default for Tabs {
    fn default() -> Self {
        Self {
            open: vec![LAUNCHER],
            active: LAUNCHER,
        }
    }
}
impl Tabs {
    fn activate(&mut self, id: u64) {
        if !self.open.contains(&id) {
            self.open.push(id);
        }
        self.active = id;
    }
    fn launch(&mut self, id: u64) {
        if let Some(index) = self.open.iter().position(|tab| *tab == LAUNCHER) {
            self.open[index] = id;
        }
        self.activate(id);
    }
    fn close(&mut self, id: u64) {
        let Some(index) = self.open.iter().position(|tab| *tab == id) else {
            return;
        };
        self.open.remove(index);
        if self.open.is_empty() {
            self.activate(LAUNCHER);
        } else if self.active == id {
            self.active = self.open[index.min(self.open.len() - 1)];
        }
    }
}
#[derive(Clone, Copy)]
pub enum CloseTarget {
    Tab(u64),
    Window,
    Quit,
}

#[derive(Clone, Copy)]
enum FileAction {
    Launcher,
    Folder,
    Filter,
    Menu,
    Upload,
    Refresh,
}

pub struct Workspace {
    toasts: Entity<crate::toast::ToastHost>,
    documents: Vec<Document>,
    tabs: Tabs,
    tab_scroll: ScrollHandle,
    next_id: u64,
    directories: HashMap<String, Vec<Contents>>,
    expanded: HashSet<String>,
    loading: HashSet<String>,
    opening: HashSet<String>,
    creating: bool,
    close_pending: bool,
    file_error: Option<String>,
    files_menu_open: bool,
    files_menu_index: Option<usize>,
    files_menu_focus: gpui::FocusHandle,
    notebooks_only: bool,
    creating_folder: bool,
    uploading: bool,
}
impl Workspace {
    pub fn new(
        toasts: Entity<crate::toast::ToastHost>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe_global::<Theme>(|_, cx| cx.notify()).detach();
        let mut this = Self {
            toasts,
            documents: vec![],
            tabs: Tabs::default(),
            tab_scroll: ScrollHandle::new(),
            next_id: 0,
            directories: HashMap::new(),
            expanded: HashSet::new(),
            loading: HashSet::new(),
            opening: HashSet::new(),
            creating: false,
            close_pending: false,
            file_error: None,
            files_menu_open: false,
            files_menu_index: None,
            files_menu_focus: cx.focus_handle(),
            notebooks_only: false,
            creating_folder: false,
            uploading: false,
        };
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(10))
                    .await;
                if this.update(cx, |this, cx| this.refresh_files(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        this.refresh_directory(String::new(), cx);
        this.create(DocumentKind::Notebook, window, cx);
        this
    }
    fn create(&mut self, kind: DocumentKind, window: &mut Window, cx: &mut Context<Self>) {
        if kind == DocumentKind::Notebook {
            self.open_notebook(None, window, cx);
            return;
        }
        self.next_id += 1;
        let number = self.documents.iter().filter(|doc| doc.kind == kind).count();
        let name = if kind == DocumentKind::Console {
            format!("Console {}", number + 1)
        } else if kind == DocumentKind::Terminal {
            format!("Terminal {}", number + 1)
        } else if number == 0 {
            format!("Untitled.{}", kind.extension())
        } else {
            format!("Untitled-{}.{}", number + 1, kind.extension())
        };
        let content = match kind {
            DocumentKind::Notebook => unreachable!(),
            DocumentKind::Terminal => Content::Terminal,
            DocumentKind::Console => Content::Console,
            _ => {
                let editor = cx.new(|cx| CodeCell::new_document(window, cx));
                cx.subscribe(&editor, |_, _, event, cx| {
                    if matches!(event, crate::code_cell::CellEvent::Changed) {
                        cx.notify();
                    }
                })
                .detach();
                editor.read(cx).focus_handle(cx).focus(window, cx);
                Content::Text(editor)
            }
        };
        self.documents.push(Document {
            id: self.next_id,
            name: name.into(),
            path: None,
            kind,
            content,
        });
        self.tabs.launch(self.next_id);
        self.activate(self.next_id, window, cx);
    }
    fn refresh_files(&mut self, cx: &mut Context<Self>) {
        let paths: Vec<_> = self
            .expanded
            .iter()
            .cloned()
            .chain(std::iter::once(String::new()))
            .collect();
        for path in paths {
            self.refresh_directory(path, cx);
        }
    }
    fn file_toast(&self, title: &str, result: Result<String, String>, cx: &mut Context<Self>) {
        let (message, kind) = match result {
            Ok(message) => (message, crate::toast::ToastKind::Success),
            Err(message) => (message, crate::toast::ToastKind::Error),
        };
        self.toasts.update(cx, |host, cx| {
            host.push(
                crate::toast::Toast::new(title.to_owned())
                    .description(message)
                    .kind(kind),
                cx,
            );
        });
    }
    fn create_folder(&mut self, cx: &mut Context<Self>) {
        if self.creating_folder {
            return;
        }
        self.creating_folder = true;
        let runtime = cx.global::<Runtime>();
        let config = runtime.config.clone();
        let task = runtime.handle.spawn(async move {
            let client = JupyterRestClient::new(config?).map_err(|error| error.to_string())?;
            client
                .create_directory("")
                .await
                .map(|folder| folder.name)
                .map_err(|error| error.to_string())
        });
        cx.spawn(async move |this, cx| {
            let result = task
                .await
                .unwrap_or_else(|_| Err("Could not create folder".into()));
            this.update(cx, |this, cx| {
                this.creating_folder = false;
                this.file_toast(
                    if result.is_ok() {
                        "Folder created"
                    } else {
                        "Could not create folder"
                    },
                    result,
                    cx,
                );
                this.refresh_files(cx);
            })
            .ok();
        })
        .detach();
    }
    fn upload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.uploading {
            return;
        }
        self.uploading = true;
        let picker = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Upload".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let paths = match picker.await {
                Ok(Ok(Some(paths))) => paths,
                result => {
                    this.update(cx, |this, cx| {
                        this.uploading = false;
                        if !matches!(result, Ok(Ok(None))) {
                            this.file_toast(
                                "Could not open file picker",
                                Err("Please try again.".into()),
                                cx,
                            );
                        }
                    })
                    .ok();
                    return;
                }
            };
            let task = this.update(cx, |_, cx| {
                let runtime = cx.global::<Runtime>();
                let config = runtime.config.clone();
                runtime.handle.spawn(async move {
                    let client =
                        JupyterRestClient::new(config?).map_err(|error| error.to_string())?;
                    let mut results = Vec::new();
                    for path in paths {
                        let name = path
                            .file_name()
                            .and_then(|name| name.to_str())
                            .map(str::to_owned);
                        let Some(name) = name else {
                            results.push(Err("Filename is not valid UTF-8".into()));
                            continue;
                        };
                        let read = tokio::task::spawn_blocking(move || std::fs::read(path)).await;
                        let result = match read {
                            Ok(Ok(bytes)) => client
                                .upload_file(&name, &bytes)
                                .await
                                .map(|_| name.clone())
                                .map_err(|error| format!("{name}: {error}")),
                            _ => Err(format!("Could not read {name}")),
                        };
                        results.push(result);
                    }
                    Ok::<_, String>(results)
                })
            });
            let Ok(task) = task else {
                return;
            };
            let result = task.await.unwrap_or_else(|_| Err("Upload failed".into()));
            this.update(cx, |this, cx| {
                this.uploading = false;
                match result {
                    Ok(results) => {
                        for result in results {
                            this.file_toast(
                                if result.is_ok() {
                                    "File uploaded"
                                } else {
                                    "Upload failed"
                                },
                                result,
                                cx,
                            );
                        }
                    }
                    Err(error) => this.file_toast("Upload failed", Err(error), cx),
                }
                this.refresh_files(cx);
            })
            .ok();
        })
        .detach();
    }
    fn refresh_directory(&mut self, path: String, cx: &mut Context<Self>) {
        if !self.loading.insert(path.clone()) {
            return;
        }
        let runtime = cx.global::<Runtime>();
        let config = runtime.config.clone();
        let request_path = path.clone();
        let task = runtime.handle.spawn(async move {
            let client = JupyterRestClient::new(config?).map_err(|e| e.to_string())?;
            let listing = client
                .get_contents(&request_path, true)
                .await
                .map_err(|e| e.to_string())?;
            let mut entries: Vec<Contents> =
                serde_json::from_value(listing.content).map_err(|e| e.to_string())?;
            entries.sort_by_key(|entry| {
                (entry.content_type != "directory", entry.name.to_lowercase())
            });
            Ok::<_, String>(entries)
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.loading.remove(&path);
                match result {
                    Ok(Ok(entries)) => {
                        this.file_error = None;
                        this.directories.insert(path, entries);
                    }
                    Ok(Err(error)) => this.file_error = Some(error),
                    Err(_) => this.file_error = Some("Could not list files".into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
    fn open_notebook(&mut self, path: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(path) = &path {
            if let Some(doc) = self
                .documents
                .iter()
                .find(|doc| doc.path.as_ref() == Some(path))
            {
                self.activate(doc.id, window, cx);
                return;
            }
            if !self.opening.insert(path.clone()) {
                return;
            }
        } else {
            if self.creating {
                return;
            }
            self.creating = true;
        }
        self.file_error = None;
        let runtime = cx.global::<Runtime>();
        let config = runtime.config.clone();
        let requested = path.clone();
        let task = runtime.handle.spawn(async move {
            let config = config?;
            let path = match requested {
                Some(path) => path,
                None => {
                    JupyterRestClient::new(config.clone())
                        .map_err(|e| e.to_string())?
                        .create_notebook("")
                        .await
                        .map_err(|e| e.to_string())?
                        .path
                }
            };
            let controller = NotebookController::open(config, &path)
                .await
                .map_err(|e| e.to_string())?;
            Ok::<_, String>((path, controller))
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                if let Some(path) = path {
                    this.opening.remove(&path);
                } else {
                    this.creating = false;
                }
                match result {
                    Ok(Ok((path, controller))) => {
                        let session = Session::from_controller(controller, cx.global::<Runtime>());
                        let notebook =
                            cx.new(|cx| Notebook::new(session, this.toasts.clone(), window, cx));
                        cx.observe(&notebook, |_, _, cx| cx.notify()).detach();
                        this.next_id += 1;
                        this.documents.push(Document {
                            id: this.next_id,
                            name: path.rsplit('/').next().unwrap_or(&path).to_owned().into(),
                            path: Some(path),
                            kind: DocumentKind::Notebook,
                            content: Content::Notebook(notebook),
                        });
                        this.tabs.launch(this.next_id);
                        this.activate(this.next_id, window, cx);
                        this.refresh_directory(String::new(), cx);
                    }
                    Ok(Err(error)) => this.file_error = Some(error),
                    Err(_) => this.file_error = Some("Could not open notebook".into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
    fn activate(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        self.tabs.activate(id);
        self.tab_scroll
            .scroll_to_item(self.tabs.open.iter().position(|tab| *tab == id).unwrap());
        if let Some(doc) = self.documents.iter().find(|doc| doc.id == id) {
            match &doc.content {
                Content::Notebook(notebook) => {
                    notebook.update(cx, |notebook, cx| notebook.focus_selected(window, cx))
                }
                Content::Text(editor) => editor.read(cx).focus_handle(cx).focus(window, cx),
                _ => window.blur(),
            }
        } else {
            window.blur();
        }
        cx.notify();
    }
    pub fn open_launcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.activate(LAUNCHER, window, cx);
    }
    pub fn close_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close(self.tabs.active, window, cx);
    }
    fn close(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        self.request_close(CloseTarget::Tab(id), window, cx);
    }
    fn affected(&self, target: CloseTarget) -> impl Iterator<Item = &Document> {
        self.documents.iter().filter(move |doc| match target {
            CloseTarget::Tab(id) => doc.id == id,
            _ => true,
        })
    }
    fn suspend_autosave(&self, target: CloseTarget, suspended: bool, cx: &mut Context<Self>) {
        for doc in self.affected(target) {
            if let Content::Notebook(notebook) = &doc.content {
                notebook.update(cx, |notebook, _| notebook.suspend_autosave(suspended));
            }
        }
    }
    pub fn request_close(
        &mut self,
        target: CloseTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.close_pending || window.has_active_prompt() {
            return;
        }
        let dirty = self.affected(target).any(|doc| match &doc.content {
            Content::Notebook(notebook) => {
                notebook.read(cx).is_dirty(cx) || notebook.read(cx).is_saving()
            }
            Content::Text(editor) => !editor.read(cx).text().is_empty(),
            _ => false,
        });
        if !dirty {
            self.complete_close(target, window, cx);
            return;
        }
        let unsaved_text = self.affected(target).any(|doc| matches!(&doc.content, Content::Text(editor) if !editor.read(cx).text().is_empty()));
        let quitting = matches!(target, CloseTarget::Quit);
        let title = if quitting {
            "Save changes before quitting?"
        } else {
            "Save changes before closing?"
        };
        let detail = if unsaved_text {
            "Text-file saving isn't available yet. Cancel to copy your text. Discarding will lose all unsaved changes."
        } else {
            "Your unsaved notebook changes will be lost if you discard them."
        };
        let mut choices = vec!["Cancel", "Discard changes"];
        if !unsaved_text {
            choices.push(if quitting {
                "Save and quit"
            } else {
                "Save and close"
            });
        }
        self.close_pending = true;
        self.suspend_autosave(target, true, cx);
        let response = window.prompt(
            gpui::PromptLevel::Warning,
            title,
            Some(detail),
            &choices,
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let response = response.await;
            this.update_in(cx, |this, window, cx| match response {
                Ok(1) => this.complete_close(target, window, cx),
                Ok(2) => this.save_and_close(target, window, cx),
                _ => {
                    this.close_pending = false;
                    this.suspend_autosave(target, false, cx);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }
    fn save_and_close(&mut self, target: CloseTarget, window: &mut Window, cx: &mut Context<Self>) {
        let notebooks: Vec<_> = self
            .affected(target)
            .filter_map(|doc| match &doc.content {
                Content::Notebook(notebook)
                    if notebook.read(cx).is_dirty(cx) || notebook.read(cx).is_saving() =>
                {
                    Some(notebook.clone())
                }
                _ => None,
            })
            .collect();
        for notebook in &notebooks {
            notebook.update(cx, |notebook, cx| notebook.save(cx));
        }
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(std::time::Duration::from_millis(50)).await;
                let finished = this.update_in(cx, |this, window, cx| {
                    if notebooks.iter().any(|notebook| notebook.read(cx).is_saving()) { return false; }
                    if notebooks.iter().any(|notebook| notebook.read(cx).save_failed()) || this.affected(target).any(|doc| match &doc.content {
                        Content::Notebook(notebook) => notebook.read(cx).is_dirty(cx) || notebook.read(cx).is_saving(),
                        Content::Text(editor) => !editor.read(cx).text().is_empty(),
                        _ => false,
                    }) {
                        this.close_pending = false;
                        this.suspend_autosave(target, false, cx);
                        if !notebooks.iter().any(|notebook| notebook.read(cx).save_failed()) {
                            this.toasts.update(cx, |host, cx| { host.push(crate::toast::Toast::new("New changes arrived while saving")
                                .description("Your notebook is still open. Save again before closing.")
                                .kind(crate::toast::ToastKind::Info), cx); });
                        }
                        cx.notify();
                    } else { this.complete_close(target, window, cx); }
                    true
                });
                if finished.unwrap_or(true) { break; }
            }
        }).detach();
    }
    fn complete_close(&mut self, target: CloseTarget, window: &mut Window, cx: &mut Context<Self>) {
        self.close_pending = false;
        match target {
            CloseTarget::Tab(id) => self.finish_close(id, window, cx),
            CloseTarget::Window => window.remove_window(),
            CloseTarget::Quit => cx.quit(),
        }
    }
    fn finish_close(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        // Closing a file never shuts down its server kernel.
        self.tabs.close(id);
        self.documents.retain(|doc| doc.id != id);
        self.activate(self.tabs.active, window, cx);
    }
    pub fn header(entity: &Entity<Self>, cx: &App) -> gpui::Div {
        let this = entity.read(cx);
        let t = cx.theme();
        div()
            .flex()
            .flex_1()
            .min_w_0()
            .items_center()
            .gap_1()
            .ml_3()
            .mr_2()
            .child(
                div()
                    .id("workspace-tabs")
                    .flex()
                    .min_w_0()
                    .gap_1()
                    .overflow_x_scroll()
                    .track_scroll(&this.tab_scroll)
                    .children(this.tabs.open.iter().map(|id| {
                        let id = *id;
                        let doc = this.documents.iter().find(|doc| doc.id == id);
                        let selected = this.tabs.active == id;
                        let name = doc
                            .map(|doc| doc.name.clone())
                            .unwrap_or_else(|| "Launcher".into());
                        let icon = doc.map(|doc| doc.kind.icon()).unwrap_or(Icon::SquaresFour);
                        let (dirty, running) = match doc.map(|doc| &doc.content) {
                            Some(Content::Notebook(notebook)) => (
                                notebook.read(cx).is_dirty(cx),
                                notebook.read(cx).running_spinner(cx),
                            ),
                            Some(Content::Text(editor)) => {
                                (!editor.read(cx).text().is_empty(), None)
                            }
                            _ => (false, None),
                        };
                        let fg = if selected {
                            t.app.fg
                        } else {
                            t.sidebar.muted_fg
                        };
                        let select = entity.clone();
                        let select_key = entity.clone();
                        let close = entity.clone();
                        let close_key = entity.clone();
                        let leading = if let Some(glyph) = running {
                            div()
                                .size_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .font_family(t.font.mono.clone())
                                .text_size(t.font.sm)
                                .text_color(fg)
                                .child(glyph.to_string())
                                .into_any_element()
                        } else {
                            icon.svg().size(px(14.)).text_color(fg).into_any_element()
                        };
                        div()
                            .id(("document-tab", id))
                            .tab_index(0)
                            .h(px(28.))
                            .w(px(180.))
                            .flex_shrink_0()
                            .px_2()
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .rounded(t.radius.md())
                            .text_size(t.font.xs)
                            .cursor_pointer()
                            .when(selected, |s| {
                                // Inset shadows use this element's rounded bounds; clipping
                                // also keeps tab content inside the same silhouette.
                                s.overflow_hidden()
                                    .bg(t.app.bg)
                                    .text_color(t.app.fg)
                                    .shadow(vec![
                                        t.body_shadow.clone(),
                                        t.active_tab_edges[0].clone(),
                                        t.active_tab_edges[1].clone(),
                                    ])
                            })
                            .when(!selected, |s| {
                                s.text_color(t.sidebar.muted_fg)
                                    .hover(|s| s.bg(t.rail_hover_bg))
                            })
                            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
                                cx.stop_propagation()
                            })
                            .on_click(move |_, window, cx| {
                                cx.stop_propagation();
                                select.update(cx, |this, cx| this.activate(id, window, cx));
                            })
                            .on_key_down(move |event, window, cx| {
                                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                                    select_key.update(cx, |this, cx| this.activate(id, window, cx));
                                    cx.stop_propagation();
                                }
                            })
                            .child(div().size(px(14.)).flex_shrink_0().child(leading))
                            .child(div().flex_1().min_w_0().truncate().child(name))
                            .child(
                                div()
                                    .id(("close-tab", id))
                                    .group("tab-close")
                                    .aria_label(if dirty {
                                        "Close tab — unsaved changes"
                                    } else {
                                        "Close tab"
                                    })
                                    .tab_index(0)
                                    .size(px(18.))
                                    .flex_shrink_0()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(t.radius.sm())
                                    .cursor_pointer()
                                    .hover(|s| s.bg(t.rail_hover_bg))
                                    .on_click(move |_, window, cx| {
                                        cx.stop_propagation();
                                        close.update(cx, |this, cx| this.close(id, window, cx));
                                    })
                                    .on_key_down(move |event, window, cx| {
                                        if matches!(event.keystroke.key.as_str(), "enter" | "space")
                                        {
                                            close_key
                                                .update(cx, |this, cx| this.close(id, window, cx));
                                            cx.stop_propagation();
                                        }
                                    })
                                    .child(if dirty {
                                        div()
                                            .size(px(6.))
                                            .rounded_full()
                                            .bg(fg)
                                            .text_color(t.app.bg)
                                            .into_any_element()
                                    } else {
                                        Icon::X
                                            .svg()
                                            .size(px(12.))
                                            .text_color(fg)
                                            .into_any_element()
                                    }),
                            )
                    })),
            )
            .child({
                let click = entity.clone();
                let key = entity.clone();
                icon_button("new-tab", "New tab", Icon::Plus, cx)
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(move |_, window, cx| {
                        cx.stop_propagation();
                        click.update(cx, |this, cx| this.open_launcher(window, cx));
                    })
                    .on_key_down(move |event, window, cx| {
                        if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                            key.update(cx, |this, cx| this.open_launcher(window, cx));
                            cx.stop_propagation();
                        }
                    })
            })
    }
    fn file_action(&mut self, action: FileAction, window: &mut Window, cx: &mut Context<Self>) {
        match action {
            FileAction::Launcher => self.open_launcher(window, cx),
            FileAction::Folder => self.create_folder(cx),
            FileAction::Filter => self.notebooks_only = !self.notebooks_only,
            FileAction::Menu => {
                self.files_menu_open = !self.files_menu_open;
                self.files_menu_index = None;
                self.files_menu_focus.focus(window, cx);
            }
            FileAction::Upload => {
                self.files_menu_open = false;
                self.upload(window, cx);
            }
            FileAction::Refresh => {
                self.files_menu_open = false;
                self.refresh_files(cx);
            }
        }
        cx.notify();
    }
    fn files_toolbar(entity: &Entity<Self>, cx: &App) -> gpui::Div {
        let this = entity.read(cx);
        let t = cx.theme();
        div()
            .flex()
            .items_center()
            .justify_center()
            .gap(px(4.))
            .flex_shrink_0()
            .children(
                [
                    (
                        FileAction::Launcher,
                        "files-launcher",
                        "New launcher",
                        Icon::Plus,
                    ),
                    (
                        FileAction::Folder,
                        "files-folder",
                        "New folder in server root",
                        Icon::FolderPlus,
                    ),
                    (
                        FileAction::Filter,
                        "files-filter",
                        "Filter: notebooks only",
                        Icon::Filter,
                    ),
                    (
                        FileAction::Menu,
                        "files-menu-button",
                        "More file actions",
                        Icon::DotsThree,
                    ),
                ]
                .into_iter()
                .map(|(action, id, label, icon)| {
                    let click = entity.clone();
                    let key = entity.clone();
                    let outside = entity.clone();
                    let selected = matches!(action, FileAction::Filter) && this.notebooks_only;
                    let fg = if selected {
                        t.active_file_fg
                    } else {
                        t.sidebar_content.fg
                    };
                    let button = div()
                        .id(id)
                        .aria_label(label)
                        .tab_index(0)
                        .size(px(26.))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(t.radius.sm())
                        .text_color(fg)
                        .cursor_pointer()
                        .hover(|view| view.bg(t.rail_hover_bg))
                        .focus_visible(|view| view.border_1().border_color(t.sidebar_content.ring))
                        .tooltip(move |_, cx| cx.new(|_| Tip(label)).into())
                        .child(icon.svg().size(px(14.)).text_color(fg))
                        .when(selected, |button| {
                            button.bg(t.active_file_bg).text_color(t.active_file_fg)
                        })
                        .when(matches!(action, FileAction::Menu), |button| {
                            button.track_focus(&this.files_menu_focus)
                        })
                        .on_click(move |_, window, cx| {
                            click.update(cx, |this, cx| this.file_action(action, window, cx))
                        })
                        .on_key_down(move |event, window, cx| {
                            key.update(cx, |this, cx| {
                                if matches!(action, FileAction::Menu) && this.files_menu_open {
                                    match event.keystroke.key.as_str() {
                                        "up" | "down" => {
                                            this.files_menu_index =
                                                Some(match this.files_menu_index {
                                                    Some(index) => 1 - index,
                                                    None if event.keystroke.key == "up" => 1,
                                                    None => 0,
                                                })
                                        }
                                        "enter" | "space" => this.file_action(
                                            if this.files_menu_index.unwrap_or(0) == 0 {
                                                FileAction::Upload
                                            } else {
                                                FileAction::Refresh
                                            },
                                            window,
                                            cx,
                                        ),
                                        "escape" | "tab" => this.files_menu_open = false,
                                        _ => return,
                                    }
                                    cx.notify();
                                } else if matches!(event.keystroke.key.as_str(), "enter" | "space")
                                {
                                    this.file_action(action, window, cx);
                                    if matches!(action, FileAction::Menu) {
                                        this.files_menu_index = Some(0);
                                    }
                                }
                                if event.keystroke.key != "tab" {
                                    cx.stop_propagation();
                                }
                            });
                        });
                    div()
                        .id(SharedString::from(format!("{id}-control")))
                        .relative()
                        .when(matches!(action, FileAction::Menu), |view| {
                            view.on_mouse_down_out(move |_, _, cx| {
                                outside.update(cx, |this, cx| {
                                    if this.files_menu_open {
                                        this.files_menu_open = false;
                                        cx.notify();
                                    }
                                });
                            })
                        })
                        .child(button)
                        .when(
                            matches!(action, FileAction::Menu) && this.files_menu_open,
                            |view| {
                                view.child(gpui::deferred(
                                    gpui::anchored()
                                        .offset(gpui::point(px(0.), px(4.)))
                                        .snap_to_window_with_margin(px(8.))
                                        .child(
                                            div()
                                                .id("files-menu")
                                                .occlude()
                                                .w(px(150.))
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
                                                .children(
                                                    [
                                                        (
                                                            FileAction::Upload,
                                                            "Upload…",
                                                            Icon::Upload,
                                                        ),
                                                        (
                                                            FileAction::Refresh,
                                                            "Refresh",
                                                            Icon::Restart,
                                                        ),
                                                    ]
                                                    .into_iter()
                                                    .enumerate()
                                                    .map(|(index, (action, label, icon))| {
                                                        let entity = entity.clone();
                                                        let hover = entity.clone();
                                                        div()
                                                            .id(("files-menu-item", index))
                                                            .h(px(26.))
                                                            .px_2()
                                                            .flex()
                                                            .items_center()
                                                            .gap_2()
                                                            .rounded(t.radius.sm())
                                                            .text_size(t.font.xs)
                                                            .cursor_pointer()
                                                            .when(
                                                                this.files_menu_index
                                                                    == Some(index),
                                                                |view| {
                                                                    view.bg(t.popover.accent)
                                                                        .text_color(
                                                                            t.popover.accent_fg,
                                                                        )
                                                                },
                                                            )
                                                            .hover(|view| {
                                                                view.bg(t.popover.accent)
                                                                    .text_color(t.popover.accent_fg)
                                                            })
                                                            .child(
                                                                icon.svg()
                                                                    .size(px(14.))
                                                                    .text_color(t.popover.fg),
                                                            )
                                                            .child(label)
                                                            .on_mouse_move(move |_, _, cx| {
                                                                hover.update(cx, |this, cx| {
                                                                    if this
                                                                        .files_menu_index
                                                                        .take()
                                                                        .is_some()
                                                                    {
                                                                        cx.notify();
                                                                    }
                                                                });
                                                            })
                                                            .on_click(move |_, window, cx| {
                                                                cx.stop_propagation();
                                                                entity.update(cx, |this, cx| {
                                                                    this.file_action(
                                                                        action, window, cx,
                                                                    )
                                                                });
                                                            })
                                                    }),
                                                ),
                                        ),
                                ))
                            },
                        )
                }),
            )
    }
    pub fn files(entity: &Entity<Self>, cx: &App) -> gpui::Div {
        let t = cx.theme();
        let this = entity.read(cx);
        let mut rows = Vec::new();
        this.tree_rows("", 0, &mut rows);
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(Self::files_toolbar(entity, cx))
            .when_some(this.file_error.as_ref(), |view, error| {
                view.child(
                    div()
                        .text_size(t.font.xs)
                        .text_color(t.destructive)
                        .child(error.clone()),
                )
            })
            .child(
                div()
                    .id("server-file-tree")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .children(rows.into_iter().map(|(entry, depth)| {
                        let directory = entry.content_type == "directory";
                        let supported = directory || entry.content_type == "notebook";
                        let selected = this.documents.iter().any(|doc| {
                            doc.id == this.tabs.active && doc.path.as_ref() == Some(&entry.path)
                        });
                        let icon = if directory {
                            if this.expanded.contains(&entry.path) {
                                Icon::FolderOpen
                            } else {
                                Icon::Folder
                            }
                        } else {
                            Icon::FileText
                        };
                        let entity = entity.clone();
                        div()
                            .id(SharedString::from(format!("file-{}", entry.path)))
                            .h(px(24.))
                            .flex_shrink_0()
                            .pl(px(4. + depth as f32 * 10.))
                            .pr_2()
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .rounded(t.radius.md())
                            .text_size(t.font.xs)
                            .when(selected, |s| {
                                s.bg(t.active_file_bg).text_color(t.active_file_fg)
                            })
                            .when(!selected, |s| {
                                s.text_color(t.sidebar_content.fg)
                                    .hover(|s| s.bg(t.sidebar_content.accent))
                            })
                            .when(supported, |s| s.cursor_pointer())
                            .when(!supported, |s| s.opacity(0.5))
                            .child(icon.svg().size(px(14.)).flex_shrink_0().text_color(
                                if selected {
                                    t.active_file_fg
                                } else {
                                    t.sidebar_content.fg
                                },
                            ))
                            .child(div().truncate().child(entry.name))
                            .on_click(move |_, window, cx| {
                                entity.update(cx, |this, cx| {
                                    if directory {
                                        if !this.expanded.remove(&entry.path) {
                                            this.expanded.insert(entry.path.clone());
                                            this.refresh_directory(entry.path.clone(), cx);
                                        }
                                        cx.notify();
                                    } else if supported {
                                        this.open_notebook(Some(entry.path.clone()), window, cx);
                                    }
                                });
                            })
                    })),
            )
    }
    fn tree_rows(&self, path: &str, depth: usize, rows: &mut Vec<(Contents, usize)>) {
        if let Some(entries) = self.directories.get(path) {
            for entry in entries {
                if self.notebooks_only
                    && entry.content_type != "directory"
                    && entry.content_type != "notebook"
                {
                    continue;
                }
                rows.push((entry.clone(), depth));
                if self.expanded.contains(&entry.path) {
                    self.tree_rows(&entry.path, depth + 1, rows);
                }
            }
        }
    }
    fn launcher(&self, cx: &Context<Self>) -> gpui::Stateful<gpui::Div> {
        let t = cx.theme();
        let groups = [
            (
                "Notebook",
                vec![(DocumentKind::Notebook, "Python 3", "ipykernel")],
            ),
            (
                "Console",
                vec![(DocumentKind::Console, "Python 3", "ipykernel")],
            ),
            (
                "Other",
                vec![
                    (DocumentKind::Terminal, "Terminal", ""),
                    (DocumentKind::Text, "Text file", ".txt"),
                    (DocumentKind::Markdown, "Markdown file", ".md"),
                    (DocumentKind::Python, "Python file", ".py"),
                ],
            ),
        ];
        div()
            .id("launcher")
            .size_full()
            .overflow_y_scroll()
            .p_5()
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap_6()
                    .children(groups.into_iter().map(|(heading, items)| {
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .text_size(t.font.sm)
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(heading),
                            )
                            .child(div().grid().grid_cols(2).gap_2().children(
                                items.into_iter().map(|(kind, label, detail)| {
                                    div()
                                        .id(SharedString::from(format!("{heading}-{label}")))
                                        .tab_index(0)
                                        .h(px(40.))
                                        .px_3()
                                        .min_w_0()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .rounded(t.radius.lg())
                                        .bg(t.sidebar_content.bg)
                                        .text_color(t.sidebar_content.fg)
                                        .cursor_pointer()
                                        .hover(|s| s.bg(t.rail_hover_bg))
                                        .focus(|s| s.border_1().border_color(t.app.ring))
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.create(kind, window, cx)
                                        }))
                                        .on_key_down(cx.listener(
                                            move |this, event: &gpui::KeyDownEvent, window, cx| {
                                                if matches!(
                                                    event.keystroke.key.as_str(),
                                                    "enter" | "space"
                                                ) {
                                                    this.create(kind, window, cx);
                                                    cx.stop_propagation();
                                                }
                                            },
                                        ))
                                        .child(
                                            kind.icon()
                                                .svg()
                                                .size(px(16.))
                                                .flex_shrink_0()
                                                .text_color(t.sidebar_content.muted_fg),
                                        )
                                        .child(div().text_size(t.font.sm).truncate().child(label))
                                        .child(div().flex_1())
                                        .when(!detail.is_empty(), |el| {
                                            el.child(
                                                div()
                                                    .text_size(t.font.xs)
                                                    .text_color(t.sidebar_content.muted_fg)
                                                    .child(detail),
                                            )
                                        })
                                }),
                            ))
                    })),
            )
    }
}
impl Render for Workspace {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme();
        let content = self.documents.iter().find(|doc| doc.id == self.tabs.active);
        div()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .child(match content {
                Some(Document {
                    content: Content::Notebook(notebook),
                    ..
                }) => notebook.clone().into_any_element(),
                Some(Document {
                    content: Content::Text(editor),
                    ..
                }) => div()
                    .flex_1()
                    .min_h_0()
                    .p_3()
                    .child(editor.clone())
                    .into_any_element(),
                Some(Document {
                    content: Content::Terminal | Content::Console,
                    ..
                }) => div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .child(
                        Icon::Terminal
                            .svg()
                            .size(px(24.))
                            .text_color(t.app.muted_fg),
                    )
                    .child(div().text_size(t.font.sm).child("No server connected"))
                    .child(
                        div()
                            .text_size(t.font.xs)
                            .text_color(t.app.muted_fg)
                            .child("Connect to a Jupyter server to start a terminal."),
                    )
                    .into_any_element(),
                None => self.launcher(cx).into_any_element(),
            })
    }
}
pub fn icon_button(
    id: impl Into<gpui::ElementId>,
    label: &'static str,
    icon: Icon,
    cx: &App,
) -> gpui::Stateful<gpui::Div> {
    let t = cx.theme();
    div()
        .id(id)
        .aria_label(label)
        .tab_index(0)
        .size(px(26.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(t.radius.sm())
        .text_color(t.app.fg)
        .cursor_pointer()
        .hover(|s| s.bg(t.rail_hover_bg))
        .active(|s| s.bg(t.sidebar.accent))
        .focus(|s| s.border_1().border_color(t.app.ring))
        .tooltip(move |_, cx| cx.new(|_| Tip(label)).into())
        .child(icon.svg().size(px(14.)).text_color(t.app.fg))
}
struct Tip(&'static str);
impl Render for Tip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme();
        div()
            .px_2()
            .py_1()
            .rounded(t.radius.sm())
            .bg(t.popover.bg)
            .text_color(t.popover.fg)
            .border_1()
            .border_color(t.popover.border)
            .text_size(t.font.xs)
            .child(self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn launcher_is_singleton_and_remains_open_when_switching_tabs() {
        let mut tabs = Tabs::default();
        tabs.activate(1);
        tabs.activate(LAUNCHER);
        tabs.activate(LAUNCHER);
        assert_eq!(tabs.open, vec![LAUNCHER, 1]);
        tabs.activate(1);
        assert!(tabs.open.contains(&LAUNCHER));
    }
    #[test]
    fn launching_replaces_launcher_in_place_and_selects_new_tab() {
        let mut tabs = Tabs::default();
        tabs.activate(1);
        tabs.activate(LAUNCHER);
        tabs.launch(2);
        assert_eq!(tabs.open, vec![2, 1]);
        assert_eq!(tabs.active, 2);
        tabs.close(1);
        tabs.close(2);
        assert_eq!(tabs.open, vec![LAUNCHER]);
    }
    #[test]
    fn closing_last_document_creates_launcher() {
        let mut tabs = Tabs::default();
        tabs.activate(1);
        tabs.close(LAUNCHER);
        tabs.close(1);
        assert_eq!(tabs.open, vec![LAUNCHER]);
        assert_eq!(tabs.active, LAUNCHER);
    }
    #[test]
    fn closing_only_launcher_recreates_it() {
        let mut tabs = Tabs::default();
        tabs.close(LAUNCHER);
        assert_eq!(tabs.open, vec![LAUNCHER]);
        assert_eq!(tabs.active, LAUNCHER);
    }
    #[test]
    fn closing_launcher_selects_another_tab_without_recreating_it() {
        let mut tabs = Tabs::default();
        tabs.activate(1);
        tabs.activate(LAUNCHER);
        tabs.close(LAUNCHER);
        assert_eq!(tabs.open, vec![1]);
        assert_eq!(tabs.active, 1);
    }
    #[test]
    fn closing_background_tab_keeps_selection() {
        let mut tabs = Tabs::default();
        tabs.activate(1);
        tabs.activate(2);
        tabs.close(1);
        assert_eq!(tabs.active, 2);
    }
}
