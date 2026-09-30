use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use arc_swap::ArcSwap;
use serde_json::Value;

/// One tool a loaded plugin registered: the description the model reads and
/// the input schema it has to satisfy.
#[derive(Clone, Debug)]
pub struct PluginToolInfo {
    pub name: Arc<str>,
    pub description: String,
    pub schema: Value,
}

/// A plugin the Lua thread holds in its `PluginMap`, with the tools it
/// registered.
#[derive(Debug)]
pub struct LoadedPlugin {
    pub name: Arc<str>,
    pub tools: Vec<PluginToolInfo>,
}

/// The plugins the Lua thread holds in its `PluginMap`, published after every
/// load and unload. The UI needs "is this plugin loaded" for its own plugins,
/// which it cannot infer from the tool registry: a plugin that only registers
/// commands or autocmds never shows up there. The tools ride along so the UI
/// can describe a plugin without calling back into the Lua thread.
#[derive(Default)]
pub struct LoadedPlugins {
    plugins: Vec<LoadedPlugin>,
    generation: u64,
}

impl LoadedPlugins {
    pub fn contains(&self, name: &str) -> bool {
        self.position(name).is_some()
    }

    pub fn plugin(&self, name: &str) -> Option<&LoadedPlugin> {
        self.position(name).map(|i| &self.plugins[i])
    }

    pub fn plugins(&self) -> &[LoadedPlugin] {
        &self.plugins
    }

    fn position(&self, name: &str) -> Option<usize> {
        self.plugins
            .binary_search_by(|candidate| candidate.name.as_ref().cmp(name))
            .ok()
    }

    /// Bumped on every publish, so a reader can notice a change cheaply.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    fn sorted(mut plugins: Vec<LoadedPlugin>, generation: u64) -> Self {
        plugins.sort_unstable_by(|a, b| a.name.as_ref().cmp(&b.name));
        plugins.dedup_by(|a, b| a.name == b.name);
        Self {
            plugins,
            generation,
        }
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
        Self::from_plugins(
            names
                .into_iter()
                .map(|name| LoadedPlugin {
                    name: Arc::from(name.as_ref()),
                    tools: Vec::new(),
                })
                .collect(),
        )
    }

    /// Same, for snapshots that carry tool info.
    pub fn from_plugins(plugins: Vec<LoadedPlugin>) -> Self {
        Self(Arc::new(ArcSwap::from_pointee(LoadedPlugins::sorted(
            plugins, 1,
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

    pub(crate) fn publish(&self, plugins: Vec<LoadedPlugin>) {
        let generation = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        self.store
            .store(Arc::new(LoadedPlugins::sorted(plugins, generation)));
    }
}
