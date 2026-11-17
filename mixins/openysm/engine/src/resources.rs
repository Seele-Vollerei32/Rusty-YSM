//! One worker, one pending request and one result: obsolete generations never publish.
use crate::{bundle::ModelBundle, package::PackageLimits};
use std::{
    path::PathBuf,
    sync::{Arc, Condvar, Mutex},
    thread::{self, JoinHandle},
};
pub struct LoadedBundle {
    pub generation: u64,
    pub result: Result<Arc<ModelBundle>, String>,
}
struct State {
    generation: u64,
    pending: Option<(u64, PathBuf)>,
    result: Option<LoadedBundle>,
    stopping: bool,
}
struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}
pub struct ResourceService {
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
}
impl ResourceService {
    pub fn new(limits: PackageLimits) -> Result<Self, String> {
        Self::with_loader(move |path| ModelBundle::open(path, limits).map(Arc::new))
    }
    fn with_loader(
        load: impl Fn(&std::path::Path) -> Result<Arc<ModelBundle>, String> + Send + 'static,
    ) -> Result<Self, String> {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                generation: 0,
                pending: None,
                result: None,
                stopping: false,
            }),
            wake: Condvar::new(),
        });
        let background = shared.clone();
        let worker = thread::Builder::new()
            .name("openysm-resources".into())
            .spawn(move || {
                loop {
                    let (generation, path) = {
                        let mut state = background.state.lock().unwrap();
                        while state.pending.is_none() && !state.stopping {
                            state = background.wake.wait(state).unwrap();
                        }
                        if state.stopping {
                            break;
                        }
                        state.pending.take().unwrap()
                    };
                    let result = load(&path);
                    let mut state = background.state.lock().unwrap();
                    if state.stopping {
                        break;
                    }
                    if state.generation == generation {
                        state.result = Some(LoadedBundle { generation, result });
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            shared,
            worker: Some(worker),
        })
    }
    pub fn request(&self, path: impl Into<PathBuf>) -> u64 {
        let mut state = self.shared.state.lock().unwrap();
        state.generation = state.generation.wrapping_add(1);
        let generation = state.generation;
        state.pending = Some((generation, path.into()));
        state.result = None;
        self.shared.wake.notify_one();
        generation
    }
    pub fn cancel(&self) {
        let mut state = self.shared.state.lock().unwrap();
        state.generation = state.generation.wrapping_add(1);
        state.pending = None;
        state.result = None;
    }
    pub fn try_recv(&self) -> Option<LoadedBundle> {
        self.shared.state.lock().unwrap().result.take()
    }
}
impl ResourceService {
    fn stop(&self) {
        let mut state = self.shared.state.lock().unwrap();
        state.stopping = true;
        state.pending = None;
        state.result = None;
        self.shared.wake.notify_one();
    }
    /// Stops the worker and waits for it; a load in progress finishes first.
    pub fn shutdown(mut self) {
        self.stop();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
impl Drop for ResourceService {
    /// Tells the worker to stop without waiting for it. The service may be
    /// dropped from a thread-local destructor while the process exits, where
    /// waiting for another thread can deadlock.
    fn drop(&mut self) {
        self.stop();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::mpsc,
        time::{Duration, Instant},
    };
    #[test]
    fn superseded_and_cancelled_generations_do_not_publish() {
        let (started_tx, started_rx) = mpsc::channel();
        let (continue_tx, continue_rx) = mpsc::channel();
        let service = ResourceService::with_loader(move |path| {
            started_tx.send(path.to_owned()).unwrap();
            continue_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            Err(path.display().to_string())
        })
        .unwrap();
        service.request("old");
        assert_eq!(
            started_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            PathBuf::from("old")
        );
        let current = service.request("new");
        continue_tx.send(()).unwrap();
        assert_eq!(
            started_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            PathBuf::from("new")
        );
        assert!(service.try_recv().is_none());
        continue_tx.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let response = loop {
            if let Some(result) = service.try_recv() {
                break result;
            }
            assert!(Instant::now() < deadline);
            thread::yield_now();
        };
        assert_eq!(response.generation, current);
        assert_eq!(response.result.err().unwrap(), "new");
        service.request("cancelled");
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        service.cancel();
        continue_tx.send(()).unwrap();
        // Joining guarantees the cancelled job has finished before inspecting its mailbox.
        let shared = service.shared.clone();
        service.shutdown();
        assert!(shared.state.lock().unwrap().result.is_none());
    }
}
