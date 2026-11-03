//! Bridge between GPUI editors and server-backed notebook controllers.
use ganymede_core::{
    jupyter::ServerConfig,
    notebook::{
        CellId, CellKind, ConnectionState, ExecutionPhase, KnownOutput, NotebookController,
        NotebookDocument, Output,
    },
};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};
use tokio::{runtime::Handle, sync::watch};

#[derive(Clone)]
pub struct Runtime {
    pub handle: Handle,
    pub config: Result<ServerConfig, String>,
    pub kernel: String,
}
impl gpui::Global for Runtime {}
impl Runtime {
    pub fn new(handle: Handle) -> Self {
        // Read without modifying process environment (which is unsafe after threads start).
        let file = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../.env"));
        let values = file.as_deref().map(parse_env).unwrap_or_default();
        let get = |name: &str| {
            std::env::var(name)
                .ok()
                .or_else(|| values.get(name).cloned())
        };
        let config = (|| {
            if let Err(error) = &file
                && error.kind() != std::io::ErrorKind::NotFound
            {
                return Err("Cannot read local .env configuration".into());
            }
            let url = get("GANYMEDE_JUPYTER_URL").ok_or("Set GANYMEDE_JUPYTER_URL in .env")?;
            let mut config = ServerConfig::new(&url).map_err(|e| e.to_string())?;
            if let Some(token) = get("GANYMEDE_JUPYTER_TOKEN").filter(|s| !s.is_empty()) {
                config = config.with_token(&token).map_err(|e| e.to_string())?;
            }
            Ok(config)
        })();
        Self {
            handle,
            config,
            kernel: get("GANYMEDE_JUPYTER_KERNEL").unwrap_or_else(|| "python3".into()),
        }
    }
}
fn parse_env(text: &str) -> HashMap<String, String> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.starts_with('#') {
                return None;
            }
            let (key, value) = line.split_once('=')?;
            let value = value.trim();
            let value = value
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .or_else(|| value.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
                .unwrap_or(value);
            Some((key.trim().to_owned(), value.to_owned()))
        })
        .collect()
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum KernelActivity {
    Disconnected,
    Idle,
    Busy,
}

