//! Local recovery files are independent from layout and scoped to document instances.
use super::*;
use ganymede_core::{
    notebook::NotebookRecovery,
    text_file::{TextFileController, TextRecovery},
};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock, mpsc},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data")]
pub(super) enum Backup {
    Notebook(NotebookRecovery),
    Text(TextRecovery),
}
impl Backup {
    fn path(&self) -> &str {
        match self {
            Self::Notebook(b) => &b.path,
            Self::Text(b) => &b.path,
        }
    }
    fn notebook(&self) -> bool {
        matches!(self, Self::Notebook(_))
    }
}
#[derive(Clone, Serialize, Deserialize)]
struct Record {
    saved_at: u128,
    backup: Backup,
}
#[derive(Clone)]
struct Entry {
    file: PathBuf,
    record: Record,
}
#[derive(Clone)]
enum Controller {
    Notebook(NotebookController),
    Text(TextFileController),
}
impl Controller {
    fn status(&self) -> (String, u64, bool) {
        match self {
            Self::Notebook(c) => {
                let s = c.snapshot();
                (s.path.clone(), s.document.revision(), s.document.is_dirty())
            }
            Self::Text(c) => {
                let (rev, dirty) = c.status();
                (c.path(), rev, dirty)
            }
        }
    }
    fn backup(&self) -> Option<Backup> {
        match self {
            Self::Notebook(c) => c.recovery().map(Backup::Notebook),
            Self::Text(c) => c.recovery().map(Backup::Text),
        }
    }
}
fn active() -> &'static Mutex<HashSet<PathBuf>> {
    static ACTIVE: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    ACTIVE.get_or_init(Default::default)
}
fn write_record(path: &Path, record: &Record) -> std::io::Result<()> {
    use std::io::Write;
    std::fs::create_dir_all(path.parent().unwrap())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            path.parent().unwrap(),
            std::fs::Permissions::from_mode(0o700),
        )?;
    }
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| {
        let mut file = options.open(&temporary)?;
        serde_json::to_writer(&mut file, record)?;
        file.flush()?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}
