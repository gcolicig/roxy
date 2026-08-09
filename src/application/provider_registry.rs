use std::collections::HashMap;
use std::sync::Arc;

use tracing::warn;

use super::ports::RegistrationProvider;
use crate::domain::DomainRegistration;

/// Aggregates registrations from multiple providers.
///
/// When `load()` is called, each provider is queried and results are
/// concatenated. Individual provider errors are logged but don't fail
/// the merge — other providers still contribute their registrations.
pub struct ProviderRegistry {
    providers: Vec<Arc<dyn RegistrationProvider>>,
}

impl Default for ProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ProviderRegistry {
    pub fn new() -> Self {
        Self {
            providers: Vec::new(),
        }
    }

    pub fn add(&mut self, provider: Arc<dyn RegistrationProvider>) {
        self.providers.push(provider);
    }
}

impl RegistrationProvider for ProviderRegistry {
    fn name(&self) -> &str {
        "aggregator"
    }

    fn load(&self) -> anyhow::Result<Vec<DomainRegistration>> {
        let mut all = Vec::new();
        // Which provider first claimed each pattern, for collision reporting.
        let mut claimed: HashMap<String, &str> = HashMap::new();

        for provider in &self.providers {
            match provider.load() {
                Ok(regs) => {
                    for reg in regs {
                        let pattern = reg.display_pattern();
                        match claimed.get(pattern.as_str()) {
                            Some(winner) => warn!(
                                domain = %pattern,
                                serving = winner,
                                shadowed = provider.name(),
                                "Duplicate domain from multiple providers; \
                                 the earlier provider keeps serving it"
                            ),
                            None => {
                                claimed.insert(pattern, provider.name());
                            }
                        }
                        all.push(reg);
                    }
                }
                Err(e) => {
                    warn!(
                        provider = provider.name(),
                        error = %e,
                        "Provider failed to load registrations, skipping"
                    );
                }
            }
        }

        Ok(all)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FixedProvider {
        name: &'static str,
        registrations: Vec<DomainRegistration>,
    }

    impl RegistrationProvider for FixedProvider {
        fn name(&self) -> &str {
            self.name
        }
        fn load(&self) -> anyhow::Result<Vec<DomainRegistration>> {
            Ok(self.registrations.clone())
        }
    }

    struct FailingProvider;

    impl RegistrationProvider for FailingProvider {
        fn name(&self) -> &str {
            "failing"
        }
        fn load(&self) -> anyhow::Result<Vec<DomainRegistration>> {
            Err(anyhow::anyhow!("provider error"))
        }
    }

    fn test_reg(domain: &str) -> DomainRegistration {
        use crate::domain::{DomainName, DomainPattern, Route};
        let name = DomainName::new(domain).unwrap();
        let pattern = DomainPattern::Exact(name);
        let routes = vec![Route::parse("/=3000").unwrap()];
        DomainRegistration::new(pattern, routes)
    }

    #[test]
    fn duplicate_patterns_are_kept_in_provider_order() {
        // The router takes the first match, so the earlier provider must
        // stay ahead of the later one even when both claim the domain.
        let mut registry = ProviderRegistry::new();
        registry.add(Arc::new(FixedProvider {
            name: "config-file",
            registrations: vec![test_reg("app.roxy")],
        }));
        registry.add(Arc::new(FixedProvider {
            name: "apple-container",
            registrations: vec![test_reg("app.roxy")],
        }));

        let loaded = registry.load().unwrap();
        assert_eq!(loaded.len(), 2, "duplicates are reported, not dropped");
        assert!(loaded.iter().all(|r| r.display_pattern() == "app.roxy"));
    }

    #[test]
    fn distinct_patterns_from_several_providers_all_load() {
        let mut registry = ProviderRegistry::new();
        registry.add(Arc::new(FixedProvider {
            name: "config-file",
            registrations: vec![test_reg("app.roxy")],
        }));
        registry.add(Arc::new(FixedProvider {
            name: "apple-container",
            registrations: vec![test_reg("web.roxy")],
        }));

        let patterns: Vec<String> = registry
            .load()
            .unwrap()
            .iter()
            .map(|r| r.display_pattern())
            .collect();
        assert_eq!(patterns, vec!["app.roxy", "web.roxy"]);
    }

    #[test]
    fn empty_registry_returns_empty() {
        let registry = ProviderRegistry::new();
        let result = registry.load().unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn single_provider() {
        let mut registry = ProviderRegistry::new();
        registry.add(Arc::new(FixedProvider {
            name: "test",
            registrations: vec![test_reg("app.roxy")],
        }));

        let result = registry.load().unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].domain().as_str(), "app.roxy");
    }

    #[test]
    fn merges_multiple_providers() {
        let mut registry = ProviderRegistry::new();
        registry.add(Arc::new(FixedProvider {
            name: "config",
            registrations: vec![test_reg("app.roxy")],
        }));
        registry.add(Arc::new(FixedProvider {
            name: "docker",
            registrations: vec![test_reg("web.myproject.roxy")],
        }));

        let result = registry.load().unwrap();
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn failing_provider_does_not_block_others() {
        let mut registry = ProviderRegistry::new();
        registry.add(Arc::new(FixedProvider {
            name: "config",
            registrations: vec![test_reg("app.roxy")],
        }));
        registry.add(Arc::new(FailingProvider));

        let result = registry.load().unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].domain().as_str(), "app.roxy");
    }

    #[test]
    fn all_providers_failing_returns_empty() {
        let mut registry = ProviderRegistry::new();
        registry.add(Arc::new(FailingProvider));

        let result = registry.load().unwrap();
        assert!(result.is_empty());
    }
}
