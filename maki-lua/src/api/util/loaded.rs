use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use arc_swap::ArcSwap;

/// Names of the plugins the Lua thread holds in its `PluginMap`, published
/// after every load and unload. The UI needs "is this plugin loaded" for its
/// own plugins, which it cannot infer from the tool registry: a plugin that
/// only registers commands or autocmds never shows up there.
#[derive(Default)]
pub struct LoadedPlugins {
    names: Vec<Arc<str>>,
    generation: u64,
}

impl LoadedPlugins {
    pub fn contains(&self, name: &str) -> bool {
        self.names
            .binary_search_by(|candidate| candidate.as_ref().cmp(name))
            .is_ok()
    }

    /// Bumped on every publish, so a reader can notice a change cheaply.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn names(&self) -> &[Arc<str>] {
        &self.names
    }

    fn sorted(mut names: Vec<Arc<str>>, generation: u64) -> Self {
        names.sort_unstable_by(|a, b| a.as_ref().cmp(b.as_ref()));
        names.dedup();
        Self { names, generation }
    }
}

#[derive(Clone)]
pub struct LoadedPluginsReader(Arc<ArcSwap<LoadedPlugins>>);

impl LoadedPluginsReader {
    pub fn empty() -> Self {
        Self(Arc::new(ArcSwap::from_pointee(LoadedPlugins::default())))
    }

    /// A settled snapshot for tests that drive the UI without a Lua host.
    pub fn from_names<S: AsRef<str>>(names: impl IntoIterator<Item = S>) -> Self {
        let names = names
            .into_iter()
            .map(|name| Arc::from(name.as_ref()))
            .collect();
        Self(Arc::new(ArcSwap::from_pointee(LoadedPlugins::sorted(
            names, 1,
        ))))
    }

    /// Full `Arc` rather than a `Guard`: a reader keeps the last one alive to
    /// compare its generation against the next.
    pub fn load_full(&self) -> Arc<LoadedPlugins> {
        self.0.load_full()
    }
}

/// Publishing end of the loaded-plugin set, owned by the Lua thread.
pub(crate) struct LoadedPluginsWriter {
    store: Arc<ArcSwap<LoadedPlugins>>,
    generation: AtomicU64,
}

impl LoadedPluginsWriter {
    pub(crate) fn new() -> (Self, LoadedPluginsReader) {
        let inner = Arc::new(ArcSwap::from_pointee(LoadedPlugins::default()));
        (
            Self {
                store: Arc::clone(&inner),
                generation: AtomicU64::new(0),
            },
            LoadedPluginsReader(inner),
        )
    }

    pub(crate) fn publish(&self, names: Vec<Arc<str>>) {
        let generation = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        self.store
            .store(Arc::new(LoadedPlugins::sorted(names, generation)));
    }
}