fn remove(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}
enum Command {
    Save(PathBuf, Controller),
    Remove(PathBuf),
    Flush(mpsc::Sender<()>),
}
struct Tracked {
    file: PathBuf,
    status: Option<(String, u64, bool)>,
}
pub(super) struct Recovery {
    directory: Option<PathBuf>,
    entries: Vec<Entry>,
    claimed: HashSet<PathBuf>,
    tracked: HashMap<u64, Tracked>,
    chosen: HashMap<String, Option<Entry>>,
    pending: std::collections::VecDeque<(String, DocumentKind)>,
    prompt: bool,
    ignored: HashSet<u64>,
    sender: mpsc::Sender<Command>,
    errors: mpsc::Receiver<String>,
    reported_error: bool,
}
impl Recovery {
    pub fn new(runtime: &Runtime) -> Self {
        let directory = runtime
            .config
            .as_ref()
            .ok()
            .map(|c| persistence::state_path(c.base_url().as_str()).with_file_name("recovery"));
        let mut entries = Vec::new();
        let mut claimed = HashSet::new();
        if let Some(directory) = &directory {
            let mut active = active().lock().unwrap();
            if let Ok(files) = std::fs::read_dir(directory) {
                for file in files
                    .flatten()
                    .map(|e| e.path())
                    .filter(|p| p.extension().is_some_and(|e| e == "json"))
                {
                    if active.contains(&file) {
                        continue;
                    }
                    if let Some(record) = std::fs::read(&file)
                        .ok()
                        .and_then(|b| serde_json::from_slice::<Record>(&b).ok())
                    {
                        active.insert(file.clone());
                        claimed.insert(file.clone());
                        entries.push(Entry { file, record });
                    }
                }
            }
        }
        entries.sort_by_key(|e| std::cmp::Reverse(e.record.saved_at));
        let (sender, receiver) = mpsc::channel();
        let (error_tx, errors) = mpsc::channel();
        std::thread::spawn(move || {
            for command in receiver {
                let result = match command {
                    Command::Save(file, controller) => match controller.backup() {
                        Some(backup) => write_record(
                            &file,
                            &Record {
                                saved_at: SystemTime::now()
                                    .duration_since(UNIX_EPOCH)
                                    .unwrap_or_default()
                                    .as_millis(),
                                backup,
                            },
                        ),
                        None => remove(&file),
                    },
                    Command::Remove(file) => remove(&file),
                    Command::Flush(done) => {
                        let _ = done.send(());
                        Ok(())
                    }
                };
                if let Err(error) = result {
                    let _ = error_tx.send(error.to_string());
                }
            }
        });
        Self {
            directory,
            entries,
            claimed,
            tracked: HashMap::new(),
            chosen: HashMap::new(),
            pending: Default::default(),
            prompt: false,
            ignored: HashSet::new(),
            sender,
            errors,
            reported_error: false,
        }
    }
    pub fn paths(&self) -> Vec<(String, bool)> {
        self.entries
            .iter()
            .map(|e| {
                (
                    e.record.backup.path().to_owned(),
                    e.record.backup.notebook(),
                )
            })
            .collect()
    }
    pub fn waiting(&self) -> bool {
        self.prompt || !self.pending.is_empty()
    }
    pub fn selected(&self, path: &str) -> Option<Backup> {
        self.chosen
            .get(path)
            .and_then(|e| e.as_ref())
            .map(|e| e.record.backup.clone())
    }
    fn track(&mut self, id: u64, path: &str) {
        let Some(directory) = &self.directory else {
            return;
        };
        let file = self
            .chosen
            .remove(path)
            .flatten()
            .map(|e| e.file)
            .unwrap_or_else(|| directory.join(format!("{}.json", uuid::Uuid::new_v4())));
        active().lock().unwrap().insert(file.clone());
        self.claimed.insert(file.clone());
        self.tracked.insert(id, Tracked { file, status: None });
    }
    fn discard(&mut self, id: u64) {
        self.ignored.insert(id);
        if let Some(tracked) = self.tracked.get(&id) {
            let _ = self.sender.send(Command::Remove(tracked.file.clone()));
        }
    }
    fn flush(&self) {
        let (send, receive) = mpsc::channel();
        if self.sender.send(Command::Flush(send)).is_ok() {
            let _ = receive.recv();
        }
    }
}
impl Drop for Recovery {
    fn drop(&mut self) {
        self.flush();
        let mut active = active().lock().unwrap();
        for file in &self.claimed {
            active.remove(file);
        }
    }
}

