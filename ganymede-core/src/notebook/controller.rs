use super::{
    CellId, CellKind, ConnectionState, Error, ExecutionPhase, NotebookDocument, NotebookEvent,
    NotebookSnapshot, Result, SaveState, state::State,
};
use crate::jupyter::{
    JupyterRestClient, KernelClient, KernelEvent, ServerConfig,
    messaging::{ExecuteRequest, KernelMessage},
    models::{CreateSession, Session},
};
use serde_json::{Map, Value};
use std::{
    future::Future,
    sync::{Arc, Mutex},
};
use tokio::sync::{broadcast, watch};

struct Live {
    kernel: KernelClient,
    task: tokio::task::AbortHandle,
}
struct Data {
    state: State,
    live: Option<Live>,
    epoch: u64,
    published_revision: u64,
    saved_version: Option<String>,
}
struct Inner {
    data: Mutex<Data>,
    config: ServerConfig,
    frontend_session: String,
    rest: JupyterRestClient,
    operations: tokio::sync::Mutex<()>,
    saves: tokio::sync::Mutex<()>,
    snapshots: watch::Sender<Arc<NotebookSnapshot>>,
    events: broadcast::Sender<NotebookEvent>,
}
impl Drop for Inner {
    fn drop(&mut self) {
        if let Ok(data) = self.data.get_mut()
            && let Some(live) = data.live.take()
        {
            live.task.abort();
        }
    }
}
impl Inner {
    fn publish(&self, data: &mut Data, mut events: Vec<NotebookEvent>) {
        let revision = data.state.snapshot.document.revision();
        if revision != data.published_revision {
            data.published_revision = revision;
            events.push(NotebookEvent::DocumentChanged { revision });
        }
        self.snapshots
            .send_replace(Arc::new(data.state.snapshot.clone()));
        for event in events {
            let _ = self.events.send(event);
        }
    }
}

/// A cloneable application handle. Document edits are synchronous; network
/// operations are asynchronous and continue once started even if their caller
/// drops the future. Dropping every handle disconnects, but never kills a kernel.
#[derive(Clone)]
pub struct NotebookController {
    inner: Arc<Inner>,
}
impl NotebookController {
    /// Construct without connecting. `document` may contain unsaved local work.
    pub fn new(
        config: ServerConfig,
        path: impl Into<String>,
        document: NotebookDocument,
    ) -> Result<Self> {
        let path = path.into();
        if path.is_empty() {
            return Err(Error::Invalid("a notebook path is required".into()));
        }
        let revision = document.revision();
        let state = State::new(path, document);
        let (snapshots, _) = watch::channel(Arc::new(state.snapshot.clone()));
        let (events, _) = broadcast::channel(256);
        Ok(Self {
            inner: Arc::new(Inner {
                rest: JupyterRestClient::new(config.clone())?,
                config,
                frontend_session: uuid::Uuid::new_v4().to_string(),
                data: Mutex::new(Data {
                    state,
                    live: None,
                    epoch: 0,
                    published_revision: revision,
                    saved_version: None,
                }),
                operations: tokio::sync::Mutex::new(()),
                saves: tokio::sync::Mutex::new(()),
                snapshots,
                events,
            }),
        })
    }
    pub async fn open(config: ServerConfig, path: impl Into<String>) -> Result<Self> {
        let path = path.into();
        let contents = JupyterRestClient::new(config.clone())?
            .get_contents(&path, true)
            .await?;
        if contents.content_type != "notebook" {
            return Err(Error::Invalid("server resource is not a notebook".into()));
        }
        let controller = Self::new(config, path, NotebookDocument::from_json(contents.content)?)?;
        controller.inner.data.lock().unwrap().saved_version = Some(contents.last_modified);
        Ok(controller)
    }
    pub fn snapshot(&self) -> Arc<NotebookSnapshot> {
        self.inner.snapshots.borrow().clone()
    }
    /// Latest-state updates coalesce; always render the current snapshot.
    pub fn watch(&self) -> watch::Receiver<Arc<NotebookSnapshot>> {
        self.inner.snapshots.subscribe()
    }
    /// Ordered notifications. A slow subscriber receives `RecvError::Lagged`
    /// and should resynchronize from `snapshot()`, including pending stdin.
    pub fn subscribe(&self) -> broadcast::Receiver<NotebookEvent> {
        self.inner.events.subscribe()
    }