#[derive(Clone)]
pub struct Session {
    controller: Result<NotebookController, String>,
    cells: Arc<Mutex<HashMap<u64, CellId>>>,
    handle: Handle,
    kernel: String,
    generation: Arc<AtomicU64>,
    submission: Arc<tokio::sync::Mutex<()>>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct OutputBlock {
    pub text: String,
    pub execution_count: Option<u64>,
    pub data: Option<serde_json::Map<String, serde_json::Value>>,
}

#[derive(Clone, Default)]
pub struct Update {
    pub count: Option<u64>,
    pub blocks: Vec<OutputBlock>,
    pub done: bool,
    pub failed: bool,
    pub input: Option<ganymede_core::notebook::PendingInput>,
}
#[cfg(test)]
impl Update {
    fn text(&self) -> String {
        self.blocks
            .iter()
            .map(|block| block.text.as_str())
            .collect()
    }
}

impl Session {
    pub fn from_controller(controller: NotebookController, runtime: &Runtime) -> Self {
        Self {
            controller: Ok(controller),
            cells: Default::default(),
            handle: runtime.handle.clone(),
            kernel: runtime.kernel.clone(),
            generation: Arc::new(AtomicU64::new(0)),
            submission: Default::default(),
        }
    }
    pub fn controller(&self) -> NotebookController {
        self.controller.as_ref().unwrap().clone()
    }
    pub fn document(&self) -> NotebookDocument {
        self.controller
            .as_ref()
            .unwrap()
            .snapshot()
            .document
            .clone()
    }
    pub fn bind(&self, key: u64, id: CellId) {
        self.cells.lock().unwrap().insert(key, id);
    }
    pub fn sync(&self, editors: &[(u64, CellKind, String)]) -> Result<(), String> {
        let controller = self.controller.as_ref().map_err(Clone::clone)?;
        let mut cells = self.cells.lock().unwrap();
        let removed: Vec<_> = cells
            .iter()
            .filter(|(key, _)| !editors.iter().any(|e| e.0 == **key))
            .map(|(key, id)| (*key, id.clone()))
            .collect();
        for (key, id) in removed {
            controller.remove_cell(&id).map_err(|e| e.to_string())?;
            cells.remove(&key);
        }
        for (index, (key, kind, source)) in editors.iter().enumerate() {
            let id = if let Some(id) = cells.get(key) {
                id.clone()
            } else {
                let id = controller
                    .insert_cell(index, *kind, source.clone())
                    .map_err(|e| e.to_string())?;
                cells.insert(*key, id.clone());
                id
            };
            let current = controller.snapshot();
            let cell = current.document.cell(&id).map_err(|e| e.to_string())?;
            if cell.kind() != *kind {
                controller
                    .set_cell_kind(&id, *kind)
                    .map_err(|e| e.to_string())?;
            }
            if cell.source() != *source {
                controller
                    .update_source(&id, source.clone())
                    .map_err(|e| e.to_string())?;
            }
            if current.document.cells()[index].id() != &id {
                controller
                    .move_cell(&id, index)
                    .map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }
    pub fn restore_structure(
        &self,
        bindings: &[(u64, CellId)],
        restored: &[ganymede_core::notebook::Cell],
    ) -> Result<(), String> {
        let controller = self.controller.as_ref().map_err(Clone::clone)?;
        let mut cells = self.cells.lock().unwrap();
        controller
            .restore_structure(
                &bindings
                    .iter()
                    .map(|(_, id)| id.clone())
                    .collect::<Vec<_>>(),
                restored,
            )
            .map_err(|error| error.to_string())?;
        *cells = bindings.iter().cloned().collect();
        Ok(())
    }

    pub fn clear_outputs(&self, key: u64) -> Result<(), String> {
        let controller = self.controller.as_ref().map_err(Clone::clone)?;
        if let Some(id) = self.cells.lock().unwrap().get(&key) {
            controller.clear_outputs(id).map_err(|e| e.to_string())?;
        }
        Ok(())
    }
    pub fn save(&self) -> tokio::task::JoinHandle<ganymede_core::notebook::Result<()>> {
        let controller = self.controller.as_ref().unwrap().clone();
        self.handle.spawn(async move { controller.save().await })
    }
    pub fn overwrite(&self) -> tokio::task::JoinHandle<ganymede_core::notebook::Result<()>> {
        let controller = self.controller.as_ref().unwrap().clone();
        self.handle
            .spawn(async move { controller.overwrite().await })
    }
    pub fn reload(&self) -> tokio::task::JoinHandle<ganymede_core::notebook::Result<()>> {
        let controller = self.controller.as_ref().unwrap().clone();
        self.handle.spawn(async move { controller.reload().await })
    }
    pub fn save_copy(&self) -> tokio::task::JoinHandle<ganymede_core::notebook::Result<String>> {
        let controller = self.controller.as_ref().unwrap().clone();
        self.handle
            .spawn(async move { controller.save_copy().await })
    }
    pub fn path(&self) -> String {
        self.controller.as_ref().unwrap().snapshot().path.clone()
    }
    pub fn clear_bindings(&self) {
        self.cells.lock().unwrap().clear();
    }
    /// Establish the notebook session without submitting any code.
    pub fn connect(&self) -> tokio::task::JoinHandle<Result<(), String>> {
        let this = self.clone();
        self.handle.spawn(async move {
            let _submission = this.submission.lock().await;
            let controller = this.controller.as_ref().map_err(Clone::clone)?;
            controller
                .connect(this.kernel)
                .await
                .map_err(|e| e.to_string())
        })
    }

    pub fn activity(&self) -> KernelActivity {
        let Ok(controller) = &self.controller else {
            return KernelActivity::Disconnected;
        };
        let snapshot = controller.snapshot();
        if snapshot.connection != ConnectionState::Connected {
            KernelActivity::Disconnected
        } else if snapshot.kernel_status.as_deref() == Some("busy") {
            KernelActivity::Busy
        } else {
            KernelActivity::Idle
        }
    }

    pub fn watch(&self) -> Option<watch::Receiver<Arc<ganymede_core::notebook::NotebookSnapshot>>> {
        self.controller.as_ref().ok().map(NotebookController::watch)
    }

    pub fn status(&self) -> String {
        match &self.controller {
            Ok(controller) => {
                let snapshot = controller.snapshot();
                match snapshot.connection {
                    ConnectionState::Connected => format!(
                        "{} · {}",
                        self.kernel,
                        snapshot.kernel_status.as_deref().unwrap_or("idle")
                    ),
                    _ if snapshot.session.is_some() => "Disconnected".into(),
                    _ => "Python · not connected".into(),
                }
            }
            Err(_) => "Configuration required".into(),
        }
    }
    pub fn run(&self, key: u64, source: String) -> watch::Receiver<Update> {
        let (tx, rx) = watch::channel(Update::default());
        let this = self.clone();
        let generation = self.generation.load(Ordering::SeqCst);
        self.handle.spawn(async move {
            let result = async {
                let submission = this.submission.lock().await;
                let controller = this.controller.as_ref().map_err(Clone::clone)?;
                // Check before reconnecting as well: cancelled submissions must not create a session.
                if this.generation.load(Ordering::SeqCst) != generation {
                    return Err("Execution cancelled before submission".into());
                }
                // Explicit Run may reconnect. No failed request is automatically replayed.
                controller.connect(this.kernel.clone()).await.map_err(|e| e.to_string())?;
                if this.generation.load(Ordering::SeqCst) != generation {
                    return Err("Execution cancelled before submission".into());
                }
                let id = {
                    let mut cells = this.cells.lock().unwrap();
                    if let Some(id) = cells.get(&key) {
                        id.clone()
                    } else {
                        let id = controller.insert_cell(controller.snapshot().document.cells().len(), CellKind::Code, source.clone()).map_err(|e| e.to_string())?;
                        cells.insert(key, id.clone());
                        id
                    }
                };
                let mut snapshots = controller.watch();
                controller.run_cell_source(&id, source, true).await.map_err(|e| e.to_string())?;
                drop(submission);
                loop {
                    let snapshot = snapshots.borrow_and_update().clone();
                    let cell = snapshot.document.cell(&id).map_err(|e| e.to_string())?;
                    let phase = snapshot.executions.get(&id).map(|run| &run.phase);
                    let done = phase.is_some_and(|phase| !phase.is_active());
                    let failed = done && phase != Some(&ExecutionPhase::Succeeded);
                    let mut blocks = output_blocks(cell.outputs());
                    if phase == Some(&ExecutionPhase::Unknown) {
                        let message = "Connection lost or kernel restarted. Execution outcome is unknown; code was not retried.";
                        blocks.push(OutputBlock { text: message.into(), execution_count: None, data: None });
                    }
                    let input = snapshot.pending_inputs.values().find(|input| input.cell_id == id).cloned();
                    tx.send_replace(Update { count: cell.execution_count(), blocks, done, failed, input });
                    if done || tx.is_closed() { break; }
                    snapshots.changed().await.map_err(|_| "Kernel connection ended".to_owned())?;
                }
                Ok::<(), String>(())
            }.await;
            if let Err(error) = result {
                tx.send_replace(Update { blocks: vec![OutputBlock { text: error.clone(), execution_count: None, data: None }], done: true, failed: true, ..Default::default() });
            }
        });
        rx
    }
    pub fn input_generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }
    pub fn reply_input(
        &self,
        id: String,
        value: String,
        generation: u64,
    ) -> tokio::task::JoinHandle<Result<(), String>> {
        let this = self.clone();
        self.handle.spawn(async move {
            let _submission = this.submission.lock().await;
            if this.input_generation() != generation {
                return Err("Input request was cancelled".into());
            }
            this.controller
                .as_ref()
                .map_err(Clone::clone)?
                .reply_input(id, value)
                .await
                .map_err(|_| "Could not send input; the request may no longer be active".into())
        })
    }
    pub fn control(&self, restart: bool) -> tokio::task::JoinHandle<Result<(), String>> {
        self.generation.fetch_add(1, Ordering::SeqCst);
        let controller = self.controller.clone();
        let submission = self.submission.clone();
        self.handle.spawn(async move {
            let _submission = submission.lock().await;
            let controller = controller?;
            if restart {
                controller.restart_kernel().await
            } else {
                controller.interrupt().await
            }
            .map_err(|e| e.to_string())
        })
    }
}

pub fn output_blocks(outputs: &[Output]) -> Vec<OutputBlock> {
    let mut blocks: Vec<OutputBlock> = Vec::new();
    let mut previous_stream: Option<&str> = None;
    for output in outputs {
        let stream = match output {
            Output::Known(KnownOutput::Stream { name, .. }) => Some(name.as_str()),
            _ => None,
        };
        let text = output_text(std::slice::from_ref(output));
        let data = match output {
            Output::Known(
                KnownOutput::ExecuteResult { data, .. } | KnownOutput::DisplayData { data, .. },
            ) => Some(data.clone()),
            _ => None,
        };
        if text.is_empty() && data.is_none() {
            continue;
        }
        // Stream messages can split a line arbitrarily; do not add row breaks.
        if stream.is_some() && stream == previous_stream {
            blocks.last_mut().unwrap().text.push_str(&text);
        } else if !text.is_empty() {
            let execution_count = match output {
                Output::Known(KnownOutput::ExecuteResult {
                    execution_count, ..
                }) => Some(*execution_count),
                _ => None,
            };
            blocks.push(OutputBlock {
                text,
                execution_count,
                data,
            });
        }
        previous_stream = stream.filter(|_| !blocks.is_empty());
    }
    blocks
}

fn output_text(outputs: &[Output]) -> String {
    let mut text = String::new();
    for output in outputs {
        match output {
            Output::Known(KnownOutput::Stream { text: value, .. }) => text.push_str(&value.text()),
            Output::Known(
                KnownOutput::ExecuteResult { data, .. } | KnownOutput::DisplayData { data, .. },
            ) => {
                if let Some(value) = data.get("text/plain") {
                    if !text.is_empty() && !text.ends_with('\n') {
                        text.push('\n');
                    }
                    match value {
                        serde_json::Value::String(s) => text.push_str(s),
                        serde_json::Value::Array(lines) => {
                            for line in lines {
                                if let Some(s) = line.as_str() {
                                    text.push_str(s);
                                }
                            }
                        }
                        _ => (),
                    }
                    text.push('\n');
                } else {
                    text.push_str("[Unsupported rich output]\n");
                }
            }
            Output::Known(KnownOutput::Error {
                ename,
                evalue,
                traceback,
                ..
            }) => {
                if !text.is_empty() && !text.ends_with('\n') {
                    text.push('\n');
                }
                if traceback.is_empty() {
                    text.push_str(&format!("{ename}: {evalue}\n"));
                } else {
                    text.push_str(&traceback.join("\n"));
                    text.push('\n');
                }
            }
            Output::Unknown(_) => text.push_str("[Unsupported output]\n"),
        }
    }
    text
}
#[cfg(test)]
fn strip_ansi(text: &str) -> String {
    let mut chars = text.chars().peekable();
    let mut result = String::new();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for ch in chars.by_ref() {
                if ('@'..='~').contains(&ch) {
                    break;
                }
            }
        } else {
            result.push(ch);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    async fn completed(mut updates: watch::Receiver<Update>) -> Update {
        tokio::time::timeout(std::time::Duration::from_secs(45), async move {
            loop {
                let update = updates.borrow_and_update().clone();
                if update.done {
                    return update;
                }
                updates.changed().await.expect("execution update stream");
            }
        })
        .await
        .expect("execution finished")
    }

    #[tokio::test]
    #[ignore = "requires the Jupyter server configured in .env"]
    async fn live_execution_streams_preserves_state_errors_interrupts_and_restarts() {
        let runtime = Runtime::new(Handle::current());
        let controller = NotebookController::new(
            runtime.config.unwrap(),
            format!("ganymede-test-{}.ipynb", uuid::Uuid::new_v4()),
            NotebookDocument::new(),
        )
        .unwrap();
        let session = Session {
            controller: Ok(controller.clone()),
            cells: Default::default(),
            handle: runtime.handle,
            kernel: runtime.kernel,
            generation: Arc::new(AtomicU64::new(0)),
            submission: Default::default(),
        };
        // Clean up only the unique session this test creates, even when assertions fail.
        session.connect().await.unwrap().unwrap();
        assert_eq!(controller.snapshot().connection, ConnectionState::Connected);
        assert!(controller.snapshot().executions.is_empty());
        assert!(controller.snapshot().document.cells().is_empty());
        let test_session = session.clone();
        let result = tokio::spawn(async move {
            let first =
                completed(test_session.run(1, "x = 40\nprint('hello')\nx + 2".into())).await;
            assert!(!first.failed, "{}", first.text());
            assert_eq!(first.text(), "hello\n42\n");
            assert_eq!(first.count, Some(1));
            let second = completed(test_session.run(2, "x + 3".into())).await;
            assert_eq!(second.text(), "43\n");
            assert_eq!(second.count, Some(2));
            let error =
                completed(test_session.run(2, "raise ValueError('test error')".into())).await;
            assert!(error.failed);
            assert!(strip_ansi(&error.text()).contains("ValueError: test error"));

            let mut streaming = test_session.run(
                3,
                "import time\nprint('started', flush=True)\ntime.sleep(30)".into(),
            );
            tokio::time::timeout(std::time::Duration::from_secs(10), async {
                loop {
                    let update = streaming.borrow_and_update().clone();
                    if update.text().contains("started") {
                        assert!(!update.done);
                        break;
                    }
                    streaming.changed().await.unwrap();
                }
            })
            .await
            .unwrap();
            test_session.control(false).await.unwrap().unwrap();
            let interrupted = completed(streaming).await;
            assert!(interrupted.failed);
            assert!(interrupted.text().contains("KeyboardInterrupt"));

            // Reconnect to the same kernel without executing or resetting its variables.
            let before = test_session.controller().snapshot();
            test_session.controller().disconnect().await.unwrap();
            assert!(matches!(
                test_session.activity(),
                KernelActivity::Disconnected
            ));
            test_session.connect().await.unwrap().unwrap();
            assert_eq!(
                test_session
                    .controller()
                    .snapshot()
                    .session
                    .as_ref()
                    .unwrap()
                    .id,
                before.session.as_ref().unwrap().id
            );
            assert_eq!(
                test_session.controller().snapshot().document.to_json(),
                before.document.to_json()
            );
            let retained = completed(test_session.run(4, "x".into())).await;
            assert_eq!(retained.text(), "40\n");

            let mut active = test_session.run(
                5,
                "import time\nprint('restarting', flush=True)\ntime.sleep(30)".into(),
            );
            tokio::time::timeout(std::time::Duration::from_secs(10), async {
                loop {
                    if active.borrow_and_update().text().contains("restarting") {
                        break;
                    }
                    active.changed().await.unwrap();
                }
            })
            .await
            .unwrap();
            test_session.control(true).await.unwrap().unwrap();
            let cancelled = completed(active).await;
            assert!(cancelled.failed);
            assert!(cancelled.text().contains("outcome is unknown"));
            let restarted = completed(test_session.run(4, "'x' in globals()".into())).await;
            assert!(!restarted.failed, "{}", restarted.text());
            assert_eq!(restarted.text(), "False\n");
            assert_eq!(restarted.count, Some(1));
            let rich = completed(test_session.run(6, "from IPython.display import display, HTML\nh = display(HTML('<b>before</b>'), display_id=True)\nh.update(HTML('<b>after</b>'))\nprint('\\033[31mred\\033[0m')\n42".into())).await;
            assert!(!rich.failed, "{}", rich.text());
            assert!(rich.blocks.iter().any(|block| block.data.as_ref().and_then(|data| data.get("text/html")) == Some(&serde_json::json!("<b>after</b>"))));
            assert!(rich.blocks.iter().any(|block| block.text.contains("\x1b[31mred")));
            assert!(rich.blocks.iter().any(|block| block.execution_count == Some(2) && block.text == "42\n"));
        })
        .await;
        if controller.snapshot().session.is_some() {
            controller
                .shutdown_session()
                .await
                .expect("delete test session");
        }
        result.unwrap();
    }

    #[tokio::test]
    #[ignore = "requires the Jupyter server configured in .env"]
    async fn live_stdin_repeated_password_interrupt_restart_and_disconnect() {
        async fn prompt(
            updates: &mut watch::Receiver<Update>,
        ) -> ganymede_core::notebook::PendingInput {
            tokio::time::timeout(std::time::Duration::from_secs(20), async {
                loop {
                    let update = updates.borrow_and_update().clone();
                    assert!(!update.done, "execution ended before prompt");
                    if let Some(input) = update.input {
                        return input;
                    }
                    updates.changed().await.unwrap();
                }
            })
            .await
            .unwrap()
        }
        let runtime = Runtime::new(Handle::current());
        let controller = NotebookController::new(
            runtime.config.clone().unwrap(),
            format!("ganymede-stdin-test-{}.ipynb", uuid::Uuid::new_v4()),
            NotebookDocument::new(),
        )
        .unwrap();
        let session = Session::from_controller(controller.clone(), &runtime);
        let result = tokio::spawn(async move {
            let mut run = session.run(1, "name = input('Name: ')\nimport getpass\nsecret = getpass.getpass('Password: ')\nempty = input('Empty: ')\nprint(name, len(secret), len(empty))".into());
            let first = prompt(&mut run).await;
            assert!(!first.password);
            let epoch = session.input_generation();
            session.reply_input(first.id.clone(), "Renée".into(), epoch).await.unwrap().unwrap();
            assert!(session.reply_input(first.id, "duplicate".into(), epoch).await.unwrap().is_err());
            // The watch channel may still contain the previous request until its next update.
            run.changed().await.unwrap();
            let password = prompt(&mut run).await;
            assert!(password.password);
            session.reply_input(password.id, "private-😀".into(), epoch).await.unwrap().unwrap();
            run.changed().await.unwrap();
            let empty = prompt(&mut run).await;
            assert!(!empty.password);
            session.reply_input(empty.id, String::new(), epoch).await.unwrap().unwrap();
            let done = completed(run).await;
            assert!(!done.failed);
            assert!(done.text().contains("Renée 9 0"));
            assert!(done.input.is_none());
            assert!(!session.document().to_json().to_string().contains("private-"));
            for restart in [false, true] {
                let mut run = session.run(2, "input('Wait: ')".into());
                let pending = prompt(&mut run).await;
                let epoch = session.input_generation();
                session.control(restart).await.unwrap().unwrap();
                assert!(session.controller().snapshot().pending_inputs.is_empty());
                assert!(session.reply_input(pending.id, "stale".into(), epoch).await.unwrap().is_err());
                assert!(completed(run).await.input.is_none());
            }
            let mut run = session.run(3, "input('Disconnect: ')".into());
            prompt(&mut run).await;
            session.controller().disconnect().await.unwrap();
            let done = completed(run).await;
            assert!(done.failed);
            assert!(done.input.is_none());
        }).await;
        if controller.snapshot().session.is_some() {
            controller.shutdown_session().await.unwrap();
        }
        result.unwrap();
    }

    #[tokio::test]
    async fn interrupt_cancels_waiting_submissions_before_connecting() {
        let runtime = Runtime::new(Handle::current());
        let controller = NotebookController::new(
            ServerConfig::new("http://127.0.0.1:1/").unwrap(),
            "cancelled.ipynb",
            NotebookDocument::new(),
        )
        .unwrap();
        let session = Session::from_controller(controller.clone(), &runtime);
        let submission = session.submission.lock().await;
        let run = session.run(1, "must_not_execute()".into());
        let interrupt = session.control(false);
        drop(submission);
        let update = completed(run).await;
        assert!(update.failed);
        assert_eq!(update.text(), "Execution cancelled before submission");
        // No session exists to interrupt; cancellation still prevents a connection attempt.
        assert!(interrupt.await.unwrap().is_err());
        assert!(controller.snapshot().session.is_none());
        assert!(controller.snapshot().document.cells().is_empty());
    }

    fn fixture() -> NotebookDocument {
        NotebookDocument::from_json(serde_json::json!({
            "nbformat": 4, "nbformat_minor": 5, "metadata": {"custom": [1, 2]}, "extension": "keep",
            "cells": [
                {"id": "code", "cell_type": "code", "source": "old", "metadata": {"tag": true},
                 "outputs": [{"output_type":"display_data", "data":{"text/html":"<b>rich</b>"},"metadata":{}}],
                 "execution_count": 3, "custom_cell": 42},
                {"id": "markdown", "cell_type": "markdown", "source": "![image](attachment:test.png)",
                 "metadata": {}, "attachments": {"test.png": {"image/png": "AA=="}}},
                {"id": "remove", "cell_type": "raw", "source": "remove me", "metadata": {}}
            ]
        })).unwrap()
    }
    #[tokio::test]
    async fn editor_sync_preserves_ids_metadata_outputs_and_orders_all_cell_types() {
        let runtime = Runtime::new(Handle::current());
        let document = fixture();
        let ids: Vec<_> = document.cells().iter().map(|c| c.id().clone()).collect();
        let controller = NotebookController::new(
            ServerConfig::new("http://127.0.0.1:1/").unwrap(),
            "test.ipynb",
            document,
        )
        .unwrap();
        let session = Session::from_controller(controller, &runtime);
        for (key, id) in ids.iter().enumerate() {
            session.bind(key as u64, id.clone());
        }
        session
            .sync(&[
                (1, CellKind::Markdown, "# Changed".into()),
                (0, CellKind::Code, "42".into()),
                (4, CellKind::Raw, "new raw".into()),
            ])
            .unwrap();
        let saved = session.document().to_json();
        assert_eq!(saved["metadata"]["custom"], serde_json::json!([1, 2]));
        assert_eq!(saved["extension"], "keep");
        assert_eq!(saved["cells"][0]["id"], "markdown");
        assert_eq!(
            saved["cells"][0]["attachments"]["test.png"]["image/png"],
            "AA=="
        );
        assert_eq!(saved["cells"][1]["custom_cell"], 42);
        assert_eq!(
            saved["cells"][1]["outputs"][0]["data"]["text/html"],
            "<b>rich</b>"
        );
        assert_eq!(saved["cells"][2]["cell_type"], "raw");
        let revision = session.document().revision();
        session
            .sync(&[
                (1, CellKind::Markdown, "# Changed".into()),
                (0, CellKind::Code, "42".into()),
                (4, CellKind::Raw, "new raw".into()),
            ])
            .unwrap();
        assert_eq!(session.document().revision(), revision);
        session
            .sync(&[
                (1, CellKind::Markdown, "# Changed".into()),
                (0, CellKind::Raw, "42".into()),
                (4, CellKind::Raw, "new raw".into()),
            ])
            .unwrap();
        let changed = session.document().to_json();
        assert_eq!(changed["cells"][1]["id"], "code");
        assert_eq!(changed["cells"][1]["metadata"]["tag"], true);
        assert!(changed["cells"][1].get("outputs").is_none());
    }

    #[tokio::test]
    #[ignore = "requires the Jupyter server configured in .env"]
    async fn live_files_open_edit_run_save_reopen() {
        use ganymede_core::jupyter::JupyterRestClient;
        let runtime = Runtime::new(Handle::current());
        let config = runtime.config.clone().unwrap();
        let client = JupyterRestClient::new(config.clone()).unwrap();
        let created = client.create_notebook("").await.unwrap();
        let opened = NotebookController::open(config.clone(), &created.path).await;
        client.delete_contents(&created.path).await.unwrap();
        assert!(opened.is_ok(), "server-created notebooks can be opened");
        let path = format!("ganymede-roundtrip-{}.ipynb", uuid::Uuid::new_v4());
        client
            .save_notebook(&path, &fixture().to_json())
            .await
            .unwrap();
        let controller = NotebookController::open(config.clone(), &path)
            .await
            .unwrap();
        let session = Session::from_controller(controller.clone(), &runtime);
        let test_path = path.clone();
        let test_client = client.clone();
        let result = tokio::spawn(async move {
            let listing = test_client.get_contents("", true).await.unwrap();
            assert!(
                listing
                    .content
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|entry| entry["path"] == test_path)
            );
            let doc = session.document();
            for (key, cell) in doc.cells().iter().enumerate() {
                session.bind(key as u64, cell.id().clone());
            }
            session
                .sync(&[
                    (1, CellKind::Markdown, "# Edited".into()),
                    (0, CellKind::Code, "print('foo')\n42".into()),
                    (2, CellKind::Raw, "raw text".into()),
                ])
                .unwrap();
            let executed = completed(session.run(0, "print('foo')\n42".into())).await;
            assert!(!executed.failed, "{}", executed.text());
            assert_eq!(executed.text(), "foo\n42\n");
            // A queued source must not replace newer edits in the saved document.
            session
                .sync(&[
                    (1, CellKind::Markdown, "# Edited".into()),
                    (0, CellKind::Code, "print('new edit')".into()),
                    (2, CellKind::Raw, "raw text".into()),
                ])
                .unwrap();
            let executed = completed(session.run(0, "6 * 7".into())).await;
            assert!(!executed.failed);
            assert_eq!(session.document().cells()[1].source(), "print('new edit')");
            session.save().await.unwrap().unwrap();
            assert!(!session.document().is_dirty());
            let reopened = NotebookController::open(config, &test_path).await.unwrap();
            assert_eq!(
                reopened.snapshot().document.to_json(),
                session.document().to_json()
            );
            assert!(!reopened.snapshot().document.is_dirty());
        })
        .await;
        if controller.snapshot().session.is_some() {
            controller.shutdown_session().await.unwrap();
        }
        client.delete_contents(&path).await.unwrap();
        result.unwrap();
    }

