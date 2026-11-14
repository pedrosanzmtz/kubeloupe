//! Table and text output for the `kubeloupe` CLI.

use crate::{
    exec_command, format_cpu, format_memory, get_namespaces, get_node_metrics, get_nodes,
    get_pod_logs, get_pod_metrics, get_pods, resources, stream_pod_logs, watch, ForwardStatus,
    ForwardTarget, LogOptions, PortForward, PortSpec,
};
use anyhow::{Context, Result};
use kube::Client;

/// List pods, optionally scoped to one namespace (CLI output).
pub async fn list_pods(client: &Client, namespace: Option<&str>) -> Result<()> {
    let pods = get_pods(client, namespace).await?;
    println!(
        "{:<50} {:<20} {:<12} {:<10} {}",
        "NAME", "NAMESPACE", "PHASE", "NODE", "AGE"
    );
    for pod in pods {
        println!(
            "{:<50} {:<20} {:<12} {:<10} {}",
            pod.name, pod.namespace, pod.phase, pod.node, pod.age
        );
    }
    Ok(())
}

/// List all namespaces in the cluster (CLI output).
pub async fn list_namespaces(client: &Client) -> Result<()> {
    let namespaces = get_namespaces(client).await?;
    println!("{:<30} {:<10} {}", "NAME", "STATUS", "AGE");
    for ns in namespaces {
        println!("{:<30} {:<10} {}", ns.name, ns.status, ns.age);
    }
    Ok(())
}

/// List all nodes in the cluster (CLI output).
pub async fn list_nodes(client: &Client) -> Result<()> {
    let nodes = get_nodes(client).await?;
    println!("{:<40} {:<12} {:<12} {}", "NAME", "READY", "VERSION", "AGE");
    for node in nodes {
        println!(
            "{:<40} {:<12} {:<12} {}",
            node.name, node.ready, node.version, node.age
        );
    }
    Ok(())
}

