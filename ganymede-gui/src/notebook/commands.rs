use super::*;
pub(crate) enum Command {
    CutCell,
    CopyCell,
    PasteCell,
    DeleteCell,
    DuplicateCell,
    InsertAbove,
    InsertBelow,
    MoveCellUp,
    MoveCellDown,
    ClearOutput,
    ClearAllOutputs,
    RunCell,
    RunAdvance,
    RunInsert,
    RunAbove,
    RunBelow,
    RunAll,
    RenderMarkdown,
    Interrupt,
    Restart,
    RestartRunAll,
    Reconnect,
    ChangeKernel,
}
impl Notebook {
    fn emit_selected(&self, event: CellEvent, cx: &mut Context<Self>) {
        self.cells[self.selected].update(cx, |_, cx| cx.emit(event));
    }
    fn clear_menu_outputs(&mut self, all: bool, cx: &mut Context<Self>) {
        for index in 0..self.cells.len() {
            if (!all && index != self.selected) || self.runs.active == Some(index) {
                continue;
            }
            let cell = &self.cells[index];
            if cell.read(cx).kind() != CellKind::Code {
                continue;
            }
            cell.update(cx, |cell, cx| cell.clear_execution(cx));
            self.save_error = self.session.clear_outputs(cell.entity_id().as_u64()).err();
        }
    }
    fn run_range(&mut self, range: std::ops::Range<usize>, cx: &mut Context<Self>) {
        if self.control_pending || self.restart_run_all || self.file_busy {
            return;
        }
        for index in range {
            self.cells[index].update(cx, |cell, cx| cell.preview_markdown(cx));
            if self.cells[index].read(cx).kind() != CellKind::Code {
                continue;
            }
            let source = self.cells[index].read(cx).text().to_owned();
            if let Some(count) = self.counter.run(&source) {
                self.runs.pending.push_back((index, count, source));
            }
        }
        self.start_next_run(cx);
    }
    pub(crate) fn menu_command(
        &mut self,
        command: Command,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if window.has_active_prompt() {
            return;
        }
        match command {
            Command::CutCell => self.toolbar_action(ToolbarAction::Cut, window, cx),
            Command::CopyCell => self.toolbar_action(ToolbarAction::Copy, window, cx),
            Command::PasteCell => self.toolbar_action(ToolbarAction::Paste, window, cx),
            Command::DeleteCell => self.emit_selected(CellEvent::DeleteCell, cx),
            Command::DuplicateCell => self.emit_selected(CellEvent::Duplicate, cx),
            Command::InsertAbove => self.emit_selected(CellEvent::InsertAbove, cx),
            Command::InsertBelow => self.emit_selected(CellEvent::InsertBelow, cx),
            Command::MoveCellUp => self.emit_selected(CellEvent::MoveUp, cx),
            Command::MoveCellDown => self.emit_selected(CellEvent::MoveDown, cx),
            Command::ClearOutput => self.clear_menu_outputs(false, cx),
            Command::ClearAllOutputs => self.clear_menu_outputs(true, cx),
            Command::RunCell => self.toolbar_action(ToolbarAction::Run, window, cx),
            Command::RunAdvance => self.emit_selected(CellEvent::RunAndAdvance, cx),
            Command::RunInsert => {
                if self.control_pending || self.restart_run_all || self.file_busy {
                    return;
                }
                self.toolbar_action(ToolbarAction::Run, window, cx);
                self.emit_selected(CellEvent::InsertBelow, cx);
            }
            Command::RunAbove => self.run_range(0..self.selected, cx),
            Command::RunBelow => self.run_range(self.selected..self.cells.len(), cx),
            Command::RunAll => self.run_range(0..self.cells.len(), cx),
            Command::RenderMarkdown => {
                for cell in &self.cells {
                    cell.update(cx, |cell, cx| cell.preview_markdown(cx));
                }
            }
            Command::Interrupt => self.toolbar_action(ToolbarAction::Stop, window, cx),
            Command::Restart => self.toolbar_action(ToolbarAction::Restart, window, cx),
            Command::RestartRunAll => self.toolbar_action(ToolbarAction::RunAll, window, cx),
            Command::Reconnect => self.reconnect(cx),
            Command::ChangeKernel => self.toggle_kernel_menu(window, cx),
        }
        self.sync(cx);
        cx.notify();
    }
}
