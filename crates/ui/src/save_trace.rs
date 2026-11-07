//! Optional bounded diagnostic events for save decisions.
use std::{
    fs::OpenOptions,
    io::Write,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender},
    },
    thread::{self, JoinHandle},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SaveOperationId(pub u64);
#[derive(Clone, Copy, Debug)]
pub(crate) enum SaveTraceEvent {
    Requested,
    SnapshotCaptured,
    ConflictObserved { observation: u64 },
    PromptPresented { observation: u64 },
    ResponseSelected { index: usize, observation: u64 },
    WriteStarted { observation: Option<u64> },
    WriteAcknowledged,
    Cancelled,
    Failed,
    PendingIntentApplied,
}
impl SaveTraceEvent {
    fn value(self) -> serde_json::Value {
        match self {
            Self::Requested => serde_json::json!({"kind":"requested"}),
            Self::SnapshotCaptured => serde_json::json!({"kind":"snapshot_captured"}),
            Self::ConflictObserved { observation } => {
                serde_json::json!({"kind":"conflict_observed","observation":observation})
            }
            Self::PromptPresented { observation } => {
                serde_json::json!({"kind":"prompt_presented","observation":observation})
            }
            Self::ResponseSelected { index, observation } => {
                serde_json::json!({"kind":"response_selected","index":index,"observation":observation})
            }
            Self::WriteStarted { observation } => {
                serde_json::json!({"kind":"write_started","observation":observation})
            }
            Self::WriteAcknowledged => serde_json::json!({"kind":"write_acknowledged"}),
            Self::Cancelled => serde_json::json!({"kind":"cancelled"}),
            Self::Failed => serde_json::json!({"kind":"failed"}),
            Self::PendingIntentApplied => serde_json::json!({"kind":"pending_intent_applied"}),
        }
    }
}

