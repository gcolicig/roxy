use std::sync::{Arc, RwLock};

use super::cli::ContainerCli;
use crate::application::ports::RegistrationProvider;
use crate::domain::DomainRegistration;

/// Supplies domain registrations discovered from Apple Container.
///
/// Mirrors `DockerProvider`: the watcher task refreshes the shared state,
/// `load()` reads it. `std::sync::RwLock` is enough — contention is
/// near-zero and the trait is sync.
pub struct AppleContainerProvider {
    state: Arc<RwLock<Vec<DomainRegistration>>>,
    cli: ContainerCli,
}

impl AppleContainerProvider {
    pub fn new(cli: ContainerCli) -> Self {
        Self {
            state: Arc::new(RwLock::new(Vec::new())),
            cli,
        }
    }

    /// Shared reference to the internal state, for use by the watcher task.
    pub fn state(&self) -> Arc<RwLock<Vec<DomainRegistration>>> {
        self.state.clone()
    }

    /// Reference to the CLI wrapper, for use by the watcher task.
    pub fn cli(&self) -> &ContainerCli {
        &self.cli
    }
}

impl RegistrationProvider for AppleContainerProvider {
    fn name(&self) -> &str {
        "apple-container"
    }

    fn load(&self) -> anyhow::Result<Vec<DomainRegistration>> {
        Ok(self
            .state
            .read()
            .map_err(|e| anyhow::anyhow!("Apple Container provider lock poisoned: {e}"))?
            .clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{DomainName, DomainPattern, Route};

    fn provider() -> AppleContainerProvider {
        AppleContainerProvider::new(ContainerCli::default())
    }

    #[test]
    fn provider_name() {
        assert_eq!(provider().name(), "apple-container");
    }

    #[test]
    fn load_returns_empty_initially() {
        assert!(provider().load().unwrap().is_empty());
    }

    #[test]
    fn load_returns_state_after_update() {
        let provider = provider();

        // Simulate the watcher refreshing state
        {
            let name = DomainName::new("web.roxy").unwrap();
            let pattern = DomainPattern::Exact(name);
            let routes = vec![Route::parse("/=192.168.64.2:8080").unwrap()];
            let state = provider.state();
            let mut guard = state.write().unwrap();
            guard.push(DomainRegistration::new(pattern, routes));
        }

        let loaded = provider.load().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].domain().as_str(), "web.roxy");
    }
}
