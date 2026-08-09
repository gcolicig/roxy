use std::sync::{Arc, RwLock};
use std::time::Duration;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use super::cli::{Container, ContainerCli};
use super::discovery::{DiscoveryResult, evaluate_container};
use crate::domain::DomainRegistration;

/// Poll Apple Container and keep the shared registration list current.
///
/// Apple Container exposes no event stream, so this polls instead of
/// subscribing. Each tick is a full reconciliation; the server is only
/// nudged when the resulting registrations actually differ.
pub async fn watch(
    cli: ContainerCli,
    state: Arc<RwLock<Vec<DomainRegistration>>>,
    nudge_tx: mpsc::Sender<()>,
    cancel: CancellationToken,
    interval: Duration,
) {
    info!(
        interval_secs = interval.as_secs(),
        "Apple Container watcher started"
    );

    loop {
        // Race the reconcile against cancellation, not just the sleep — a
        // slow CLI call must not hold up shutdown for its whole duration.
        tokio::select! {
            _ = cancel.cancelled() => {
                info!("Apple Container watcher shutting down");
                return;
            }
            result = reconcile(&cli, &state, &nudge_tx) => {
                if let Err(e) = result {
                    // A transient CLI failure must not take the daemon down;
                    // the next tick retries with the last good state intact.
                    warn!(error = %e, "Apple Container reconciliation failed");
                }
            }
        }

        tokio::select! {
            _ = cancel.cancelled() => {
                info!("Apple Container watcher shutting down");
                return;
            }
            _ = tokio::time::sleep(interval) => {}
        }
    }
}

/// List containers, evaluate them, and swap in the result if it changed.
async fn reconcile(
    cli: &ContainerCli,
    state: &Arc<RwLock<Vec<DomainRegistration>>>,
    nudge_tx: &mpsc::Sender<()>,
) -> anyhow::Result<()> {
    let containers = cli.list_all().await?;

    let registrations = registrations_from(&containers);
    let next = fingerprint(&registrations);

    let current = {
        let guard = state
            .read()
            .map_err(|e| anyhow::anyhow!("Apple Container state lock poisoned: {e}"))?;
        fingerprint(&guard)
    };

    if next == current {
        return Ok(());
    }

    log_changes(&current, &next);

    {
        let mut guard = state
            .write()
            .map_err(|e| anyhow::anyhow!("Apple Container state lock poisoned: {e}"))?;
        *guard = registrations;
    }

    let _ = nudge_tx.send(()).await;
    Ok(())
}

/// Evaluate every container, keeping the ones that qualify.
fn registrations_from(containers: &[Container]) -> Vec<DomainRegistration> {
    let mut registrations = Vec::new();

    for container in containers {
        match evaluate_container(container) {
            DiscoveryResult::Register(reg) => {
                debug!(
                    domain = %reg.display_pattern(),
                    container = %container.id,
                    "Apple Container registered"
                );
                registrations.push(reg);
            }
            DiscoveryResult::Skip(reason) => {
                debug!(container = %container.id, reason, "Apple Container skipped");
            }
        }
    }

    registrations
}

/// Order-independent summary of a registration set, used to detect change.
///
/// Deliberately includes the route targets, not just the domain: Apple
/// Container hands out a new IP when a container restarts, so a set with
/// identical domain names can still need a reload.
fn fingerprint(registrations: &[DomainRegistration]) -> Vec<String> {
    let mut entries: Vec<String> = registrations
        .iter()
        .map(|reg| {
            let targets: Vec<String> = reg
                .routes()
                .iter()
                .map(|route| format!("{}={}", route.path(), route.target()))
                .collect();
            format!("{} => {}", reg.display_pattern(), targets.join(","))
        })
        .collect();
    entries.sort();
    entries
}

fn log_changes(current: &[String], next: &[String]) {
    for entry in next.iter().filter(|e| !current.contains(e)) {
        info!(registration = %entry, "Apple Container registration added");
    }
    for entry in current.iter().filter(|e| !next.contains(e)) {
        info!(registration = %entry, "Apple Container registration removed");
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::net::Ipv4Addr;

    use super::*;
    use crate::infrastructure::apple_container::cli::{ContainerState, PublishedPort};

    fn container(id: &str, ip: [u8; 4]) -> Container {
        Container {
            id: id.to_string(),
            labels: HashMap::from([("roxy.enable".to_string(), "true".to_string())]),
            state: ContainerState::Running,
            ipv4: Some(Ipv4Addr::from(ip)),
            published_ports: vec![PublishedPort {
                container_port: 8080,
                host_port: 18080,
                proto: "tcp".to_string(),
            }],
        }
    }

    #[test]
    fn only_qualifying_containers_are_registered() {
        let mut skipped = container("skipped", [192, 168, 64, 3]);
        skipped.labels.clear();

        let registrations = registrations_from(&[container("web", [192, 168, 64, 2]), skipped]);

        assert_eq!(registrations.len(), 1);
        assert_eq!(registrations[0].domain().as_str(), "web.roxy");
    }

    #[test]
    fn fingerprint_ignores_ordering() {
        let a = registrations_from(&[
            container("web", [192, 168, 64, 2]),
            container("api", [192, 168, 64, 3]),
        ]);
        let b = registrations_from(&[
            container("api", [192, 168, 64, 3]),
            container("web", [192, 168, 64, 2]),
        ]);

        assert_eq!(fingerprint(&a), fingerprint(&b));
    }

    #[test]
    fn fingerprint_changes_when_ip_changes() {
        let before = registrations_from(&[container("web", [192, 168, 64, 2])]);
        let after = registrations_from(&[container("web", [192, 168, 65, 9])]);

        assert_ne!(
            fingerprint(&before),
            fingerprint(&after),
            "a restarted container keeps its name but moves address; \
             comparing names alone would miss the change"
        );
    }

    #[test]
    fn fingerprint_changes_when_container_disappears() {
        let before = registrations_from(&[
            container("web", [192, 168, 64, 2]),
            container("api", [192, 168, 64, 3]),
        ]);
        let after = registrations_from(&[container("web", [192, 168, 64, 2])]);

        assert_ne!(fingerprint(&before), fingerprint(&after));
    }

    #[test]
    fn fingerprint_is_stable_for_an_unchanged_set() {
        let before = registrations_from(&[container("web", [192, 168, 64, 2])]);
        let after = registrations_from(&[container("web", [192, 168, 64, 2])]);

        assert_eq!(fingerprint(&before), fingerprint(&after));
    }

    #[test]
    fn empty_listing_produces_no_registrations() {
        assert!(registrations_from(&[]).is_empty());
        assert!(fingerprint(&[]).is_empty());
    }
}
