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

    /// Loop-level tests. The review of this branch found two defects
    /// (unbounded CLI calls, cancellation not raced against reconcile)
    /// that the pure-helper tests above could not catch, so the loop
    /// itself is exercised here against a scripted stand-in CLI.
    #[cfg(unix)]
    mod loop_tests {
        use std::os::unix::fs::PermissionsExt;
        use std::path::{Path, PathBuf};

        use super::*;

        const RUNNING_WEB: &str = r#"[{"id":"web",
            "configuration":{"labels":{"roxy.enable":"true"},
                "publishedPorts":[{"containerPort":8080,"hostPort":18080,"proto":"tcp"}]},
            "status":{"state":"running","networks":[{"ipv4Address":"192.168.64.2/24"}]}}]"#;

        const RUNNING_WEB_MOVED: &str = r#"[{"id":"web",
            "configuration":{"labels":{"roxy.enable":"true"},
                "publishedPorts":[{"containerPort":8080,"hostPort":18080,"proto":"tcp"}]},
            "status":{"state":"running","networks":[{"ipv4Address":"192.168.65.9/24"}]}}]"#;

        /// A scripted CLI: cats a sibling data file, ignoring its args.
        /// Swapping the file's content between polls simulates container
        /// churn without a real Apple Container installation.
        struct FakeCli {
            dir: PathBuf,
        }

        impl FakeCli {
            fn new(name: &str, initial_json: &str) -> Self {
                let dir = std::env::temp_dir().join(format!("roxy-watcher-test-{name}"));
                std::fs::create_dir_all(&dir).unwrap();
                let script = dir.join("container");
                std::fs::write(
                    &script,
                    format!("#!/bin/sh\ncat {}\n", dir.join("output.json").display()),
                )
                .unwrap();
                std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
                let fake = Self { dir };
                fake.set_output(initial_json);
                fake
            }

            fn set_output(&self, json: &str) {
                std::fs::write(self.dir.join("output.json"), json).unwrap();
            }

            fn script(&self) -> String {
                self.dir.join("container").to_string_lossy().into_owned()
            }
        }

        impl Drop for FakeCli {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.dir);
            }
        }

        fn targets(state: &Arc<RwLock<Vec<DomainRegistration>>>) -> Vec<String> {
            fingerprint(&state.read().unwrap())
        }

        async fn recv_nudge(rx: &mut mpsc::Receiver<()>) -> bool {
            tokio::time::timeout(Duration::from_secs(5), rx.recv())
                .await
                .is_ok_and(|msg| msg.is_some())
        }

        fn spawn_watch(
            script: &str,
            state: &Arc<RwLock<Vec<DomainRegistration>>>,
            cancel: &CancellationToken,
        ) -> (tokio::task::JoinHandle<()>, mpsc::Receiver<()>) {
            let (tx, rx) = mpsc::channel(4);
            let handle = tokio::spawn(watch(
                ContainerCli::new(script),
                state.clone(),
                tx,
                cancel.clone(),
                Duration::from_millis(50),
            ));
            (handle, rx)
        }

        #[tokio::test]
        async fn discovers_on_first_poll_and_nudges_once() {
            let fake = FakeCli::new("discover", RUNNING_WEB);
            let state = Arc::new(RwLock::new(Vec::new()));
            let cancel = CancellationToken::new();
            let (handle, mut rx) = spawn_watch(&fake.script(), &state, &cancel);

            assert!(recv_nudge(&mut rx).await, "first poll should nudge");
            assert_eq!(targets(&state), vec!["web.roxy => /=192.168.64.2:8080"]);

            // Several unchanged polls later there must be no second nudge.
            tokio::time::sleep(Duration::from_millis(300)).await;
            assert!(
                rx.try_recv().is_err(),
                "unchanged listings must not renudge the server"
            );

            cancel.cancel();
            handle.await.unwrap();
        }

        #[tokio::test]
        async fn ip_change_is_picked_up_and_nudges_again() {
            let fake = FakeCli::new("ip-change", RUNNING_WEB);
            let state = Arc::new(RwLock::new(Vec::new()));
            let cancel = CancellationToken::new();
            let (handle, mut rx) = spawn_watch(&fake.script(), &state, &cancel);

            assert!(recv_nudge(&mut rx).await);
            fake.set_output(RUNNING_WEB_MOVED);

            assert!(recv_nudge(&mut rx).await, "moved IP should renudge");
            assert_eq!(targets(&state), vec!["web.roxy => /=192.168.65.9:8080"]);

            cancel.cancel();
            handle.await.unwrap();
        }

        #[tokio::test]
        async fn failing_cli_keeps_last_good_state() {
            let fake = FakeCli::new("fail", RUNNING_WEB);
            let state = Arc::new(RwLock::new(Vec::new()));
            let cancel = CancellationToken::new();
            let (handle, mut rx) = spawn_watch(&fake.script(), &state, &cancel);

            assert!(recv_nudge(&mut rx).await);
            let before = targets(&state);

            // Deleting the data file makes every subsequent poll fail.
            std::fs::remove_file(Path::new(&fake.dir.join("output.json"))).unwrap();
            tokio::time::sleep(Duration::from_millis(300)).await;

            assert_eq!(
                targets(&state),
                before,
                "a failed poll must not wipe the registrations"
            );
            assert!(rx.try_recv().is_err(), "failures must not nudge");

            cancel.cancel();
            handle.await.unwrap();
        }

        #[tokio::test]
        async fn cancellation_ends_the_loop() {
            let fake = FakeCli::new("cancel", RUNNING_WEB);
            let state = Arc::new(RwLock::new(Vec::new()));
            let cancel = CancellationToken::new();
            let (handle, _rx) = spawn_watch(&fake.script(), &state, &cancel);

            cancel.cancel();
            tokio::time::timeout(Duration::from_secs(5), handle)
                .await
                .expect("watch must return promptly after cancellation")
                .unwrap();
        }
    }
}