    fn edit<T>(
        &self,
        edit: impl FnOnce(&mut State) -> Result<T>,
        event: Option<CellId>,
    ) -> Result<T> {
        let mut data = self.inner.data.lock().unwrap();
        let result = edit(&mut data.state)?;
        self.inner.publish(
            &mut data,
            event
                .map(|cell_id| vec![NotebookEvent::CellChanged { cell_id }])
                .unwrap_or_default(),
        );
        Ok(result)
    }
    pub fn insert_cell(
        &self,
        index: usize,
        kind: CellKind,
        source: impl Into<String>,
    ) -> Result<CellId> {
        self.edit(
            |state| state.snapshot.document.insert_cell(index, kind, source),
            None,
        )
    }
    pub fn update_source(&self, id: &CellId, source: impl Into<String>) -> Result<()> {
        self.edit(
            |state| state.snapshot.document.update_source(id, source),
            Some(id.clone()),
        )
    }
    pub fn remove_cell(&self, id: &CellId) -> Result<()> {
        self.edit(
            |state| {
                if state
                    .snapshot
                    .executions
                    .get(id)
                    .is_some_and(|run| run.phase.is_active())
                {
                    return Err(Error::Unavailable(
                        "interrupt or finish execution before removing this cell".into(),
                    ));
                }
                state.snapshot.document.remove_cell(id)?;
                state.forget_cell(id);
                Ok(())
            },
            Some(id.clone()),
        )
    }
    pub fn move_cell(&self, id: &CellId, index: usize) -> Result<()> {
        self.edit(
            |state| state.snapshot.document.move_cell(id, index),
            Some(id.clone()),
        )
    }
    pub fn set_cell_kind(&self, id: &CellId, kind: CellKind) -> Result<()> {
        self.edit(
            |state| {
                if state
                    .snapshot
                    .executions
                    .get(id)
                    .is_some_and(|run| run.phase.is_active())
                {
                    return Err(Error::Unavailable(
                        "finish execution before changing cell type".into(),
                    ));
                }
                if state.snapshot.document.cell(id)?.kind() != kind {
                    state.snapshot.document.set_cell_kind(id, kind)?;
                    state.forget_cell(id);
                }
                Ok(())
            },
            Some(id.clone()),
        )
    }
    pub fn set_metadata(&self, metadata: Map<String, Value>) -> Result<()> {
        self.edit(
            |state| {
                state.snapshot.document.set_metadata(metadata);
                Ok(())
            },
            None,
        )
    }
    pub fn set_cell_metadata(&self, id: &CellId, metadata: Map<String, Value>) -> Result<()> {
        self.edit(
            |state| state.snapshot.document.set_cell_metadata(id, metadata),
            Some(id.clone()),
        )
    }
    pub fn clear_outputs(&self, id: &CellId) -> Result<()> {
        self.edit(|state| state.clear_outputs(id), Some(id.clone()))
    }

    /// Saves a captured revision. Later edits and kernel output remain dirty.
    /// Concurrent saves are serialized; cancelling the caller cannot leave a
    /// completed server write unacknowledged in the local save state.
    /// Checks the last acknowledged server modification time before writing.
    /// This is not atomic: the standard Contents API has no conditional PUT.
    /// Controllers constructed with `new` only save to missing paths until an
    /// explicit overwrite; use `open` to establish a baseline for existing files.
    pub async fn save(&self) -> Result<()> {
        self.save_inner(false).await
    }

    /// Explicit user-authorized overwrite after a conflict.
    pub async fn overwrite(&self) -> Result<()> {
        self.save_inner(true).await
    }

    async fn save_inner(&self, overwrite: bool) -> Result<()> {
        let this = self.clone();
        finish(async move {
            let _save = this.inner.saves.lock().await;
            let (path, revision, json, version) = {
                let mut data = this.inner.data.lock().unwrap();
                let snapshot = &data.state.snapshot;
                let values = (
                    snapshot.path.clone(),
                    snapshot.document.revision(),
                    snapshot.document.to_json(),
                    data.saved_version.clone(),
                );
                let saving = SaveState::Saving { revision: values.1 };
                data.state.snapshot.save = saving.clone();
                this.inner
                    .publish(&mut data, vec![NotebookEvent::SaveChanged(saving)]);
                values
            };
            let result: Result<_> = async {
                if !overwrite {
                    match this.inner.rest.get_contents(&path, false).await {
                        Ok(contents)
                            if version.as_ref() == Some(&contents.last_modified)
                                && contents.content_type == "notebook" => {}
                        Err(crate::jupyter::Error::Api { status, .. })
                            if status.as_u16() == 404 && version.is_none() => {}
                        Ok(_) => return Err(Error::SaveConflict),
                        Err(crate::jupyter::Error::Api { status, .. })
                            if status.as_u16() == 404 =>
                        {
                            return Err(Error::SaveConflict);
                        }
                        Err(error) => return Err(error.into()),
                    }
                }
                Ok(this.inner.rest.save_notebook(&path, &json).await?)
            }
            .await;
            let mut data = this.inner.data.lock().unwrap();
            let save = match &result {
                Ok(contents) => {
                    data.saved_version = Some(contents.last_modified.clone());
                    data.state.snapshot.document.mark_saved(revision);
                    SaveState::Idle
                }
                Err(error) => SaveState::Failed {
                    message: error.to_string(),
                },
            };
            data.state.snapshot.save = save.clone();
            this.inner
                .publish(&mut data, vec![NotebookEvent::SaveChanged(save)]);
            result.map(|_| ())
        })
        .await
    }