/// Print a pod's logs (CLI output). With `follow`, keeps streaming until
/// interrupted.
pub async fn print_pod_logs(
    client: &Client,
    namespace: Option<&str>,
    name: &str,
    opts: &LogOptions,
    follow: bool,
) -> Result<()> {
    let namespace = namespace.unwrap_or("default");
    if follow {
        anyhow::ensure!(
            !opts.previous,
            "--follow cannot be combined with --previous"
        );
        stream_pod_logs(client, namespace, name, opts, |line| println!("{line}")).await
    } else {
        let logs = get_pod_logs(client, namespace, name, opts).await?;
        print!("{logs}");
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// CLI output for the new commands.
// ---------------------------------------------------------------------------

/// Print `kubectl top pods`-style usage (CLI output), optionally for one
/// namespace. The namespace is queried directly (not filtered from a
/// cluster-wide list), so it works for users limited to that namespace.
pub async fn print_top_pods(client: &Client, namespace: Option<&str>) -> Result<()> {
    match get_pod_metrics(client, namespace).await? {
        None => {
            anyhow::bail!(
                "Metrics API not available on this cluster (is metrics-server installed?)"
            )
        }
        Some(usages) => {
            println!(
                "{:<50} {:<20} {:<10} {}",
                "NAME", "NAMESPACE", "CPU", "MEMORY"
            );
            for u in usages {
                println!(
                    "{:<50} {:<20} {:<10} {}",
                    u.name,
                    u.namespace,
                    format_cpu(u.cpu_millicores),
                    format_memory(u.memory_bytes)
                );
            }
        }
    }
    Ok(())
}

/// Print `kubectl top nodes`-style usage (CLI output).
pub async fn print_top_nodes(client: &Client) -> Result<()> {
    match get_node_metrics(client).await? {
        None => {
            anyhow::bail!(
                "Metrics API not available on this cluster (is metrics-server installed?)"
            )
        }
        Some(usages) => {
            println!("{:<40} {:<16} {}", "NAME", "CPU", "MEMORY");
            for u in usages {
                let cpu = if u.cpu_capacity_millicores > 0 {
                    format!(
                        "{}/{}",
                        format_cpu(u.cpu_millicores),
                        format_cpu(u.cpu_capacity_millicores)
                    )
                } else {
                    format_cpu(u.cpu_millicores)
                };
                let mem = if u.memory_capacity_bytes > 0 {
                    format!(
                        "{}/{}",
                        format_memory(u.memory_bytes),
                        format_memory(u.memory_capacity_bytes)
                    )
                } else {
                    format_memory(u.memory_bytes)
                };
                println!("{:<40} {:<16} {}", u.name, cpu, mem);
            }
        }
    }
    Ok(())
}

/// Forward ports of a pod, Service or workload to localhost until Ctrl-C
/// (CLI output). Prints which pod serves each forward when it changes, and
/// errors as they happen.
pub async fn run_port_forward(
    client: &Client,
    namespace: Option<&str>,
    target: &str,
    ports: &[String],
) -> Result<()> {
    let ns = namespace.unwrap_or(client.default_namespace());
    let target = ForwardTarget::parse(target, ns)?;
    let specs = ports
        .iter()
        .map(|p| PortSpec::parse(p))
        .collect::<Result<Vec<_>>>()?;
    let mut forwards = Vec::new();
    for spec in specs {
        let f = PortForward::start(client, target.clone(), spec.remote, spec.local).await?;
        println!(
            "Forwarding from 127.0.0.1:{} -> {}:{}",
            f.local_port,
            target_label(&target),
            f.remote
        );
        forwards.push(f);
    }
    println!("Ctrl-C to stop.");
    let mut last: Vec<ForwardStatus> = forwards.iter().map(|f| f.status()).collect();
    for (f, s) in forwards.iter().zip(&last) {
        println!("  127.0.0.1:{} served by pod {}", f.local_port, s.pod);
    }
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(500));
    loop {
        tokio::select! {
            r = tokio::signal::ctrl_c() => { r?; break; }
            _ = tick.tick() => {
                for (f, prev) in forwards.iter().zip(last.iter_mut()) {
                    let now = f.status();
                    if now.pod != prev.pod {
                        println!("  127.0.0.1:{} now served by pod {}", f.local_port, now.pod);
                    }
                    if now.last_error != prev.last_error && !now.last_error.is_empty() {
                        eprintln!("  127.0.0.1:{}: {}", f.local_port, now.last_error);
                    }
                    *prev = now;
                }
            }
        }
    }
    drop(forwards);
    println!("Port-forward stopped.");
    Ok(())
}

/// `svc/api` (the namespace is implied by `-n`).
fn target_label(t: &ForwardTarget) -> String {
    format!("{}/{}", t.kind.short(), t.name)
}

/// Run a command in a pod and print its output (CLI output).
pub async fn exec_in_pod(
    client: &Client,
    namespace: Option<&str>,
    name: &str,
    container: Option<&str>,
    command: &[String],
) -> Result<()> {
    let result = exec_command(
        client,
        namespace.unwrap_or("default"),
        name,
        container,
        command,
    )
    .await?;
    print!("{}", result.stdout);
    eprint!("{}", result.stderr);
    if !result.success {
        anyhow::bail!("command exited with a non-zero status");
    }
    Ok(())
}

/// Print live changes to any resource type until interrupted.
pub async fn watch_and_print(
    client: &Client,
    resource: &str,
    namespace: Option<&str>,
) -> Result<()> {
    use watch::{run_batched, Output, DEFAULT_FLUSH};
    fn print<T: std::fmt::Debug>(out: Output<T>) {
        match out {
            Output::State(s) => println!("state: {s:?}"),
            Output::Batch(b) => {
                println!(
                    "{}: {} applied, {} deleted",
                    if b.snapshot { "snapshot" } else { "batch" },
                    b.applied.len(),
                    b.deleted.len()
                );
                for item in &b.applied {
                    println!("  + {item:?}");
                }
                for key in &b.deleted {
                    println!("  - {key}");
                }
            }
        }
    }
    let kinds = resources::discover(client).await?;
    let kind = resources::resolve_kind(&kinds, resource)
        .with_context(|| format!("unknown resource type: {resource}"))?;
    let (_store, updates) = watch::watch_resource(client, kind, namespace);
    run_batched(updates, DEFAULT_FLUSH, print).await;
    Ok(())
}

/// One line for a CLI error: the context chain without repeats, and the API
/// server's own message instead of kube's debug dump of an API error.
pub fn error_text(err: &anyhow::Error) -> String {
    let mut parts: Vec<String> = Vec::new();
    for cause in err.chain() {
        let api = match cause.downcast_ref::<kube::Error>() {
            Some(kube::Error::Api(r)) => Some(r),
            _ => None,
        };
        let text = match api {
            Some(r) if !r.message.is_empty() => r.message.clone(),
            Some(r) => format!("{} ({})", r.reason, r.code),
            None => cause.to_string(),
        };
        if !parts.iter().any(|p| p.contains(&text)) {
            parts.push(text);
        }
        if api.is_some() {
            break;
        }
    }
    parts.join(": ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use kube::core::ErrorResponse;

    #[test]
    fn error_text_uses_the_api_message_and_drops_repeats() {
        let api: anyhow::Error = kube::Error::Api(ErrorResponse {
            status: "Failure".into(),
            message: "pods \"x\" not found".into(),
            reason: "NotFound".into(),
            code: 404,
        })
        .into();
        assert_eq!(
            error_text(&api.context("describe pod/x")),
            "describe pod/x: pods \"x\" not found"
        );
        let io = anyhow::Error::from(std::io::Error::other("no such file"))
            .context("failed to read kubeconfig: no such file")
            .context("cannot read kubeconfig /k");
        assert_eq!(
            error_text(&io),
            "cannot read kubeconfig /k: failed to read kubeconfig: no such file"
        );
    }
}
