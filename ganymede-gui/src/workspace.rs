//! Server file browser, notebook tabs, and launcher.
mod file_management;
mod kernels;
mod persistence;
mod selection;
mod server;
use crate::execution::{Runtime, Session};
use crate::{
    code_cell::CodeCell,
    icon::Icon,
    notebook::Notebook,
    text_file::TextFile,
    theme::{ActiveTheme, Theme},
};
use file_management::{DragFile, FileMenu, Operation};
use ganymede_core::{
    jupyter::{JupyterRestClient, models::Contents},
    notebook::NotebookController,
};
use gpui::{
    App, Context, Entity, Focusable, FontWeight, ScrollHandle, SharedString, Window, div,
    prelude::*, px,
};
use selection::{FileSelection, selection_roots};
use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

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
    fn for_text_path(path: &str) -> Self {
        match path
            .rsplit('.')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str()
        {
            "py" | "pyw" | "pyi" => Self::Python,
            "md" | "markdown" => Self::Markdown,
            _ => Self::Text,
        }
    }
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
#[derive(Clone)]
enum Content {
    Notebook(Entity<Notebook>),
    Text(Entity<TextFile>),
    Terminal,
    Console,
}
impl Content {
    fn is_dirty(&self, cx: &App) -> bool {
        match self {
            Self::Notebook(view) => view.read(cx).is_dirty(cx),
            Self::Text(view) => view.read(cx).is_dirty(),
            _ => false,
        }
    }
    fn is_saving(&self, cx: &App) -> bool {
        match self {
            Self::Notebook(view) => view.read(cx).is_saving(),
            Self::Text(view) => view.read(cx).is_saving(),
            _ => false,
        }
    }
    fn save_failed(&self, cx: &App) -> bool {
        match self {
            Self::Notebook(view) => view.read(cx).save_failed(),
            Self::Text(view) => view.read(cx).save_failed(),
            _ => false,
        }
    }
    fn save(&self, window: &mut Window, cx: &mut App) {
        match self {
            Self::Notebook(view) => view.update(cx, |view, cx| view.save(window, cx)),
            Self::Text(view) => view.update(cx, |view, cx| view.save(window, cx)),
            _ => (),
        }
    }
    fn suspend_autosave(&self, suspended: bool, cx: &mut App) {
        match self {
            Self::Notebook(view) => view.update(cx, |view, _| view.suspend_autosave(suspended)),
            Self::Text(view) => view.update(cx, |view, _| view.suspend_autosave(suspended)),
            _ => (),
        }
    }
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
const TAB_MOTION_DURATION: Duration = Duration::from_millis(180);

struct TabMotion {
    from: f32,
    to: f32,
    started: Instant,
}
impl TabMotion {
    fn value(&self, now: Instant) -> f32 {
        let progress = (now.duration_since(self.started).as_secs_f32()
            / TAB_MOTION_DURATION.as_secs_f32())
        .min(1.);
        let eased = if self.to == 0. {
            progress * progress
        } else {
            1. - (1. - progress).powi(3)
        };
        self.from + (self.to - self.from) * eased
    }
    fn set_open(&mut self, open: bool, now: Instant) {
        let target = if open { 1. } else { 0. };
        if target != self.to {
            self.from = self.value(now);
            self.to = target;
            self.started = now;
        }
    }
    fn active(&self, now: Instant) -> bool {
        self.from != self.to && now.duration_since(self.started) < TAB_MOTION_DURATION
    }
}

// Retain only presentation data while a closed tab leaves the header. Its
// document and autosave lifecycle end immediately when the close is accepted.
struct HeaderTab {
    id: u64,
    name: SharedString,
    icon: Icon,
    selected: bool,
    motion: TabMotion,
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
    runtime: Runtime,
    saved_layout: persistence::Layout,
    last_written: Option<persistence::Layout>,
    unrestored: Vec<(usize, persistence::SavedTab)>,
    restoring: bool,
    toasts: Entity<crate::toast::ToastHost>,
    documents: Vec<Document>,
    tabs: Tabs,
    header_tabs: Vec<HeaderTab>,
    reveal_tab: Option<u64>,
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
    filter_open: bool,
    filter_input: Entity<CodeCell>,
    filter_query: String,
    filter_requested: HashSet<String>,
    uploading: bool,
    file_busy: bool,
    file_directory: String,
    file_selection: FileSelection,
    file_operations: std::collections::VecDeque<Operation>,
    file_menu: Option<FileMenu>,
    file_menu_focus: gpui::FocusHandle,
    tree_generation: u64,
    kernels: kernels::KernelList,
    server: server::Server,
}
impl Workspace {
    pub fn new(
        runtime: Runtime,
        toasts: Entity<crate::toast::ToastHost>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe_global::<Theme>(|_, cx| cx.notify()).detach();
        let filter_input = cx.new(|cx| CodeCell::new_filter(window, cx));
        cx.subscribe_in(
            &filter_input,
            window,
            |this, input, event, window, cx| match event {
                crate::code_cell::CellEvent::Changed => {
                    this.filter_query = input.read(cx).text().trim().to_lowercase();
                    this.scan_filter_folders(cx);
                    cx.notify();
                }
                crate::code_cell::CellEvent::DeleteCell => this.close_filter(window, cx),
                _ => (),
            },
        )
        .detach();
        let saved_layout = Self::load_layout(&runtime);
        let mut this = Self {
            saved_layout,
            last_written: None,
            unrestored: Vec::new(),
            restoring: false,
            runtime,
            toasts,
            documents: vec![],
            tabs: Tabs::default(),
            header_tabs: Vec::new(),
            reveal_tab: None,
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
            filter_open: false,
            filter_input,
            filter_query: String::new(),
            filter_requested: HashSet::new(),
            uploading: false,
            file_busy: false,
            file_directory: String::new(),
            file_selection: FileSelection::default(),
            file_operations: Default::default(),
            file_menu: None,
            file_menu_focus: cx.focus_handle(),
            tree_generation: 0,
            kernels: Default::default(),
            server: server::Server::new(cx),
        };
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(10))
                    .await;
                if this
                    .update(cx, |this, cx| {
                        this.check_server(false, cx);
                        this.refresh_files(cx);
                        this.refresh_kernels(false, cx);
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        this.check_server(false, cx);
        this.refresh_kernels(false, cx);
        this.refresh_directory(String::new(), cx);
        this.open_launcher(window, cx);
        this.begin_restore(window, cx);
        this
    }
    fn create(&mut self, kind: DocumentKind, window: &mut Window, cx: &mut Context<Self>) {
        if self.file_busy {
            return;
        }
        if kind == DocumentKind::Notebook {
            self.open_notebook(None, window, cx);
            return;
        }
        if matches!(
            kind,
            DocumentKind::Text | DocumentKind::Markdown | DocumentKind::Python
        ) {
            self.open_text(None, kind, window, cx);
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
            _ => unreachable!(),
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
    fn close_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.filter_open = false;
        self.filter_query.clear();
        self.filter_requested.clear();
        self.filter_input
            .update(cx, |input, cx| input.set_text(String::new(), cx));
        window.blur();
        cx.notify();
    }
    fn scan_filter_folders(&mut self, cx: &mut Context<Self>) {
        if !self.filter_open || self.filter_query.is_empty() {
            return;
        }
        // Keep remote directory requests bounded. Search also visits collapsed
        // folders, without modifying the user's normal expanded-folder state.
        let slots = 4usize.saturating_sub(self.loading.len());
        let remaining = 1024usize.saturating_sub(self.filter_requested.len());
        let candidates: Vec<_> = std::iter::once(String::new())
            .chain(
                self.directories
                    .values()
                    .flat_map(|entries| entries.iter())
                    .filter(|entry| {
                        entry.content_type == "directory" && entry.path.split('/').count() < 32
                    })
                    .map(|entry| entry.path.clone()),
            )
            .filter(|path| !self.filter_requested.contains(path) && !self.loading.contains(path))
            .take(slots.min(remaining))
            .collect();
        for path in candidates {
            self.filter_requested.insert(path.clone());
            self.refresh_directory(path, cx);
        }
    }
    fn refresh_files(&mut self, cx: &mut Context<Self>) {
        if self.file_busy {
            return;
        }
        if self.filter_open && !self.filter_query.is_empty() {
            // Let an in-progress scan finish instead of restarting it on every timer tick.
            if !self.loading.is_empty() {
                return;
            }
            self.filter_requested.clear();
            self.scan_filter_folders(cx);
            return;
        }
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
    fn upload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.file_busy {
            return;
        }
        if self.uploading {
            return;
        }
        self.uploading = true;
        if !self.file_directory.is_empty() {
            self.expanded.insert(self.file_directory.clone());
        }
        let directory = self.file_directory.clone();
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
            let task = this.update(cx, |this, _| {
                let runtime = &this.runtime;
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
                                .upload_file(
                                    &ganymede_core::files::child_path(&directory, &name)
                                        .map_err(|e| e.to_string())?,
                                    &bytes,
                                )
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
        let generation = self.tree_generation;
        let runtime = &self.runtime;
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
            entries.sort_by_key(|entry| (entry.content_type != "directory", entry.name.clone()));
            Ok::<_, String>(entries)
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                if this.tree_generation != generation {
                    return;
                }
                this.loading.remove(&path);
                match result {
                    Ok(Ok(entries)) => {
                        this.file_error = None;
                        this.directories.insert(path, entries);
                    }
                    Ok(Err(error)) => this.file_error = Some(error),
                    Err(_) => this.file_error = Some("Could not list files".into()),
                }
                this.scan_filter_folders(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
    fn open_text(
        &mut self,
        path: Option<String>,
        kind: DocumentKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.file_busy {
            return;
        }
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
        let runtime = &self.runtime;
        let config = runtime.config.clone();
        let requested = path.clone();
        let directory = self.file_directory.clone();
        let task = runtime.handle.spawn(async move {
            let config = config?;
            let path = match requested {
                Some(path) => path,
                None => {
                    JupyterRestClient::new(config.clone())
                        .map_err(|e| e.to_string())?
                        .create_text_file(&directory, &format!(".{}", kind.extension()))
                        .await
                        .map_err(|e| e.to_string())?
                        .path
                }
            };
            let controller = ganymede_core::text_file::TextFileController::open(config, &path)
                .await
                .map_err(|e| e.to_string())?;
            Ok::<_, String>((path, controller))
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                let preserve_selection = path.is_some();
                if let Some(path) = path {
                    this.opening.remove(&path);
                } else {
                    this.creating = false;
                }
                match result {
                    Ok(Ok((path, controller))) => {
                        this.unrestored
                            .retain(|(_, tab)| tab.path.as_ref() != Some(&path));
                        this.reveal_file_parent(&path);
                        let editor =
                            cx.new(|cx| TextFile::new(controller, this.toasts.clone(), window, cx));
                        editor.update(cx, |view, _| view.set_file_busy(this.file_busy));
                        cx.observe(&editor, |_, _, cx| cx.notify()).detach();
                        this.next_id += 1;
                        this.documents.push(Document {
                            id: this.next_id,
                            name: path.rsplit('/').next().unwrap_or(&path).to_owned().into(),
                            kind: DocumentKind::for_text_path(&path),
                            path: Some(path),
                            content: Content::Text(editor),
                        });
                        this.tabs.launch(this.next_id);
                        let selection = this.file_selection.clone();
                        let directory = this.file_directory.clone();
                        this.activate(this.next_id, window, cx);
                        if preserve_selection {
                            this.file_selection = selection;
                            this.file_directory = directory;
                        }
                        this.refresh_files(cx);
                    }
                    Ok(Err(error)) => this.file_toast("Could not open file", Err(error), cx),
                    Err(_) => this.file_toast(
                        "Could not open file",
                        Err("The file request failed".into()),
                        cx,
                    ),
                }
                this.finish_restore(window, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
    fn open_notebook(&mut self, path: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        if self.file_busy {
            return;
        }
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
        let runtime = &self.runtime;
        let config = runtime.config.clone();
        let restoring = self.restoring;
        let requested = path.clone();
        let directory = self.file_directory.clone();
        let task = runtime.handle.spawn(async move {
            let config = config?;
            let path = match requested {
                Some(path) => path,
                None => {
                    JupyterRestClient::new(config.clone())
                        .map_err(|e| e.to_string())?
                        .create_notebook(&directory)
                        .await
                        .map_err(|e| e.to_string())?
                        .path
                }
            };
            let controller = NotebookController::open(config.clone(), &path)
                .await
                .map_err(|e| e.to_string())?;
            if restoring {
                // A stopped or unavailable kernel must not prevent restoring the document.
                let _ = controller.attach_existing_session().await;
            }
            Ok::<_, String>((path, controller))
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                let preserve_selection = path.is_some();
                if let Some(path) = path {
                    this.opening.remove(&path);
                } else {
                    this.creating = false;
                }
                match result {
                    Ok(Ok((path, controller))) => {
                        this.unrestored
                            .retain(|(_, tab)| tab.path.as_ref() != Some(&path));
                        this.reveal_file_parent(&path);
                        let session = Session::from_controller(controller, &this.runtime);
                        let notebook = cx.new(|cx| {
                            if restoring {
                                Notebook::new_with_connection(
                                    session,
                                    this.toasts.clone(),
                                    false,
                                    window,
                                    cx,
                                )
                            } else {
                                Notebook::new(session, this.toasts.clone(), window, cx)
                            }
                        });
                        notebook.update(cx, |view, _| view.set_file_busy(this.file_busy));
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
                        let selection = this.file_selection.clone();
                        let directory = this.file_directory.clone();
                        this.activate(this.next_id, window, cx);
                        if preserve_selection {
                            this.file_selection = selection;
                            this.file_directory = directory;
                        }
                        this.refresh_files(cx);
                    }
                    Ok(Err(error)) => this.file_error = Some(error),
                    Err(_) => this.file_error = Some("Could not open notebook".into()),
                }
                this.finish_restore(window, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
    fn activate(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        self.tabs.activate(id);
        self.reveal_tab = Some(id);
        if let Some(doc) = self.documents.iter().find(|doc| doc.id == id) {
            if let Some(path) = &doc.path {
                self.file_selection.single(path);
                self.file_directory = ganymede_core::files::parent(path).to_owned();
            }
            match &doc.content {
                Content::Notebook(notebook) => {
                    notebook.update(cx, |notebook, cx| notebook.focus_selected(window, cx))
                }
                Content::Text(editor) => editor.update(cx, |editor, cx| editor.focus(window, cx)),
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
            doc.content.suspend_autosave(suspended, cx);
        }
    }
    pub fn request_close(
        &mut self,
        target: CloseTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.file_busy || self.close_pending || window.has_active_prompt() {
            if matches!(target, CloseTarget::Quit) {
                crate::connection::cancel_quit(cx);
            }
            return;
        }
        let dirty = self
            .affected(target)
            .any(|doc| doc.content.is_dirty(cx) || doc.content.is_saving(cx));
        if !dirty {
            self.complete_close(target, window, cx);
            return;
        }
        let quitting = matches!(target, CloseTarget::Quit);
        let title = if quitting {
            "Save changes before quitting?"
        } else {
            "Save changes before closing?"
        };
        let detail = "Your unsaved changes will be lost if you discard them.";
        let choices = vec![
            "Cancel",
            "Discard changes",
            if quitting {
                "Save and quit"
            } else {
                "Save and close"
            },
        ];
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
                    crate::connection::cancel_quit(cx);
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
        let documents: Vec<_> = self
            .affected(target)
            .filter(|doc| doc.content.is_dirty(cx) || doc.content.is_saving(cx))
            .map(|doc| doc.content.clone())
            .collect();
        for document in &documents {
            document.save(window, cx);
        }
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(std::time::Duration::from_millis(50)).await;
                let finished = this.update_in(cx, |this, window, cx| {
                    if documents.iter().any(|document| document.is_saving(cx)) { return false; }
                    if documents.iter().any(|document| document.save_failed(cx)) || this.affected(target).any(|doc| doc.content.is_dirty(cx) || doc.content.is_saving(cx)) {
                        crate::connection::cancel_quit(cx);
                        this.close_pending = false;
                        this.suspend_autosave(target, false, cx);
                        if !documents.iter().any(|document| document.save_failed(cx)) {
                            this.toasts.update(cx, |host, cx| { host.push(crate::toast::Toast::new("New changes arrived while saving")
                                .description("Your document is still open. Save again before closing.")
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
            CloseTarget::Window | CloseTarget::Quit => {
                self.persist_layout(cx);
                window.remove_window();
            }
        }
    }
    fn finish_close(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        // Closing a file never shuts down its server kernel.
        self.tabs.close(id);
        self.documents.retain(|doc| doc.id != id);
        self.activate(self.tabs.active, window, cx);
    }
    fn sync_header_tabs(&mut self, now: Instant, reduced: bool) {
        for tab in &mut self.header_tabs {
            tab.motion.set_open(self.tabs.open.contains(&tab.id), now);
            if reduced {
                tab.motion.from = tab.motion.to;
            }
        }
        self.header_tabs
            .retain(|tab| tab.motion.to != 0. || tab.motion.active(now));
        for (index, id) in self.tabs.open.iter().copied().enumerate() {
            if !self.header_tabs.iter().any(|tab| tab.id == id) {
                let position = self.tabs.open[index + 1..]
                    .iter()
                    .find_map(|next| self.header_tabs.iter().position(|tab| tab.id == *next))
                    .unwrap_or(self.header_tabs.len());
                self.header_tabs.insert(
                    position,
                    HeaderTab {
                        id,
                        name: "Launcher".into(),
                        icon: Icon::SquaresFour,
                        selected: false,
                        motion: TabMotion {
                            from: if reduced { 1. } else { 0. },
                            to: 1.,
                            started: now,
                        },
                    },
                );
            }
            let tab = self
                .header_tabs
                .iter_mut()
                .find(|tab| tab.id == id)
                .unwrap();
            if let Some(doc) = self.documents.iter().find(|doc| doc.id == id) {
                tab.name = doc.name.clone();
                tab.icon = doc.kind.icon();
            }
            tab.selected = self.tabs.active == id;
        }
    }
    pub fn header(entity: &Entity<Self>, window: &mut Window, cx: &mut App) -> gpui::Div {
        let now = Instant::now();
        let reduced = cx.reduce_motion();
        entity.update(cx, |this, _| {
            this.sync_header_tabs(now, reduced);
            if let Some(id) = this.reveal_tab {
                if let Some(index) = this.header_tabs.iter().position(|tab| tab.id == id) {
                    this.tab_scroll.scroll_to_item(index);
                    if !this.header_tabs.iter().any(|tab| tab.motion.active(now)) {
                        this.reveal_tab = None;
                    }
                } else {
                    this.reveal_tab = None;
                }
            }
        });
        let this = entity.read(cx);
        if this.header_tabs.iter().any(|tab| tab.motion.active(now)) {
            window.request_animation_frame();
        }
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
                    .overflow_x_scroll()
                    .track_scroll(&this.tab_scroll)
                    .children(this.header_tabs.iter().map(|tab| {
                        let id = tab.id;
                        let leaving = tab.motion.to == 0.;
                        let visibility = tab.motion.value(now);
                        let doc = this.documents.iter().find(|doc| doc.id == id);
                        let selected = tab.selected;
                        let name = tab.name.clone();
                        let icon = tab.icon;
                        let (dirty, running) = match doc.map(|doc| &doc.content) {
                            Some(Content::Notebook(notebook)) => (
                                notebook.read(cx).is_dirty(cx),
                                notebook.read(cx).running_spinner(cx),
                            ),
                            Some(Content::Text(editor)) => (editor.read(cx).is_dirty(), None),
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
                            .id(("tab-slot", id))
                            .w(px(180. * visibility))
                            .h(px(28.))
                            .mr(px(4. * visibility))
                            .flex_shrink_0()
                            .when(visibility < 1., |slot| slot.overflow_hidden())
                            .child(
                                div()
                                    .id(("document-tab", id))
                                    .relative()
                                    .top(px(28. * (1. - visibility)))
                                    .opacity(visibility)
                                    .tab_index(if leaving { -1 } else { 0 })
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
                                        if leaving {
                                            return;
                                        }
                                        select.update(cx, |this, cx| this.activate(id, window, cx));
                                    })
                                    .on_key_down(move |event, window, cx| {
                                        if !leaving
                                            && matches!(
                                                event.keystroke.key.as_str(),
                                                "enter" | "space"
                                            )
                                        {
                                            select_key.update(cx, |this, cx| {
                                                this.activate(id, window, cx)
                                            });
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
                                            .tab_index(if leaving { -1 } else { 0 })
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
                                                if leaving {
                                                    return;
                                                }
                                                close.update(cx, |this, cx| {
                                                    this.close(id, window, cx)
                                                });
                                            })
                                            .on_key_down(move |event, window, cx| {
                                                if !leaving
                                                    && matches!(
                                                        event.keystroke.key.as_str(),
                                                        "enter" | "space"
                                                    )
                                                {
                                                    close_key.update(cx, |this, cx| {
                                                        this.close(id, window, cx)
                                                    });
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
                                    ),
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
            FileAction::Folder => self.prompt_new_folder(window, cx),
            FileAction::Filter => {
                if self.filter_open {
                    self.close_filter(window, cx);
                } else {
                    self.filter_open = true;
                    self.filter_input
                        .read(cx)
                        .focus_handle(cx)
                        .focus(window, cx);
                }
            }
            FileAction::Menu => {
                self.file_menu = None;
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
                        "New folder",
                        Icon::FolderPlus,
                    ),
                    (
                        FileAction::Filter,
                        "files-filter",
                        "Filter files",
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
                    let selected = matches!(action, FileAction::Filter) && this.filter_open;
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
                                                            .child(
                                                                icon.svg()
                                                                    .size(px(14.))
                                                                    .text_color(
                                                                        if this.files_menu_index
                                                                            == Some(index)
                                                                        {
                                                                            t.popover.accent_fg
                                                                        } else {
                                                                            t.popover.fg
                                                                        },
                                                                    ),
                                                            )
                                                            .child(label)
                                                            .on_hover(move |hovered, _, cx| {
                                                                hover.update(cx, |this, cx| {
                                                                    if *hovered {
                                                                        this.files_menu_index =
                                                                            Some(index);
                                                                    } else if this.files_menu_index
                                                                        == Some(index)
                                                                    {
                                                                        this.files_menu_index =
                                                                            None;
                                                                    }
                                                                    cx.notify();
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
        let visible_paths = std::sync::Arc::new(
            rows.iter()
                .map(|(entry, _)| entry.path.clone())
                .collect::<Vec<_>>(),
        );
        let root_menu = entity.clone();
        let root_drop = entity.clone();
        let root_select = entity.clone();
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(Self::files_toolbar(entity, cx))
            .when(this.filter_open, |view| {
                let close = entity.clone();
                let close_key = entity.clone();
                view.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .mt_1()
                        .mb_1()
                        .min_w_0()
                        .child(this.filter_input.clone())
                        .child(
                            icon_button("clear-file-filter", "Clear and close filter", Icon::X, cx)
                                .on_click(move |_, window, cx| {
                                    close.update(cx, |this, cx| this.close_filter(window, cx))
                                })
                                .on_key_down(move |event, window, cx| {
                                    if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                                        close_key
                                            .update(cx, |this, cx| this.close_filter(window, cx));
                                    }
                                }),
                        ),
                )
            })
            .when(
                this.filter_open
                    && !this.filter_query.is_empty()
                    && this.filter_requested.len() >= 1024,
                |view| {
                    view.child(
                        div()
                            .text_size(t.font.xs)
                            .text_color(t.sidebar_content.muted_fg)
                            .child("Search limited to 1,024 folders."),
                    )
                },
            )
            .when_some(this.file_error.as_ref(), |view, error| {
                view.child(div().text_size(t.font.xs).text_color(t.destructive).child(
                    if this.server_unavailable() {
                        "Server unavailable. Showing cached files.".to_owned()
                    } else {
                        error.clone()
                    },
                ))
            })
            .child(
                div()
                    .id("server-file-tree")
                    .on_mouse_down(gpui::MouseButton::Right, move |event, window, cx| {
                        cx.stop_propagation();
                        root_menu.update(cx, |this, cx| {
                            this.show_file_menu(String::new(), true, event.position, window, cx)
                        });
                    })
                    .on_mouse_down(gpui::MouseButton::Left, move |_, _, cx| {
                        root_select.update(cx, |this, cx| {
                            this.select_file_path("", true);
                            cx.notify();
                        });
                    })
                    .on_drop(move |drag: &DragFile, window, cx| {
                        cx.stop_propagation();
                        root_drop.update(cx, |this, cx| {
                            this.move_files(drag.paths.clone(), String::new(), window, cx)
                        });
                    })
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .children(rows.into_iter().map(|(entry, depth)| {
                        let directory = entry.content_type == "directory";
                        let supported =
                            directory || matches!(entry.content_type.as_str(), "notebook" | "file");
                        let selected = this.file_selection.paths.contains(&entry.path);
                        let icon = if directory {
                            if this.expanded.contains(&entry.path)
                                || (this.filter_open && !this.filter_query.is_empty())
                            {
                                Icon::FolderOpen
                            } else {
                                Icon::Folder
                            }
                        } else {
                            Icon::FileText
                        };
                        let entity = entity.clone();
                        let menu_entity = entity.clone();
                        let drop_entity = entity.clone();
                        let drag_select = entity.clone();
                        let menu_path = entry.path.clone();
                        let visible_paths = visible_paths.clone();
                        let drop_directory = if directory {
                            entry.path.clone()
                        } else {
                            ganymede_core::files::parent(&entry.path).to_owned()
                        };
                        let drop_check = drop_directory.clone();
                        let drag = DragFile {
                            paths: if selected {
                                selection_roots(
                                    this.file_selection
                                        .paths
                                        .iter()
                                        .filter(|path| visible_paths.contains(path))
                                        .cloned(),
                                )
                            } else {
                                vec![entry.path.clone()]
                            },
                        };
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
                            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
                                cx.stop_propagation()
                            })
                            .on_mouse_down(gpui::MouseButton::Right, move |event, window, cx| {
                                cx.stop_propagation();
                                menu_entity.update(cx, |this, cx| {
                                    this.show_file_menu(
                                        menu_path.clone(),
                                        directory,
                                        event.position,
                                        window,
                                        cx,
                                    )
                                });
                            })
                            .when(supported && !this.file_busy, |view| {
                                view.on_drag(drag, move |drag, _, _, cx| {
                                    drag_select.update(cx, |this, cx| {
                                        if drag.paths.len() == 1
                                            && !this.file_selection.paths.contains(&drag.paths[0])
                                        {
                                            this.file_selection.single(&drag.paths[0]);
                                            cx.notify();
                                        }
                                    });
                                    cx.new(|_| drag.clone())
                                })
                                .drag_over::<DragFile>(move |view, drag, _, cx| {
                                    if !drag.paths.iter().any(|path| {
                                        ganymede_core::files::is_within(&drop_check, path)
                                    }) && drag.paths.iter().any(|path| {
                                        ganymede_core::files::parent(path) != drop_check
                                    }) {
                                        view.bg(cx.theme().active_file_bg)
                                    } else {
                                        view
                                    }
                                })
                                .on_drop(
                                    move |drag: &DragFile, window, cx| {
                                        cx.stop_propagation();
                                        drop_entity.update(cx, |this, cx| {
                                            this.move_files(
                                                drag.paths.clone(),
                                                drop_directory.clone(),
                                                window,
                                                cx,
                                            )
                                        });
                                    },
                                )
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
                            .on_click(move |event, window, cx| {
                                entity.update(cx, |this, cx| {
                                    if this.file_busy {
                                        return;
                                    }
                                    let modifiers = event.modifiers();
                                    let toggle = if cfg!(target_os = "macos") {
                                        modifiers.platform
                                    } else {
                                        modifiers.control
                                    };
                                    this.file_selection.click(
                                        &entry.path,
                                        &visible_paths,
                                        toggle,
                                        modifiers.shift,
                                    );
                                    this.file_directory = if directory {
                                        entry.path.clone()
                                    } else {
                                        ganymede_core::files::parent(&entry.path).to_owned()
                                    };
                                    cx.notify();
                                    if toggle || modifiers.shift {
                                        return;
                                    }
                                    if directory {
                                        if !this.expanded.remove(&entry.path) {
                                            this.expanded.insert(entry.path.clone());
                                            this.refresh_directory(entry.path.clone(), cx);
                                        }
                                        cx.notify();
                                    } else if supported {
                                        if entry.content_type == "notebook" {
                                            this.open_notebook(
                                                Some(entry.path.clone()),
                                                window,
                                                cx,
                                            );
                                        } else {
                                            this.open_text(
                                                Some(entry.path.clone()),
                                                DocumentKind::for_text_path(&entry.path),
                                                window,
                                                cx,
                                            );
                                        }
                                    }
                                });
                            })
                    })),
            )
            .children(Self::file_context_menu(entity, cx))
    }
    fn tree_rows(&self, path: &str, depth: usize, rows: &mut Vec<(Contents, usize)>) {
        collect_tree_rows(
            &self.directories,
            &self.expanded,
            path,
            depth,
            if self.filter_open {
                &self.filter_query
            } else {
                ""
            },
            rows,
        );
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
                }) => editor.clone().into_any_element(),
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
fn collect_tree_rows(
    directories: &HashMap<String, Vec<Contents>>,
    expanded: &HashSet<String>,
    path: &str,
    depth: usize,
    query: &str,
    rows: &mut Vec<(Contents, usize)>,
) {
    if depth >= 32 {
        return;
    }
    if let Some(entries) = directories.get(path) {
        for entry in entries {
            if query.is_empty() {
                rows.push((entry.clone(), depth));
                if expanded.contains(&entry.path) {
                    collect_tree_rows(directories, expanded, &entry.path, depth + 1, query, rows);
                }
            } else {
                let mut descendants = Vec::new();
                if entry.content_type == "directory" {
                    collect_tree_rows(
                        directories,
                        expanded,
                        &entry.path,
                        depth + 1,
                        query,
                        &mut descendants,
                    );
                }
                if entry.name.to_lowercase().contains(query) || !descendants.is_empty() {
                    rows.push((entry.clone(), depth));
                    rows.extend(descendants);
                }
            }
        }
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
impl Workspace {
    pub(crate) fn add_dirty_test_notebook(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use ganymede_core::notebook::{CellKind, NotebookDocument};
        let mut document = NotebookDocument::new();
        document
            .insert_cell(0, CellKind::Code, "unsaved = True")
            .unwrap();
        let config = ganymede_core::jupyter::ServerConfig::new("http://127.0.0.1:9/").unwrap();
        let controller = NotebookController::new(config, "test.ipynb", document).unwrap();
        let session = Session::from_controller(controller, &self.runtime);
        let notebook = cx.new(|cx| Notebook::new(session, self.toasts.clone(), window, cx));
        self.next_id += 1;
        self.documents.push(Document {
            id: self.next_id,
            name: "test.ipynb".into(),
            path: Some("test.ipynb".into()),
            kind: DocumentKind::Notebook,
            content: Content::Notebook(notebook),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn tree_entry(path: &str, kind: &str) -> Contents {
        serde_json::from_value(serde_json::json!({
            "name": path.rsplit('/').next().unwrap(), "path": path, "type": kind,
            "writable": true, "created": "", "last_modified": "", "content": null
        }))
        .unwrap()
    }
    #[test]
    fn filename_filter_shows_collapsed_ancestors_and_preserves_normal_expansion() {
        let directories = HashMap::from([
            (
                "".into(),
                vec![
                    tree_entry("projects", "directory"),
                    tree_entry("other.txt", "file"),
                ],
            ),
            (
                "projects".into(),
                vec![
                    tree_entry("projects/nested", "directory"),
                    tree_entry("projects/skip.py", "file"),
                ],
            ),
            (
                "projects/nested".into(),
                vec![
                    tree_entry("projects/nested/Analysis.ipynb", "notebook"),
                    tree_entry("projects/nested/analysis.txt", "file"),
                ],
            ),
        ]);
        let expanded = HashSet::new();
        let mut rows = Vec::new();
        collect_tree_rows(&directories, &expanded, "", 0, "analysis", &mut rows);
        assert_eq!(
            rows.iter()
                .map(|(entry, depth)| (entry.path.as_str(), *depth))
                .collect::<Vec<_>>(),
            vec![
                ("projects", 0),
                ("projects/nested", 1),
                ("projects/nested/Analysis.ipynb", 2),
                ("projects/nested/analysis.txt", 2)
            ]
        );
        rows.clear();
        collect_tree_rows(&directories, &expanded, "", 0, "missing", &mut rows);
        assert!(rows.is_empty());
        collect_tree_rows(&directories, &expanded, "", 0, "", &mut rows);
        assert_eq!(
            rows.iter()
                .map(|(entry, _)| entry.path.as_str())
                .collect::<Vec<_>>(),
            vec!["projects", "other.txt"]
        );
        assert!(expanded.is_empty());
    }
    #[test]
    fn tab_motion_reverses_without_jumping_and_finishes_closed() {
        let start = Instant::now();
        let mut motion = TabMotion {
            from: 0.,
            to: 1.,
            started: start,
        };
        assert_eq!(motion.value(start), 0.);
        let midway = start + TAB_MOTION_DURATION / 2;
        let before = motion.value(midway);
        assert!(before > 0. && before < 1.);
        motion.set_open(false, midway);
        assert_eq!(motion.value(midway), before);
        assert!(motion.active(midway));
        assert_eq!(motion.value(midway + TAB_MOTION_DURATION), 0.);
        assert!(!motion.active(midway + TAB_MOTION_DURATION));
    }
    #[test]
    fn reopening_an_exiting_tab_keeps_its_current_position() {
        let start = Instant::now();
        let mut motion = TabMotion {
            from: 1.,
            to: 0.,
            started: start,
        };
        let midway = start + TAB_MOTION_DURATION / 2;
        let before = motion.value(midway);
        motion.set_open(true, midway);
        assert_eq!(motion.value(midway), before);
        assert_eq!(motion.value(midway + TAB_MOTION_DURATION), 1.);
        motion.from = motion.to; // Reduced-motion behavior settles immediately.
        assert!(!motion.active(midway));
        assert_eq!(motion.value(midway), 1.);
    }
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