    /// Save a separate server-allocated notebook without changing the original's dirty state.
    pub async fn save_copy(&self) -> Result<String> {
        let this = self.clone();
        finish(async move {
            let _save = this.inner.saves.lock().await;
            let snapshot = this.snapshot();
            let parent = snapshot
                .path
                .rsplit_once('/')
                .map_or("", |(parent, _)| parent);
            let copy = this.inner.rest.create_notebook(parent).await?;
            this.inner
                .rest
                .save_notebook(&copy.path, &snapshot.document.to_json())
                .await?;
            Ok(copy.path)
        })
        .await
    }

    /// Reload only if neither editing nor execution changed the document while fetching.
    pub async fn reload(&self) -> Result<()> {
        let this = self.clone();
        finish(async move {
            let _save = this.inner.saves.lock().await;
            let before = this.snapshot();
            let contents = this.inner.rest.get_contents(&before.path, true).await?;
            if contents.content_type != "notebook" {
                return Err(Error::Invalid("server resource is not a notebook".into()));
            }
            let document = NotebookDocument::from_json(contents.content)?;
            let mut data = this.inner.data.lock().unwrap();
            if data.state.snapshot.document.revision() != before.document.revision()
                || data
                    .state
                    .snapshot
                    .executions
                    .values()
                    .any(|run| run.phase.is_active())
            {
                return Err(Error::Unavailable(
                    "notebook changed or is running; try reloading again after execution finishes"
                        .into(),
                ));
            }
            let ids: Vec<_> = data
                .state
                .snapshot
                .document
                .cells()
                .iter()
                .map(|cell| cell.id().clone())
                .collect();
            for id in ids {
                data.state.forget_cell(&id);
            }
            data.state.snapshot.document = document;
            data.state.snapshot.save = SaveState::Idle;
            data.saved_version = Some(contents.last_modified);
            this.inner
                .publish(&mut data, vec![NotebookEvent::SaveChanged(SaveState::Idle)]);
            Ok(())
        })
        .await
    }

