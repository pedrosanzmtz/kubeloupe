//! The running log streams (pod and workload), keyed by the stream id the UI
//! picked. Each dock tab has its own stream; every stream is an open
//! connection to the API server, so there is a cap.

use std::collections::HashMap;
use std::sync::Mutex;
use tauri::async_runtime::JoinHandle;

/// At most this many log streams run at once.
pub(crate) const MAX_LOG_STREAMS: usize = 10;

#[derive(Default)]
pub(crate) struct LogStreams {
    tasks: Mutex<HashMap<u64, JoinHandle<()>>>,
}

impl LogStreams {
    /// Start stream `id` with `spawn`, unless `MAX_LOG_STREAMS` other streams
    /// are still running. A stream already running under `id` is replaced.
    /// The check and the insert happen under one lock, so two starts cannot
    /// both take the last slot.
    pub(crate) fn start(
        &self,
        id: u64,
        spawn: impl FnOnce() -> JoinHandle<()>,
    ) -> Result<(), String> {
        let mut tasks = self.tasks.lock().unwrap();
        // Streams that ended on their own (pod gone, container exited) no
        // longer count.
        tasks.retain(|_, t| !t.inner().is_finished());
        if let Some(old) = tasks.remove(&id) {
            old.abort();
        }
        if tasks.len() >= MAX_LOG_STREAMS {
            return Err(format!(
                "Too many log streams: {MAX_LOG_STREAMS} are already open. Close a log tab to open another."
            ));
        }
        tasks.insert(id, spawn());
        Ok(())
    }

    /// Stop stream `id`, if it is still running.
    pub(crate) fn stop(&self, id: u64) {
        if let Some(task) = self.tasks.lock().unwrap().remove(&id) {
            task.abort();
        }
    }

    pub(crate) fn stop_all(&self) {
        for (_, task) in self.tasks.lock().unwrap().drain() {
            task.abort();
        }
    }

    #[cfg(test)]
    fn running(&self) -> usize {
        let mut tasks = self.tasks.lock().unwrap();
        tasks.retain(|_, t| !t.inner().is_finished());
        tasks.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn forever() -> JoinHandle<()> {
        tauri::async_runtime::spawn(std::future::pending())
    }

    #[test]
    fn caps_concurrent_streams_and_frees_slots_on_stop() {
        let streams = LogStreams::default();
        for id in 0..MAX_LOG_STREAMS as u64 {
            streams.start(id, forever).unwrap();
        }
        let err = streams.start(100, forever).unwrap_err();
        assert!(err.contains("Too many log streams"), "{err}");
        assert_eq!(streams.running(), MAX_LOG_STREAMS);

        streams.stop(3);
        streams.start(100, forever).unwrap();
        assert_eq!(streams.running(), MAX_LOG_STREAMS);

        streams.stop_all();
        assert_eq!(streams.running(), 0);
    }

    #[test]
    fn restarting_an_id_replaces_its_stream() {
        let streams = LogStreams::default();
        for id in 0..MAX_LOG_STREAMS as u64 {
            streams.start(id, forever).unwrap();
        }
        // Same id while at the cap: the old task is replaced, not counted.
        streams.start(0, forever).unwrap();
        assert_eq!(streams.running(), MAX_LOG_STREAMS);
    }

    #[test]
    fn finished_streams_do_not_count() {
        let streams = LogStreams::default();
        for id in 0..MAX_LOG_STREAMS as u64 {
            streams
                .start(id, || tauri::async_runtime::spawn(async {}))
                .unwrap();
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while streams.running() > 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        streams.start(100, forever).unwrap();
        assert_eq!(streams.running(), 1);
    }
}
