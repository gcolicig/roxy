use tracing::warn;

use super::cli::Container;
use crate::domain::value_objects::port::Port;
use crate::domain::{
    DomainName, DomainPattern, DomainRegistration, PathPrefix, ProxyTarget, RegistrationSource,
    Route, RouteTarget,
};

/// Result of evaluating a container for registration.
#[derive(Debug)]
pub enum DiscoveryResult {
    /// Container qualifies; here's its registration.
    Register(DomainRegistration),
    /// Container was opted out, is not running, or lacks routing information.
    Skip(String),
}

/// Evaluate a container and decide whether to register it.
///
/// Qualification rules:
/// 1. Not running --> skip (no IP to route to)
/// 2. `roxy.enable=false` --> skip (explicit opt-out)
/// 3. `roxy.enable=true` or a `roxy.domain` label --> register
/// 4. Otherwise --> skip
///
/// Unlike Docker there is no Compose metadata to infer intent from, so
/// registration is always opt-in via labels.
///
/// The proxy target is the container's own IP plus its container port.
/// Apple Container gives every container a routable address, so there is no
/// reason to detour through a published host port the way the Docker
/// provider must.
pub fn evaluate_container(container: &Container) -> DiscoveryResult {
    if !container.state.is_running() {
        return DiscoveryResult::Skip(format!(
            "container {} is {:?}, not running",
            container.id, container.state
        ));
    }

    if container.label("roxy.enable") == Some("false") {
        return DiscoveryResult::Skip("explicitly disabled via roxy.enable=false".into());
    }

    let opted_in = container.label("roxy.enable") == Some("true");
    if !opted_in && container.label("roxy.domain").is_none() {
        return DiscoveryResult::Skip(format!(
            "container {} has neither roxy.enable=true nor roxy.domain",
            container.id
        ));
    }

    let Some(domain) = resolve_domain(container) else {
        return DiscoveryResult::Skip(format!(
            "container {} has no valid domain (set roxy.domain)",
            container.id
        ));
    };

    let Some(ip) = container.ipv4 else {
        return DiscoveryResult::Skip(format!(
            "container {} is running but has no IPv4 address",
            container.id
        ));
    };

    let port = match resolve_port(container) {
        Ok(port) => port,
        Err(reason) => return DiscoveryResult::Skip(reason),
    };

    let target = ProxyTarget::new(ip.to_string(), port);

    let root_path = match PathPrefix::new("/") {
        Ok(path) => path,
        Err(e) => return DiscoveryResult::Skip(format!("bug: root path is invalid: {e}")),
    };
    let route = Route::new(root_path, RouteTarget::Proxy(target));

    let pattern = if container.label("roxy.wildcard") == Some("true") {
        DomainPattern::Wildcard(domain)
    } else {
        DomainPattern::Exact(domain)
    };

    let mut registration =
        DomainRegistration::with_source(pattern, vec![route], RegistrationSource::External);
    registration.enable_https();

    DiscoveryResult::Register(registration)
}

/// Resolve the domain name for a container.
///
/// Priority:
/// 1. `roxy.domain` label (explicit override)
/// 2. `{container-id}.roxy` — Apple Container ids are unique per host and
///    default to the `--name` given at start
fn resolve_domain(container: &Container) -> Option<DomainName> {
    if let Some(label) = container.label("roxy.domain") {
        return match DomainName::new(label) {
            Ok(domain) => Some(domain),
            Err(e) => {
                warn!(
                    container = %container.id,
                    domain = %label,
                    error = %e,
                    "Invalid roxy.domain label"
                );
                None
            }
        };
    }

    let derived = format!("{}.roxy", container.id);
    match DomainName::new(&derived) {
        Ok(domain) => Some(domain),
        Err(e) => {
            warn!(
                container = %container.id,
                domain = %derived,
                error = %e,
                "Could not derive a valid domain from the container id"
            );
            None
        }
    }
}