    #[tokio::test]
    #[ignore = "requires the Jupyter server configured in .env"]
    async fn live_folder_and_upload_preserve_binary_and_notebook_content() {
        use ganymede_core::jupyter::{Error, JupyterRestClient};
        let runtime = Runtime::new(Handle::current());
        let client = JupyterRestClient::new(runtime.config.unwrap()).unwrap();
        let folder = client.create_directory("").await.unwrap();
        let binary_path = format!("{}/upload.bin", folder.path);
        let notebook_path = format!("{}/upload.ipynb", folder.path);
        let test_client = client.clone();
        let binary = binary_path.clone();
        let notebook = notebook_path.clone();
        let result = tokio::spawn(async move {
            test_client.upload_file(&binary, &[0, 1, 255]).await.unwrap();
            let contents = test_client.get_contents(&binary, true).await.unwrap();
            assert_eq!(contents.format.as_deref(), Some("base64"));
            assert_eq!(contents.content.as_str().unwrap().trim(), "AAH/");
            assert!(matches!(test_client.upload_file(&binary, &[42]).await, Err(Error::Api { status, .. }) if status.as_u16() == 409));
            let document = fixture().to_json();
            test_client.upload_file(&notebook, &serde_json::to_vec(&document).unwrap()).await.unwrap();
            let contents = test_client.get_contents(&notebook, true).await.unwrap();
            assert_eq!(contents.content_type, "notebook");
            // Jupyter adds trust metadata when loading uploaded rich outputs.
            let mut cells = contents.content["cells"].clone();
            for cell in cells.as_array_mut().unwrap() {
                cell["metadata"].as_object_mut().unwrap().remove("trusted");
            }
            assert_eq!(cells, document["cells"]);
            assert_eq!(contents.content["metadata"], document["metadata"]);
        }).await;
        for path in [&binary_path, &notebook_path] {
            match client.delete_contents(path).await {
                Ok(()) => (),
                Err(Error::Api { status, .. }) if status.as_u16() == 404 => (),
                Err(error) => panic!("test cleanup: {error}"),
            }
        }
        client.delete_contents(&folder.path).await.unwrap();
        result.unwrap();
    }

