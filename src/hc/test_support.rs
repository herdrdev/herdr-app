use super::*;

pub(crate) fn recording_input() -> (Arc<Core>, mpsc::Receiver<Vec<u8>>) {
    let (writer, frames) = mpsc::channel();
    (offline_with_writer(writer), frames)
}

/// A core that is never there: calls fail at once, without reporting a lost connection.
pub(crate) fn offline() -> Arc<Core> {
    let (writer, frames) = mpsc::channel();
    std::mem::forget(frames);
    offline_with_writer(writer)
}

fn offline_with_writer(writer: mpsc::Sender<Vec<u8>>) -> Arc<Core> {
    let (lost_tx, lost) = async_channel::bounded(1);
    std::mem::forget(lost_tx);
    let child = std::process::Command::new(std::env::current_exe().unwrap()).arg("--help").stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
    Arc::new(Core {
        hello: Value::Null,
        writer,
        routes: Arc::new(Mutex::new(Routes { closed: true, ..Default::default() })),
        next_id: AtomicU64::new(1),
        child: Mutex::new(child),
        lost,
    })
}