pub(crate) struct SaveTrace {
    sender: Option<SyncSender<String>>,
    incomplete: Arc<AtomicBool>,
    _worker: Option<JoinHandle<()>>,
    sequence: u64,
    next_operation: u64,
}
impl SaveTrace {
    pub(crate) fn disabled() -> Self {
        Self {
            sender: None,
            incomplete: Arc::new(AtomicBool::new(false)),
            _worker: None,
            sequence: 0,
            next_operation: 0,
        }
    }
    pub(crate) fn from_environment() -> Self {
        match std::env::var_os("ALE_SAVE_TRACE").map(PathBuf::from) {
            Some(path) if path.is_absolute() => Self::with_path(path, 1024 * 1024, 256),
            Some(_) => {
                eprintln!("Save trace requires an absolute path; tracing disabled.");
                Self::disabled()
            }
            None => Self::disabled(),
        }
    }
    pub(crate) fn with_path(path: PathBuf, limit: usize, capacity: usize) -> Self {
        let (sender, receiver) = mpsc::sync_channel::<String>(capacity);
        let incomplete = Arc::new(AtomicBool::new(false));
        let state = incomplete.clone();
        let worker = thread::Builder::new()
            .name("save-trace".into())
            .spawn(move || {
                let mut options = OpenOptions::new();
                options.write(true).create_new(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o600);
                }
                let Ok(mut file) = options.open(path) else {
                    state.store(true, Ordering::Release);
                    eprintln!("Could not create save trace; tracing incomplete.");
                    return;
                };
                const END_RESERVE: usize = 32;
                let mut written = 0usize;
                for line in receiver {
                    if state.load(Ordering::Acquire) {
                        continue;
                    }
                    if written
                        .saturating_add(line.len())
                        .saturating_add(END_RESERVE)
                        > limit
                        || file.write_all(line.as_bytes()).is_err()
                    {
                        state.store(true, Ordering::Release);
                        eprintln!("Save trace incomplete: output limit or write failure.");
                        continue;
                    }
                    written += line.len();
                }
                let end = if state.load(Ordering::Acquire) {
                    "{\"complete\":false}\n"
                } else {
                    "{\"complete\":true}\n"
                };
                if written.saturating_add(end.len()) <= limit
                    && file
                        .write_all(end.as_bytes())
                        .and_then(|_| file.flush())
                        .is_err()
                {
                    state.store(true, Ordering::Release);
                    eprintln!("Save trace incomplete: final write failed.");
                }
            });
        let worker = match worker {
            Ok(worker) => worker,
            Err(_) => {
                incomplete.store(true, Ordering::Release);
                eprintln!("Could not start save trace worker; tracing incomplete.");
                return Self {
                    sender: None,
                    incomplete,
                    _worker: None,
                    sequence: 0,
                    next_operation: 0,
                };
            }
        };
        Self {
            sender: Some(sender),
            incomplete,
            _worker: Some(worker),
            sequence: 0,
            next_operation: 0,
        }
    }
    pub(crate) fn begin(&mut self, revision: u64) -> SaveOperationId {
        self.next_operation = self.next_operation.saturating_add(1);
        let id = SaveOperationId(self.next_operation);
        self.record(id, revision, SaveTraceEvent::Requested);
        id
    }
    pub(crate) fn record(
        &mut self,
        operation: SaveOperationId,
        revision: u64,
        event: SaveTraceEvent,
    ) {
        let Some(sender) = &self.sender else {
            return;
        };
        if self.incomplete.load(Ordering::Acquire) {
            return;
        }
        self.sequence = self.sequence.saturating_add(1);
        let line = format!(
            "{}\n",
            serde_json::json!({"sequence":self.sequence,"operation":operation.0,"revision":revision,"event":event.value()})
        );
        if sender.try_send(line).is_err() {
            self.incomplete.store(true, Ordering::Release);
            eprintln!("Save trace incomplete: event queue unavailable.");
        }
    }
    #[cfg(test)]
    fn with_memory(capacity: usize) -> (Self, mpsc::Receiver<String>) {
        let (sender, receiver) = mpsc::sync_channel(capacity);
        (
            Self {
                sender: Some(sender),
                incomplete: Arc::new(AtomicBool::new(false)),
                _worker: None,
                sequence: 0,
                next_operation: 0,
            },
            receiver,
        )
    }
    #[cfg(test)]
    pub(crate) fn finish_for_test(mut self) -> bool {
        self.sender.take();
        if let Some(worker) = self._worker.take() {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
            while !worker.is_finished() && std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            if !worker.is_finished() || worker.join().is_err() {
                return false;
            }
        }
        !self.incomplete.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn in_memory_sink_collects_events_without_environment_or_files() {
        let (mut trace, receiver) = SaveTrace::with_memory(8);
        let operation = trace.begin(7);
        trace.record(operation, 7, SaveTraceEvent::Cancelled);
        assert!(trace.finish_for_test());
        let rows: Vec<serde_json::Value> = receiver
            .try_iter()
            .map(|line| serde_json::from_str(&line).unwrap())
            .collect();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["sequence"], 1);
        assert_eq!(rows[0]["event"]["kind"], "requested");
        assert_eq!(rows[1]["sequence"], 2);
        assert_eq!(rows[1]["event"]["kind"], "cancelled");
    }
    #[test]
    fn trace_is_disabled_by_default() {
        if std::env::var_os("ALE_SAVE_TRACE").is_none() {
            assert!(SaveTrace::from_environment().finish_for_test());
        }
        let mut trace = SaveTrace::disabled();
        let id = trace.begin(1);
        trace.record(id, 1, SaveTraceEvent::Cancelled);
        assert!(trace.finish_for_test());
    }
    #[test]
    fn trace_never_truncates_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trace");
        std::fs::write(&path, b"sentinel").unwrap();
        let mut trace = SaveTrace::with_path(path.clone(), 1024, 256);
        trace.begin(1);
        assert!(!trace.finish_for_test());
        assert_eq!(std::fs::read(path).unwrap(), b"sentinel");
    }
    #[test]
    fn queue_overflow_and_sink_failure_are_incomplete() {
        let (sender, _receiver) = mpsc::sync_channel(0);
        let incomplete = Arc::new(AtomicBool::new(false));
        let mut trace = SaveTrace {
            sender: Some(sender),
            incomplete,
            _worker: None,
            sequence: 0,
            next_operation: 0,
        };
        trace.begin(1);
        assert!(!trace.finish_for_test());

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("absent").join("trace");
        let mut trace = SaveTrace::with_path(path, 1024, 256);
        trace.begin(1);
        assert!(!trace.finish_for_test());
    }
    #[test]
    fn trace_has_monotonic_sequences_and_budget() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trace");
        let mut trace = SaveTrace::with_path(path.clone(), 1024, 256);
        let id = trace.begin(2);
        trace.record(id, 2, SaveTraceEvent::ConflictObserved { observation: 4 });
        assert!(trace.finish_for_test());
        let text = std::fs::read_to_string(&path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let rows: Vec<serde_json::Value> = text
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(rows[0]["sequence"], 1);
        assert_eq!(rows[1]["sequence"], 2);
        assert_eq!(rows[2]["complete"], true);
        let small = dir.path().join("small");
        let mut trace = SaveTrace::with_path(small.clone(), 64, 256);
        trace.begin(2);
        assert!(!trace.finish_for_test());
        assert!(std::fs::metadata(small).unwrap().len() <= 64);
    }
    #[test]
    fn trace_records_no_fixture_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trace");
        let mut trace = SaveTrace::with_path(path.clone(), 4096, 256);
        let id = trace.begin(3);
        trace.record(id, 3, SaveTraceEvent::WriteStarted { observation: None });
        for event in [
            SaveTraceEvent::SnapshotCaptured,
            SaveTraceEvent::PromptPresented { observation: 1 },
            SaveTraceEvent::ResponseSelected {
                index: 0,
                observation: 1,
            },
            SaveTraceEvent::WriteAcknowledged,
            SaveTraceEvent::Failed,
            SaveTraceEvent::PendingIntentApplied,
        ] {
            trace.record(id, 3, event);
        }
        assert!(trace.finish_for_test());
        assert!(
            !std::fs::read_to_string(path)
                .unwrap()
                .contains("private-payload")
        );
    }
}
