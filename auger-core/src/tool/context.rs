use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;

/// The working directory shared by the calls of one session.
#[derive(Clone, Debug)]
pub(crate) struct WorkDir(Arc<Mutex<PathBuf>>);

impl WorkDir {
    pub(crate) fn new(dir: PathBuf) -> Self {
        Self(Arc::new(Mutex::new(dir)))
    }

    pub(crate) fn get(&self) -> PathBuf {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn set(&self, dir: PathBuf) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = dir;
    }
}

/// What a tool call runs against.
pub struct ToolContext {
    cwd: PathBuf,
    workdir: WorkDir,
}

impl ToolContext {
    /// A context with its own working directory, not shared with a session.
    pub fn new(cwd: PathBuf) -> Self {
        let workdir = WorkDir::new(cwd.clone());
        Self { cwd, workdir }
    }

    pub(crate) fn for_session(cwd: PathBuf, workdir: WorkDir) -> Self {
        Self { cwd, workdir }
    }

    /// The working directory this call runs in: the session's directory
    /// when the call was checked by policy.
    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    /// Resolve a path argument against [`Self::cwd`].
    pub fn resolve(&self, path: &str) -> PathBuf {
        self.cwd.join(path)
    }

    /// Move the session's working directory for later calls.
    pub fn set_cwd(&self, dir: PathBuf) {
        self.workdir.set(dir);
    }
}
