use super::{CellId, CellKind, Error, KnownOutput, Multiline, NotebookDocument, Output, Result};
use crate::jupyter::{
    messaging::{ExecutionTracker, KernelMessage, MessageBody},
    models::Session,
};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionState {
    Disconnected { reason: Option<String> },
    Connected,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutionPhase {
    Queued,
    Running,
    WaitingForInput,
    Succeeded,
    Failed,
    Aborted,
    Unknown,
}
impl ExecutionPhase {
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Queued | Self::Running | Self::WaitingForInput)
    }
}
#[derive(Debug, Clone)]
pub struct CellExecution {
    pub request_id: String,
    /// Source captured when submitted, even if the user subsequently edits the cell.
    pub source: String,
    pub phase: ExecutionPhase,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveState {
    Idle,
    Saving { revision: u64 },
    Failed { message: String },
}
#[derive(Debug, Clone)]
pub struct PendingInput {
    pub id: String,
    pub cell_id: CellId,
    pub prompt: String,
    pub password: bool,
}
#[derive(Debug, Clone)]
pub struct NotebookSnapshot {
    pub path: String,
    pub document: NotebookDocument,
    pub connection: ConnectionState,
    pub session: Option<Session>,
    pub kernel_status: Option<String>,
    pub executions: BTreeMap<CellId, CellExecution>,
    pub pending_inputs: BTreeMap<String, PendingInput>,
    pub save: SaveState,
}

#[derive(Debug, Clone)]
pub enum NotebookEvent {
    DocumentChanged {
        revision: u64,
    },
    CellChanged {
        cell_id: CellId,
    },
    ExecutionChanged {
        cell_id: CellId,
        phase: ExecutionPhase,
    },
    ConnectionChanged(ConnectionState),
    KernelStatusChanged(String),
    KernelRestarted,
    InputRequested(PendingInput),
    SaveChanged(SaveState),
    /// Extension/foreign messages remain available without becoming cell output.
    UnhandledMessage(Box<KernelMessage>),
    ProtocolError(String),
}

struct Run {
    cell: CellId,
    tracker: ExecutionTracker,
    outcome: ExecutionPhase,
    clear_pending: bool,
}
pub(super) struct State {
    pub snapshot: NotebookSnapshot,
    runs: HashMap<String, Run>,
    displays: HashMap<String, Vec<(CellId, usize)>>,
    pub input_messages: HashMap<String, KernelMessage>,
    kernel_session: Option<String>,
    retired_sessions: HashSet<String>,
}
impl State {
    pub fn new(path: String, document: NotebookDocument) -> Self {
        Self {
            snapshot: NotebookSnapshot {
                path,
                document,
                connection: ConnectionState::Disconnected { reason: None },
                session: None,
                kernel_status: None,
                executions: BTreeMap::new(),
                pending_inputs: BTreeMap::new(),
                save: SaveState::Idle,
            },
            runs: HashMap::new(),
            displays: HashMap::new(),
            input_messages: HashMap::new(),
            kernel_session: None,
            retired_sessions: HashSet::new(),
        }
    }
    pub fn begin_run(&mut self, cell: &CellId, request_id: String) -> Result<Vec<NotebookEvent>> {
        let source = self.snapshot.document.cell(cell)?.source();
        if self.snapshot.document.cell(cell)?.kind() != CellKind::Code {
            return Err(Error::Invalid("only code cells can run".into()));
        }
        if self
            .snapshot
            .executions
            .get(cell)
            .is_some_and(|run| run.phase.is_active())
        {
            return Err(Error::Unavailable(
                "cell already has an active execution".into(),
            ));
        }
        self.forget_cell(cell);
        self.snapshot.document.clear_outputs(cell)?;
        self.runs.insert(
            request_id.clone(),
            Run {
                cell: cell.clone(),
                tracker: ExecutionTracker::new(&request_id),
                outcome: ExecutionPhase::Succeeded,
                clear_pending: false,
            },
        );
        self.snapshot.executions.insert(
            cell.clone(),
            CellExecution {
                request_id,
                source,
                phase: ExecutionPhase::Queued,
            },
        );
        Ok(vec![
            NotebookEvent::CellChanged {
                cell_id: cell.clone(),
            },
            NotebookEvent::ExecutionChanged {
                cell_id: cell.clone(),
                phase: ExecutionPhase::Queued,
            },
        ])
    }
    pub fn forget_cell(&mut self, cell: &CellId) {
        self.runs.retain(|_, run| run.cell != *cell);
        self.snapshot.executions.remove(cell);
        self.forget_displays(cell);
        self.snapshot
            .pending_inputs
            .retain(|_, input| input.cell_id != *cell);
        self.input_messages
            .retain(|id, _| self.snapshot.pending_inputs.contains_key(id));
    }
    fn forget_displays(&mut self, cell: &CellId) {
        self.displays.retain(|_, targets| {
            targets.retain(|(id, _)| id != cell);
            !targets.is_empty()
        });
    }
    pub fn clear_outputs(&mut self, cell: &CellId) -> Result<()> {
        self.forget_displays(cell);
        // Protocol clear_output affects output areas, not execution counters.
        let count = self.snapshot.document.cell(cell)?.execution_count();
        self.snapshot.document.clear_outputs(cell)?;
        self.snapshot.document.set_count(cell, count)
    }
    pub fn invalidate_runs(&mut self) -> Vec<NotebookEvent> {
        let mut events = vec![];
        for (cell, run) in &mut self.snapshot.executions {
            if run.phase.is_active() {
                run.phase = ExecutionPhase::Unknown;
                events.push(NotebookEvent::ExecutionChanged {
                    cell_id: cell.clone(),
                    phase: ExecutionPhase::Unknown,
                });
            }
        }
        self.runs.clear();
        self.displays.clear();
        self.input_messages.clear();
        self.snapshot.pending_inputs.clear();
        events
    }
    pub fn establish_kernel_session(&mut self, session: String) {
        if let Some(previous) = self.kernel_session.replace(session.clone())
            && previous != session
        {
            self.retired_sessions.insert(previous);
        }
    }
    pub fn disconnected(&mut self, reason: Option<String>) -> Vec<NotebookEvent> {
        let mut events = self.invalidate_runs();
        self.snapshot.connection = ConnectionState::Disconnected { reason };
        self.snapshot.kernel_status = None;
        events.push(NotebookEvent::ConnectionChanged(
            self.snapshot.connection.clone(),
        ));
        events
    }
    fn phase(&mut self, cell: &CellId, phase: ExecutionPhase, events: &mut Vec<NotebookEvent>) {
        if let Some(run) = self.snapshot.executions.get_mut(cell)
            && run.phase != phase
        {
            run.phase = phase.clone();
            events.push(NotebookEvent::ExecutionChanged {
                cell_id: cell.clone(),
                phase,
            });
        }
    }
    fn update_displays(
        &mut self,
        display_id: &str,
        data: &Map<String, Value>,
        metadata: &Map<String, Value>,
        events: &mut Vec<NotebookEvent>,
    ) -> Result<()> {
        if let Some(targets) = self.displays.get(display_id) {
            for (cell, index) in targets {
                self.snapshot.document.update_display(
                    cell,
                    *index,
                    data.clone(),
                    metadata.clone(),
                )?;
                events.push(NotebookEvent::CellChanged {
                    cell_id: cell.clone(),
                });
            }
        }
        Ok(())
    }
    pub fn apply(&mut self, message: KernelMessage) -> Result<Vec<NotebookEvent>> {
        let mut events = vec![];
        if self.retired_sessions.contains(&message.header.session) {
            return Ok(events);
        }
        // Only kernel-originated channels identify the kernel process.
        if matches!(message.channel.as_str(), "iopub" | "shell" | "stdin") {
            if self
                .kernel_session
                .as_ref()
                .is_some_and(|id| id != &message.header.session)
            {
                events.extend(self.invalidate_runs());
                events.push(NotebookEvent::KernelRestarted);
            }
            self.establish_kernel_session(message.header.session.clone());
        }
        let body = message.body()?;
        if let MessageBody::Status(status) = &body
            && message.channel == "iopub"
        {
            self.snapshot.kernel_status = Some(status.execution_state.clone());
            events.push(NotebookEvent::KernelStatusChanged(
                status.execution_state.clone(),
            ));
        }
        if let MessageBody::UpdateDisplayData(display) = &body
            && message.channel == "iopub"
        {
            if let Some(id) = display.transient.get("display_id").and_then(Value::as_str) {
                self.update_displays(
                    id,
                    &display.data.clone().into_iter().collect(),
                    display.metadata.as_object().ok_or_else(|| {
                        Error::Invalid("display metadata must be an object".into())
                    })?,
                    &mut events,
                )?;
            }
            return Ok(events);
        }
        let Some(request_id) = message.parent_id().map(str::to_owned) else {
            events.push(NotebookEvent::UnhandledMessage(Box::new(message)));
            return Ok(events);
        };
        let Some(run) = self.runs.get(&request_id) else {
            events.push(NotebookEvent::UnhandledMessage(Box::new(message)));
            return Ok(events);
        };
        let cell = run.cell.clone();
        if self.snapshot.document.cell(&cell).is_err() {
            return Ok(events);
        }
        let mut output = None;
        let mut display_id = None;
        match body {
            MessageBody::Status(status) if message.channel == "iopub" => {
                if status.execution_state == "busy" {
                    self.phase(&cell, ExecutionPhase::Running, &mut events);
                }
            }
            MessageBody::ExecuteInput(input) if message.channel == "iopub" => {
                self.snapshot
                    .document
                    .set_count(&cell, Some(input.execution_count))?;
            }
            MessageBody::ExecuteReply(reply) if message.channel == "shell" => {
                self.snapshot
                    .document
                    .set_count(&cell, reply.execution_count)?;
                self.runs.get_mut(&request_id).unwrap().outcome = match reply.status.as_str() {
                    "ok" => ExecutionPhase::Succeeded,
                    "aborted" => ExecutionPhase::Aborted,
                    _ => ExecutionPhase::Failed,
                };
            }
            MessageBody::Stream(stream) if message.channel == "iopub" => {
                output = Some(KnownOutput::Stream {
                    name: stream.name,
                    text: Multiline::Text(stream.text),
                    extra: Map::new(),
                });
            }
            MessageBody::Error(error) if message.channel == "iopub" => {
                output = Some(KnownOutput::Error {
                    ename: error.ename,
                    evalue: error.evalue,
                    traceback: error.traceback,
                    extra: Map::new(),
                });
            }
            MessageBody::DisplayData(display) | MessageBody::ExecuteResult(display)
                if message.channel == "iopub" =>
            {
                display_id = display
                    .transient
                    .get("display_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                let data = display.data.into_iter().collect();
                let metadata =
                    display.metadata.as_object().cloned().ok_or_else(|| {
                        Error::Invalid("display metadata must be an object".into())
                    })?;
                if let Some(id) = &display_id {
                    self.update_displays(id, &data, &metadata, &mut events)?;
                }
                output = Some(if message.header.msg_type == "execute_result" {
                    KnownOutput::ExecuteResult {
                        data,
                        metadata,
                        execution_count: display
                            .execution_count
                            .ok_or_else(|| Error::Invalid("missing execution count".into()))?,
                        extra: Map::new(),
                    }
                } else {
                    KnownOutput::DisplayData {
                        data,
                        metadata,
                        extra: Map::new(),
                    }
                });
            }
            MessageBody::ClearOutput(clear) if message.channel == "iopub" => {
                self.runs.get_mut(&request_id).unwrap().clear_pending = clear.wait;
                if !clear.wait {
                    self.clear_outputs(&cell)?;
                    events.push(NotebookEvent::CellChanged {
                        cell_id: cell.clone(),
                    });
                }
            }
            MessageBody::InputRequest(input) if message.channel == "stdin" => {
                let pending = PendingInput {
                    id: message.header.msg_id.clone(),
                    cell_id: cell.clone(),
                    prompt: input.prompt,
                    password: input.password,
                };
                self.snapshot
                    .pending_inputs
                    .insert(pending.id.clone(), pending.clone());
                self.input_messages
                    .insert(pending.id.clone(), message.clone());
                self.phase(&cell, ExecutionPhase::WaitingForInput, &mut events);
                events.push(NotebookEvent::InputRequested(pending));
            }
            _ => events.push(NotebookEvent::UnhandledMessage(Box::new(message.clone()))),
        }
        if let Some(output) = output {
            if self.runs.get(&request_id).unwrap().clear_pending {
                self.clear_outputs(&cell)?;
                self.runs.get_mut(&request_id).unwrap().clear_pending = false;
            }
            let index = self
                .snapshot
                .document
                .push_output(&cell, Output::Known(output))?;
            if let Some(id) = display_id {
                self.displays
                    .entry(id)
                    .or_default()
                    .push((cell.clone(), index));
            }
            events.push(NotebookEvent::CellChanged {
                cell_id: cell.clone(),
            });
        }
        let run = self.runs.get_mut(&request_id).unwrap();
        run.tracker.observe(&message);
        if run.tracker.is_complete() {
            let outcome = run.outcome.clone();
            self.phase(&cell, outcome, &mut events);
            self.snapshot
                .pending_inputs
                .retain(|_, input| input.cell_id != cell);
            self.input_messages
                .retain(|id, _| self.snapshot.pending_inputs.contains_key(id));
        }
        Ok(events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn setup() -> (State, CellId, CellId) {
        let mut document = NotebookDocument::new();
        let a = document.insert_cell(0, CellKind::Code, "a").unwrap();
        let b = document.insert_cell(1, CellKind::Code, "b").unwrap();
        (State::new("test.ipynb".into(), document), a, b)
    }
    fn msg(parent: &str, channel: &str, kind: &str, content: Value) -> KernelMessage {
        let mut message = KernelMessage::request("kernel-session", channel, kind, content);
        message.parent_header = json!({"msg_id":parent});
        message
    }
    fn apply(state: &mut State, parent: &str, channel: &str, kind: &str, content: Value) {
        state.apply(msg(parent, channel, kind, content)).unwrap();
    }
    fn finish(state: &mut State, parent: &str) {
        apply(
            state,
            parent,
            "iopub",
            "status",
            json!({"execution_state":"idle"}),
        );
        apply(
            state,
            parent,
            "shell",
            "execute_reply",
            json!({"status":"ok","execution_count":1}),
        );
    }
    #[test]
    fn correlates_interleaved_executions_late_outputs_and_new_runs() {
        let (mut state, a, b) = setup();
        state.begin_run(&a, "a1".into()).unwrap();
        state.begin_run(&b, "b1".into()).unwrap();
        assert!(state.begin_run(&a, "a2".into()).is_err());
        apply(
            &mut state,
            "b1",
            "iopub",
            "stream",
            json!({"name":"stdout","text":"b"}),
        );
        apply(
            &mut state,
            "a1",
            "iopub",
            "stream",
            json!({"name":"stdout","text":"a"}),
        );
        apply(
            &mut state,
            "foreign",
            "iopub",
            "stream",
            json!({"name":"stdout","text":"ignored"}),
        );
        state
            .snapshot
            .document
            .update_source(&a, "edited while running")
            .unwrap();
        assert_eq!(state.snapshot.executions[&a].source, "a");
        finish(&mut state, "a1");
        assert_eq!(
            state.snapshot.executions[&a].phase,
            ExecutionPhase::Succeeded
        );
        apply(
            &mut state,
            "a1",
            "iopub",
            "stream",
            json!({"name":"stdout","text":"late"}),
        );
        assert_eq!(state.snapshot.document.cell(&a).unwrap().outputs().len(), 2);
        assert_eq!(state.snapshot.document.cell(&b).unwrap().outputs().len(), 1);
        state.begin_run(&a, "a2".into()).unwrap();
        apply(
            &mut state,
            "a1",
            "iopub",
            "stream",
            json!({"name":"stdout","text":"obsolete"}),
        );
        assert!(
            state
                .snapshot
                .document
                .cell(&a)
                .unwrap()
                .outputs()
                .is_empty()
        );
        state.snapshot.document.remove_cell(&b).unwrap();
        state.forget_cell(&b);
        apply(
            &mut state,
            "b1",
            "iopub",
            "stream",
            json!({"name":"stdout","text":"deleted"}),
        );
    }
    #[test]
    fn display_updates_cross_cells_and_clear_wait_removes_stale_targets() {
        let (mut state, a, b) = setup();
        state.begin_run(&a, "a".into()).unwrap();
        state.begin_run(&b, "b".into()).unwrap();
        let display = |text| json!({"data":{"text/plain":text},"metadata":{},"transient":{"display_id":"shared"}});
        apply(&mut state, "a", "iopub", "display_data", display("first"));
        apply(&mut state, "b", "iopub", "display_data", display("second"));
        apply(
            &mut state,
            "foreign",
            "iopub",
            "update_display_data",
            display("updated"),
        );
        for cell in [&a, &b] {
            let output =
                serde_json::to_value(&state.snapshot.document.cell(cell).unwrap().outputs()[0])
                    .unwrap();
            assert_eq!(output["data"]["text/plain"], "updated");
            assert!(output.get("transient").is_none());
        }
        apply(
            &mut state,
            "a",
            "iopub",
            "execute_input",
            json!({"code":"a","execution_count":8}),
        );
        apply(
            &mut state,
            "a",
            "iopub",
            "clear_output",
            json!({"wait":true}),
        );
        assert_eq!(state.snapshot.document.cell(&a).unwrap().outputs().len(), 1);
        apply(
            &mut state,
            "a",
            "iopub",
            "stream",
            json!({"name":"stdout","text":"replacement"}),
        );
        assert_eq!(
            state.snapshot.document.cell(&a).unwrap().execution_count(),
            Some(8)
        );
        apply(
            &mut state,
            "foreign",
            "iopub",
            "update_display_data",
            display("new"),
        );
        assert_eq!(
            serde_json::to_value(&state.snapshot.document.cell(&a).unwrap().outputs()[0]).unwrap()
                ["text"],
            "replacement"
        );
        assert_eq!(
            serde_json::to_value(&state.snapshot.document.cell(&b).unwrap().outputs()[0]).unwrap()
                ["data"]["text/plain"],
            "new"
        );
        apply(
            &mut state,
            "b",
            "iopub",
            "clear_output",
            json!({"wait":false}),
        );
        assert!(
            state
                .snapshot
                .document
                .cell(&b)
                .unwrap()
                .outputs()
                .is_empty()
        );
    }
    #[test]
    fn errors_stdin_disconnect_and_restart_preserve_document() {
        let (mut state, a, b) = setup();
        state.begin_run(&a, "a".into()).unwrap();
        apply(
            &mut state,
            "a",
            "stdin",
            "input_request",
            json!({"prompt":"secret","password":true}),
        );
        assert_eq!(
            state.snapshot.executions[&a].phase,
            ExecutionPhase::WaitingForInput
        );
        assert!(
            state
                .snapshot
                .pending_inputs
                .values()
                .next()
                .unwrap()
                .password
        );
        state.disconnected(Some("lost connection".into()));
        assert_eq!(state.snapshot.executions[&a].phase, ExecutionPhase::Unknown);
        assert!(state.snapshot.pending_inputs.is_empty());
        assert_eq!(state.snapshot.document.cell(&a).unwrap().source(), "a");
        state.begin_run(&a, "a2".into()).unwrap();
        apply(
            &mut state,
            "a2",
            "iopub",
            "error",
            json!({"ename":"ValueError","evalue":"oops","traceback":["trace"]}),
        );
        apply(
            &mut state,
            "a2",
            "shell",
            "execute_reply",
            json!({"status":"error","execution_count":2}),
        );
        apply(
            &mut state,
            "a2",
            "iopub",
            "status",
            json!({"execution_state":"idle"}),
        );
        assert_eq!(state.snapshot.executions[&a].phase, ExecutionPhase::Failed);
        state.begin_run(&b, "b".into()).unwrap();
        let mut restart = msg("", "iopub", "status", json!({"execution_state":"starting"}));
        restart.header.session = "new-kernel".into();
        assert!(
            state
                .apply(restart)
                .unwrap()
                .iter()
                .any(|e| matches!(e, NotebookEvent::KernelRestarted))
        );
        assert_eq!(state.snapshot.executions[&b].phase, ExecutionPhase::Unknown);
        assert_eq!(state.snapshot.document.cell(&a).unwrap().outputs().len(), 1);
        state.begin_run(&b, "b2".into()).unwrap();
        // A buffered message from the retired kernel must not invalidate b2.
        apply(
            &mut state,
            "b",
            "iopub",
            "status",
            json!({"execution_state":"idle"}),
        );
        assert_eq!(state.snapshot.executions[&b].phase, ExecutionPhase::Queued);
    }
}