impl Workspace {
    pub(super) fn recovery_before_open(
        &mut self,
        path: &str,
        kind: DocumentKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.recovery.chosen.contains_key(path)
            || !self
                .recovery
                .entries
                .iter()
                .any(|e| e.record.backup.path() == path)
        {
            return false;
        }
        let kind = match self
            .recovery
            .entries
            .iter()
            .find(|e| e.record.backup.path() == path)
        {
            Some(entry) if entry.record.backup.notebook() => DocumentKind::Notebook,
            Some(_) => DocumentKind::for_text_path(path),
            None => kind,
        };
        if !self.recovery.pending.iter().any(|(p, _)| p == path) {
            self.recovery.pending.push_back((path.into(), kind));
        }
        self.next_recovery(window, cx);
        true
    }
    pub(super) fn next_recovery(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.recovery.prompt || window.has_active_prompt() {
            return;
        }
        let Some((path, kind)) = self.recovery.pending.pop_front() else {
            return;
        };
        let Some(entry) = self
            .recovery
            .entries
            .iter()
            .find(|e| e.record.backup.path() == path)
            .cloned()
        else {
            return;
        };
        self.recovery.prompt = true;
        let response = window.prompt(gpui::PromptLevel::Warning, "Recover unsaved changes?",
            Some(&format!("A local recovery copy is available for {path}. Restore opens those edits without changing the server file. Autosave stays paused until you save. Discard removes only this recovery copy.")),
            &["Later", "Discard", "Restore"], cx);
        cx.spawn_in(window, async move |this, cx| {
            let answer = response.await.unwrap_or(0);
            this.update_in(cx, |this, window, cx| {
                this.recovery.prompt = false;
                let selected = if answer == 2 {
                    Some(entry.clone())
                } else {
                    None
                };
                if answer != 0 {
                    this.recovery.entries.retain(|e| e.file != entry.file);
                }
                if answer == 1 {
                    let _ = this
                        .recovery
                        .sender
                        .send(Command::Remove(entry.file.clone()));
                }
                this.recovery.chosen.insert(path.clone(), selected);
                if kind == DocumentKind::Notebook {
                    this.open_notebook(Some(path), window, cx);
                } else {
                    this.open_text(Some(path), kind, window, cx);
                }
                this.next_recovery(window, cx);
                this.finish_restore(window, cx);
            })
            .ok();
        })
        .detach();
    }
    pub(super) fn track_recovery(&mut self, id: u64, path: &str) {
        self.recovery.track(id, path);
    }
    pub(super) fn discard_recovery(&mut self, target: CloseTarget) {
        let ids: Vec<_> = self.affected(target).map(|d| d.id).collect();
        for id in ids {
            self.recovery.discard(id);
        }
        self.recovery.flush();
    }
    pub(super) fn persist_recovery(&mut self, cx: &mut App) {
        for doc in &self.documents {
            if self.recovery.ignored.contains(&doc.id) {
                continue;
            }
            let controller = match &doc.content {
                Content::Notebook(view) => {
                    Controller::Notebook(view.update(cx, |view, cx| view.file_controller(cx)))
                }
                Content::Text(view) => Controller::Text(view.read(cx).file_controller()),
                _ => continue,
            };
            let Some(tracked) = self.recovery.tracked.get_mut(&doc.id) else {
                continue;
            };
            let status = controller.status();
            if tracked.status.as_ref() != Some(&status) {
                let command = if status.2 {
                    Command::Save(tracked.file.clone(), controller)
                } else {
                    Command::Remove(tracked.file.clone())
                };
                let _ = self.recovery.sender.send(command);
                tracked.status = Some(status);
            }
        }
        if self.recovery.errors.try_recv().is_ok() {
            while self.recovery.errors.try_recv().is_ok() {}
            // Retry rather than treating a failed disk write as a successful backup.
            for tracked in self.recovery.tracked.values_mut() {
                tracked.status = None;
            }
            if self.recovery.reported_error {
                return;
            }
            self.recovery.reported_error = true;
            self.toasts.update(cx, |host, cx| {
                host.push(
                    crate::toast::Toast::new("Could not update local recovery copies")
                        .description(
                            "Your edits remain open. Save them to the server to keep them.",
                        )
                        .kind(crate::toast::ToastKind::Error),
                    cx,
                )
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn independent_copies_survive_updates_and_discard_is_ordered() {
        let directory =
            std::env::temp_dir().join(format!("ganymede-recovery-{}", uuid::Uuid::new_v4()));
        let runtime = Runtime {
            handle: tokio::runtime::Handle::current(),
            config: Err("test".into()),
            kernel: "python3".into(),
        };
        let mut recovery = Recovery::new(&runtime);
        recovery.directory = Some(directory.clone());
        recovery.track(1, "notes.txt");
        recovery.track(2, "notes.txt");
        let a = recovery.tracked[&1].file.clone();
        let b = recovery.tracked[&2].file.clone();
        assert_ne!(a, b);
        let controller = TextFileController::from_recovery(
            ganymede_core::jupyter::ServerConfig::new("http://localhost:9").unwrap(),
            TextRecovery {
                path: "notes.txt".into(),
                text: "first window".into(),
                saved_version: "v1".into(),
                crlf: false,
                bom: false,
            },
        )
        .unwrap();
        recovery
            .sender
            .send(Command::Save(
                a.clone(),
                Controller::Text(controller.clone()),
            ))
            .unwrap();
        recovery
            .sender
            .send(Command::Save(
                b.clone(),
                Controller::Text(controller.clone()),
            ))
            .unwrap();
        recovery.flush();
        controller.update_text("newest edit".into());
        recovery
            .sender
            .send(Command::Save(a.clone(), Controller::Text(controller)))
            .unwrap();
        recovery.discard(1);
        recovery.flush();
        assert!(!a.exists());
        let record: Record = serde_json::from_slice(&std::fs::read(&b).unwrap()).unwrap();
        let Backup::Text(copy) = record.backup else {
            panic!()
        };
        assert_eq!(copy.text, "first window");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&b).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        drop(recovery);
        assert!(!active().lock().unwrap().contains(&b));
        std::fs::remove_dir_all(directory).unwrap();
    }
}
