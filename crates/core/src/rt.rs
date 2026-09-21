//! The async runtime the core runs on. Inside the tray app that is Tauri's
//! tokio runtime, picked up from the calling context; with no runtime around
//! (tests, a future headless agent's sync entry points) a shared one is
//! started on first use. Keeps the core free of a Tauri dependency.

use std::future::Future;
use std::sync::OnceLock;
use tokio::runtime::{Handle, Runtime};
use tokio::task::JoinHandle;

fn fallback() -> &'static Runtime {
    static RT: OnceLock<Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("start tokio runtime")
    })
}

fn handle() -> Handle {
    Handle::try_current().unwrap_or_else(|_| fallback().handle().clone())
}

pub fn spawn<F>(future: F) -> JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    handle().spawn(future)
}

pub fn spawn_blocking<F, R>(f: F) -> JoinHandle<R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    handle().spawn_blocking(f)
}

/// Runs a future to completion from synchronous code. Must not be called
/// from inside the runtime.
pub fn block_on<F: Future>(future: F) -> F::Output {
    fallback().block_on(future)
}