    /// Create/reuse a notebook session, or reconnect to this controller's known
    /// session. Does not save the document or replay cells.
    pub async fn connect(&self, kernel_name: impl Into<String>) -> Result<()> {
        let this = self.clone();
        let kernel_name = kernel_name.into();
        finish(async move {
            let _op = this.inner.operations.lock().await;
            if this.inner.data.lock().unwrap().live.is_some() {
                return Ok(());
            }
            let snapshot = this.snapshot();
            let session = match &snapshot.session {
                Some(session) => this.inner.rest.get_session(&session.id).await?,
                None => {
                    this.inner
                        .rest
                        .create_session(&CreateSession::notebook(&snapshot.path, kernel_name))
                        .await?
                }
            };
            this.attach_inner(session).await
        })
        .await
    }
    /// Attach to an existing session for this notebook path, without taking
    /// ownership of its kernel. Shutdown always requires an explicit call.
    pub async fn attach_session(&self, session_id: impl Into<String>) -> Result<()> {
        let this = self.clone();
        let id = session_id.into();
        finish(async move {
            let _op = this.inner.operations.lock().await;
            let session = this.inner.rest.get_session(&id).await?;
            if session.path != this.snapshot().path {
                return Err(Error::Invalid(
                    "session belongs to a different notebook path".into(),
                ));
            }
            this.disconnect_inner().await?;
            this.attach_inner(session).await
        })
        .await
    }
    async fn attach_inner(&self, session: Session) -> Result<()> {
        {
            let mut data = self.inner.data.lock().unwrap();
            data.state.snapshot.session = Some(session.clone());
            self.inner.publish(&mut data, vec![]);
        }
        let ready = async {
            let (kernel, mut incoming) = KernelClient::connect_with_session(
                self.inner.config.clone(),
                &session.kernel.id,
                self.inner.frontend_session.clone(),
            )
            .await?;
            let id = kernel.kernel_info().await?;
            loop {
                match incoming.recv().await {
                    Some(KernelEvent::Message(message))
                        if message.channel == "shell"
                            && message.header.msg_type == "kernel_info_reply"
                            && message.parent_id() == Some(id.as_str()) =>
                    {
                        return Ok::<_, crate::jupyter::Error>((
                            kernel,
                            incoming,
                            message.header.session.clone(),
                        ));
                    }
                    Some(KernelEvent::Message(_)) => (), // Drain messages buffered before attachment.
                    Some(KernelEvent::Failed(error)) => return Err(error),
                    _ => return Err(crate::jupyter::Error::Disconnected),
                }
            }
        };
        let (kernel, mut incoming, kernel_session) =
            match tokio::time::timeout(self.inner.config.request_timeout, ready)
                .await
                .unwrap_or(Err(crate::jupyter::Error::Timeout))
            {
                Ok(connection) => connection,
                Err(error) => {
                    let mut data = self.inner.data.lock().unwrap();
                    let events = data.state.disconnected(Some(error.to_string()));
                    self.inner.publish(&mut data, events);
                    return Err(error.into());
                }
            };
        let mut data = self.inner.data.lock().unwrap();
        data.state.establish_kernel_session(kernel_session);
        data.epoch += 1;
        let epoch = data.epoch;
        let weak = Arc::downgrade(&self.inner);
        let task = tokio::spawn(async move {
            while let Some(event) = incoming.recv().await {
                let Some(inner) = weak.upgrade() else {
                    break;
                };
                let mut data = inner.data.lock().unwrap();
                if data.epoch != epoch {
                    break;
                }
                let terminal = !matches!(&event, KernelEvent::Message(_));
                let events = match event {
                    KernelEvent::Message(message) => {
                        data.state.apply(*message).unwrap_or_else(|error| {
                            vec![NotebookEvent::ProtocolError(error.to_string())]
                        })
                    }
                    KernelEvent::Closed => data.state.disconnected(None),
                    KernelEvent::Failed(error) => data.state.disconnected(Some(error.to_string())),
                };
                if terminal {
                    data.live = None;
                }
                inner.publish(&mut data, events);
                if terminal {
                    break;
                }
            }
        });
        data.live = Some(Live {
            kernel,
            task: task.abort_handle(),
        });
        data.state.snapshot.connection = ConnectionState::Connected;
        self.inner.publish(
            &mut data,
            vec![NotebookEvent::ConnectionChanged(ConnectionState::Connected)],
        );
        Ok(())
    }
    pub async fn disconnect(&self) -> Result<()> {
        let this = self.clone();
        finish(async move {
            let _op = this.inner.operations.lock().await;
            this.disconnect_inner().await
        })
        .await
    }
    async fn disconnect_inner(&self) -> Result<()> {
        let live = {
            let mut data = self.inner.data.lock().unwrap();
            data.epoch += 1;
            let live = data.live.take();
            if let Some(live) = &live {
                live.task.abort();
            }
            let events = data.state.disconnected(None);
            self.inner.publish(&mut data, events);
            live
        };
        // The event receiver dropping can close the low-level connection first.
        if let Some(live) = live {
            let _ = live.kernel.close().await;
        }
        Ok(())
    }
    pub async fn shutdown_session(&self) -> Result<()> {
        let this = self.clone();
        finish(async move {
            let _op = this.inner.operations.lock().await;
            let session = this
                .snapshot()
                .session
                .clone()
                .ok_or_else(|| Error::Unavailable("no session".into()))?;
            this.inner.rest.delete_session(&session.id).await?;
            this.disconnect_inner().await?;
            let mut data = this.inner.data.lock().unwrap();
            data.state.snapshot.session = None;
            this.inner.publish(&mut data, vec![]);
            Ok(())
        })
        .await
    }
    /// Forget the association without deleting the server's session or kernel.
    /// The document is retained, including all unsaved edits and outputs.
    pub async fn detach_session(&self) -> Result<()> {
        let this = self.clone();
        finish(async move {
            let _op = this.inner.operations.lock().await;
            this.disconnect_inner().await?;
            let mut data = this.inner.data.lock().unwrap();
            data.state.snapshot.session = None;
            this.inner.publish(&mut data, vec![]);
            Ok(())
        })
        .await
    }
    pub async fn restart_kernel(&self) -> Result<()> {
        let this = self.clone();
        finish(async move {
            let _op = this.inner.operations.lock().await;
            let mut session = this
                .snapshot()
                .session
                .clone()
                .ok_or_else(|| Error::Unavailable("no session".into()))?;
            this.disconnect_inner().await?;
            session.kernel = this.inner.rest.restart_kernel(&session.kernel.id).await?;
            this.attach_inner(session).await
        })
        .await
    }
    pub async fn interrupt(&self) -> Result<()> {
        let this = self.clone();
        finish(async move {
            let _op = this.inner.operations.lock().await;
            let session = this
                .snapshot()
                .session
                .clone()
                .ok_or_else(|| Error::Unavailable("no session".into()))?;
            Ok(this.inner.rest.interrupt_kernel(&session.kernel.id).await?)
        })
        .await
    }
    pub async fn run_cell(&self, cell: &CellId, allow_stdin: bool) -> Result<String> {
        self.run_cell_inner(cell, allow_stdin, None).await
    }
    /// Execute a queued source snapshot without overwriting newer document edits.
    pub async fn run_cell_source(
        &self,
        cell: &CellId,
        source: String,
        allow_stdin: bool,
    ) -> Result<String> {
        self.run_cell_inner(cell, allow_stdin, Some(source)).await
    }
    async fn run_cell_inner(
        &self,
        cell: &CellId,
        allow_stdin: bool,
        source: Option<String>,
    ) -> Result<String> {
        let this = self.clone();
        let cell = cell.clone();
        finish(async move {
            let _op = this.inner.operations.lock().await;
            let (kernel, message) = {
                let mut data = this.inner.data.lock().unwrap();
                let kernel = data
                    .live
                    .as_ref()
                    .ok_or_else(|| Error::Unavailable("kernel is disconnected".into()))?
                    .kernel
                    .clone();
                let mut execute = ExecuteRequest::new(
                    source.unwrap_or(data.state.snapshot.document.cell(&cell)?.source()),
                );
                execute.allow_stdin = allow_stdin;
                let message = KernelMessage::request(
                    kernel.session_id(),
                    "shell",
                    "execute_request",
                    serde_json::to_value(execute)?,
                );
                // Register before sending: a fast kernel may reply before send resolves.
                let events = data.state.begin_run(&cell, message.header.msg_id.clone())?;
                this.inner.publish(&mut data, events);
                (kernel, message)
            };
            match kernel.send(message).await {
                Ok(id) => Ok(id),
                Err(error) => {
                    // Sending may have reached the kernel. Never report "not run" or retry.
                    let mut data = this.inner.data.lock().unwrap();
                    if let Some(execution) = data.state.snapshot.executions.get_mut(&cell) {
                        execution.phase = ExecutionPhase::Unknown;
                    }
                    this.inner.publish(
                        &mut data,
                        vec![NotebookEvent::ExecutionChanged {
                            cell_id: cell,
                            phase: ExecutionPhase::Unknown,
                        }],
                    );
                    Err(error.into())
                }
            }
        })
        .await
    }
    pub async fn reply_input(
        &self,
        input_id: impl Into<String>,
        value: impl Into<String>,
    ) -> Result<()> {
        let this = self.clone();
        let id = input_id.into();
        let value = value.into();
        finish(async move {
            let _op = this.inner.operations.lock().await;
            let (kernel, request) = {
                let data = this.inner.data.lock().unwrap();
                (
                    data.live
                        .as_ref()
                        .ok_or_else(|| Error::Unavailable("kernel is disconnected".into()))?
                        .kernel
                        .clone(),
                    data.state.input_messages.get(&id).cloned().ok_or_else(|| {
                        Error::Unavailable("input request is no longer pending".into())
                    })?,
                )
            };
            kernel.input_reply(&request, &value).await?;
            let mut data = this.inner.data.lock().unwrap();
            data.state.input_messages.remove(&id);
            let mut events = vec![];
            if let Some(input) = data.state.snapshot.pending_inputs.remove(&id)
                && let Some(execution) = data.state.snapshot.executions.get_mut(&input.cell_id)
                && execution.phase == ExecutionPhase::WaitingForInput
            {
                execution.phase = ExecutionPhase::Running;
                events.push(NotebookEvent::ExecutionChanged {
                    cell_id: input.cell_id,
                    phase: ExecutionPhase::Running,
                });
            }
            this.inner.publish(&mut data, events);
            Ok(())
        })
        .await
    }
}
async fn finish<T: Send + 'static>(
    future: impl Future<Output = Result<T>> + Send + 'static,
) -> Result<T> {
    tokio::spawn(future)
        .await
        .map_err(|error| Error::Unavailable(format!("notebook operation task failed: {error}")))?
}