/// Resolve the container port to proxy to.
///
/// Priority:
/// 1. `roxy.port` label (explicit)
/// 2. Exactly one published port --> its container port
/// 3. Error — nothing published, or ambiguous without a label
///
/// Note this uses `containerPort`, not `hostPort`: the target is the
/// container's own address, so the host-side mapping is irrelevant even
/// when one exists.
fn resolve_port(container: &Container) -> Result<Port, String> {
    if let Some(label) = container.label("roxy.port") {
        let parsed = label
            .parse::<u16>()
            .map_err(|_| format!("container {} has invalid roxy.port '{label}'", container.id))?;
        return Port::any(parsed)
            .map_err(|e| format!("container {} has invalid roxy.port: {e}", container.id));
    }

    match container.published_ports.as_slice() {
        [single] => Port::any(single.container_port).map_err(|e| {
            format!(
                "container {} publishes invalid port {}: {e}",
                container.id, single.container_port
            )
        }),
        [] => Err(format!(
            "container {} publishes no ports; set roxy.port to choose one",
            container.id
        )),
        many => Err(format!(
            "container {} publishes {} ports ({:?}); set roxy.port to choose one",
            container.id,
            many.len(),
            many.iter().map(|p| p.container_port).collect::<Vec<_>>()
        )),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::net::Ipv4Addr;

    use super::*;
    use crate::infrastructure::apple_container::cli::{ContainerState, PublishedPort};

    /// A running container with one published port and no labels.
    fn running(id: &str) -> Container {
        Container {
            id: id.to_string(),
            labels: HashMap::new(),
            state: ContainerState::Running,
            ipv4: Some(Ipv4Addr::new(192, 168, 64, 2)),
            published_ports: vec![PublishedPort {
                container_port: 8080,
                host_port: 18080,
                proto: "tcp".to_string(),
            }],
        }
    }

    fn with_labels(mut container: Container, labels: &[(&str, &str)]) -> Container {
        container.labels = labels
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        container
    }

    fn expect_registered(result: DiscoveryResult) -> DomainRegistration {
        match result {
            DiscoveryResult::Register(reg) => reg,
            DiscoveryResult::Skip(reason) => panic!("expected registration, got skip: {reason}"),
        }
    }

    fn expect_skip(result: DiscoveryResult) -> String {
        match result {
            DiscoveryResult::Skip(reason) => reason,
            DiscoveryResult::Register(reg) => {
                panic!("expected skip, got registration for {}", reg.domain())
            }
        }
    }

    #[test]
    fn opt_in_container_targets_its_own_ip_and_container_port() {
        let container = with_labels(running("web"), &[("roxy.enable", "true")]);
        let reg = expect_registered(evaluate_container(&container));

        assert_eq!(reg.domain().as_str(), "web.roxy");
        assert_eq!(reg.routes().len(), 1);
        assert_eq!(
            reg.routes()[0].target().to_string(),
            "192.168.64.2:8080",
            "must use the container IP and containerPort, not the host mapping"
        );
    }

    #[test]
    fn registrations_are_external_and_https_enabled() {
        let container = with_labels(running("web"), &[("roxy.enable", "true")]);
        let reg = expect_registered(evaluate_container(&container));

        assert_eq!(reg.source(), RegistrationSource::External);
        assert!(reg.is_https_enabled());
    }

    #[test]
    fn roxy_domain_label_alone_is_enough_to_opt_in() {
        let container = with_labels(running("web"), &[("roxy.domain", "shop.roxy")]);
        let reg = expect_registered(evaluate_container(&container));
        assert_eq!(reg.domain().as_str(), "shop.roxy");
    }

    #[test]
    fn roxy_domain_overrides_the_derived_name() {
        let container = with_labels(
            running("web"),
            &[("roxy.enable", "true"), ("roxy.domain", "shop.roxy")],
        );
        let reg = expect_registered(evaluate_container(&container));
        assert_eq!(reg.domain().as_str(), "shop.roxy");
    }

    #[test]
    fn roxy_port_label_wins_over_published_port() {
        let container = with_labels(
            running("web"),
            &[("roxy.enable", "true"), ("roxy.port", "3000")],
        );
        let reg = expect_registered(evaluate_container(&container));
        assert_eq!(reg.routes()[0].target().to_string(), "192.168.64.2:3000");
    }

    #[test]
    fn roxy_port_allows_privileged_ports() {
        let container = with_labels(
            running("web"),
            &[("roxy.enable", "true"), ("roxy.port", "80")],
        );
        let reg = expect_registered(evaluate_container(&container));
        assert_eq!(reg.routes()[0].target().to_string(), "192.168.64.2:80");
    }

    #[test]
    fn wildcard_label_produces_a_wildcard_pattern() {
        let container = with_labels(
            running("web"),
            &[("roxy.enable", "true"), ("roxy.wildcard", "true")],
        );
        let reg = expect_registered(evaluate_container(&container));
        assert!(reg.is_wildcard());
    }

    #[test]
    fn opt_out_wins_over_domain_label() {
        let container = with_labels(
            running("web"),
            &[("roxy.enable", "false"), ("roxy.domain", "shop.roxy")],
        );
        assert!(expect_skip(evaluate_container(&container)).contains("roxy.enable=false"));
    }

    #[test]
    fn unlabelled_container_is_skipped() {
        let reason = expect_skip(evaluate_container(&running("web")));
        assert!(reason.contains("roxy.enable"));
    }

    #[test]
    fn stopped_container_is_skipped() {
        let mut container = with_labels(running("web"), &[("roxy.enable", "true")]);
        container.state = ContainerState::Stopped;
        container.ipv4 = None;
        assert!(expect_skip(evaluate_container(&container)).contains("not running"));
    }

    #[test]
    fn running_container_without_ipv4_is_skipped() {
        let mut container = with_labels(running("web"), &[("roxy.enable", "true")]);
        container.ipv4 = None;
        assert!(expect_skip(evaluate_container(&container)).contains("no IPv4"));
    }

    #[test]
    fn container_without_published_ports_is_skipped() {
        let mut container = with_labels(running("web"), &[("roxy.enable", "true")]);
        container.published_ports.clear();
        assert!(expect_skip(evaluate_container(&container)).contains("roxy.port"));
    }

    #[test]
    fn multiple_published_ports_without_label_are_ambiguous() {
        let mut container = with_labels(running("web"), &[("roxy.enable", "true")]);
        container.published_ports.push(PublishedPort {
            container_port: 9090,
            host_port: 19090,
            proto: "tcp".to_string(),
        });

        let reason = expect_skip(evaluate_container(&container));
        assert!(reason.contains("8080"), "reason should list the candidates");
        assert!(reason.contains("9090"));
        assert!(reason.contains("roxy.port"));
    }

    #[test]
    fn invalid_roxy_port_is_skipped() {
        let container = with_labels(
            running("web"),
            &[("roxy.enable", "true"), ("roxy.port", "not-a-port")],
        );
        assert!(expect_skip(evaluate_container(&container)).contains("invalid roxy.port"));
    }

    #[test]
    fn invalid_roxy_domain_is_skipped() {
        let container = with_labels(
            running("web"),
            &[("roxy.enable", "true"), ("roxy.domain", "shop.example.com")],
        );
        assert!(expect_skip(evaluate_container(&container)).contains("no valid domain"));
    }

    #[test]
    fn container_id_that_cannot_form_a_domain_is_skipped() {
        let container = with_labels(running("web_underscore"), &[("roxy.enable", "true")]);
        assert!(expect_skip(evaluate_container(&container)).contains("no valid domain"));
    }
}
