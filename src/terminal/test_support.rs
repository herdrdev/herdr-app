use super::TerminalView;

pub(crate) fn record_input(terminal: &mut TerminalView) -> std::sync::mpsc::Receiver<Vec<u8>> {
    terminal._reader = None;
    let (core, frames) = crate::hc::test_support::recording_input();
    let mut grid = terminal.grid.borrow_mut();
    grid.core = core;
    grid.channel = Some(1);
    frames
}