    #[test]
    fn prints_and_returns_keep_distinct_prompts_and_stream_chunks_join() {
        let outputs: Vec<Output> = serde_json::from_value(serde_json::json!([
            {"output_type":"stream","name":"stdout","text":"fo"},
            {"output_type":"stream","name":"stdout","text":"o\n"},
            {"output_type":"execute_result","data":{"text/plain":"42"},"metadata":{},"execution_count":3},
            {"output_type":"display_data","data":{"text/plain":"displayed"},"metadata":{}}
        ])).unwrap();
        let blocks = output_blocks(&outputs);
        assert_eq!(
            blocks,
            vec![
                OutputBlock {
                    text: "foo\n".into(),
                    execution_count: None,
                    data: None
                },
                OutputBlock {
                    text: "42\n".into(),
                    execution_count: Some(3),
                    data: Some(
                        serde_json::from_value(serde_json::json!({"text/plain":"42"})).unwrap()
                    )
                },
                OutputBlock {
                    text: "displayed\n".into(),
                    execution_count: None,
                    data: Some(
                        serde_json::from_value(serde_json::json!({"text/plain":"displayed"}))
                            .unwrap()
                    )
                },
            ]
        );
    }

    #[test]
    fn dotenv_values_preserve_equals_and_accept_quotes() {
        let values = parse_env("# comment\nURL = http://localhost/\nTOKEN='abc=def'\n");
        assert_eq!(values["TOKEN"], "abc=def");
        assert_eq!(values["URL"], "http://localhost/");
    }
    #[test]
    fn output_preserves_streams_results_and_readable_errors() {
        let outputs: Vec<Output> = serde_json::from_value(serde_json::json!([
            {"output_type":"stream","name":"stdout","text":["hello", " world\n"]},
            {"output_type":"execute_result","data":{"text/plain":"42"},"metadata":{},"execution_count":1},
            {"output_type":"error","ename":"ValueError","evalue":"bad","traceback":["\u{1b}[31mValueError: bad\u{1b}[0m"]}
        ])).unwrap();
        assert_eq!(
            strip_ansi(&output_text(&outputs)),
            "hello world\n42\nValueError: bad\n"
        );
    }
}
