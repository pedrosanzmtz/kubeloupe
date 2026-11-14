mod args;

use anyhow::Result;
use args::{Cli, Commands};
use clap::{CommandFactory, Parser};
use kubeloupe::actions::{cli_action, cli_delete, cli_drain, ActionId, DrainOptions};
use kubeloupe::{
    client_with, error_text, exec_in_pod, list_namespaces, list_nodes, list_pods, parse_log_target,
    print_describe, print_get, print_pod_logs, print_top_nodes, print_top_pods,
    print_workload_logs, print_yaml, run_port_forward, split_target, watch_and_print, GetOptions,
    LogOptions, LogTarget,
};
use std::io::IsTerminal;
use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    reset_sigpipe();
    // Diagnostics go to stderr, coloured only on a terminal, so stdout stays
    // clean for pipes.
    tracing_subscriber::fmt()
        .with_env_filter("kubeloupe=info")
        .with_writer(std::io::stderr)
        .with_ansi(std::io::stderr().is_terminal())
        .init();

    let cli = Cli::parse();
    match run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {}", error_text(&e));
            ExitCode::FAILURE
        }
    }
}

/// Let `kubeloupe get pods | head` end quietly, like other Unix tools,
/// instead of panicking on a closed pipe.
fn reset_sigpipe() {
    #[cfg(unix)]
    // SAFETY: `signal` with SIG_DFL only changes the process-wide action for
    // SIGPIPE; it touches no memory of ours.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

async fn run(cli: Cli) -> Result<()> {
    if let Commands::Completions { shell } = cli.command {
        clap_complete::generate(
            shell,
            &mut Cli::command(),
            "kubeloupe",
            &mut std::io::stdout(),
        );
        return Ok(());
    }
    // Exec credential plugins may prompt only when we have a terminal.
    let client = client_with(
        cli.kubeconfig.as_deref(),
        cli.context.as_deref(),
        std::io::stdin().is_terminal(),
    )
    .await?;
    match cli.command {
        Commands::Completions { .. } => unreachable!("handled above"),
        Commands::Pods { namespace } => list_pods(&client, namespace.as_deref()).await?,
        Commands::Get {
            kind,
            name,
            namespace,
            output,
            selector,
            field_selector,
            reveal,
            all_namespaces: _,
        } => {
            let (kind, name) = split_target(&kind, name.as_deref())?;
            let opts = GetOptions {
                namespace,
                output,
                selector,
                field_selector,
                reveal,
            };
            print_get(&client, kind, name, &opts).await?
        }
        Commands::Describe {
            target,
            name,
            namespace,
        } => match split_target(&target, name.as_deref())? {
            (kind, Some(name)) => print_describe(&client, kind, name, namespace.as_deref()).await?,
            (_, None) => anyhow::bail!("describe needs <kind>/<name> or <kind> <name>"),
        },
        Commands::Watch {
            resource,
            namespace,
        } => watch_and_print(&client, &resource, namespace.as_deref()).await?,
        Commands::Namespaces => list_namespaces(&client).await?,
        Commands::Nodes => list_nodes(&client).await?,
        Commands::Logs {
            name,
            namespace,
            tail,
            container,
            previous,
            follow,
        } => {
            let opts = LogOptions {
                container,
                previous,
                tail_lines: Some(tail),
                ..LogOptions::default()
            };
            match parse_log_target(&name)? {
                LogTarget::Pod(pod) => {
                    print_pod_logs(&client, namespace.as_deref(), &pod, &opts, follow).await?
                }
                LogTarget::Workload(kind, workload) => {
                    print_workload_logs(
                        &client,
                        namespace.as_deref(),
                        kind,
                        &workload,
                        &opts,
                        follow,
                    )
                    .await?
                }
            }
        }
        Commands::Top {
            resource,
            namespace,
        } => match resource.as_str() {
            "pods" => print_top_pods(&client, namespace.as_deref()).await?,
            "nodes" if namespace.is_some() => anyhow::bail!("nodes are not namespaced: drop -n"),
            "nodes" => print_top_nodes(&client).await?,
            other => anyhow::bail!("unknown resource for top: {other} (use pods or nodes)"),
        },
        Commands::Scale {
            target,
            replicas,
            namespace,
        } => {
            cli_action(
                &client,
                &target,
                namespace.as_deref(),
                ActionId::Scale,
                Some(replicas),
            )
            .await?
        }
        Commands::Restart { target, namespace } => {
            cli_action(
                &client,
                &target,
                namespace.as_deref(),
                ActionId::Restart,
                None,
            )
            .await?
        }
        Commands::Cordon { node } => {
            cli_action(
                &client,
                &format!("node/{node}"),
                None,
                ActionId::Cordon,
                None,
            )
            .await?
        }
        Commands::Uncordon { node } => {
            cli_action(
                &client,
                &format!("node/{node}"),
                None,
                ActionId::Uncordon,
                None,
            )
            .await?
        }
        Commands::Drain {
            node,
            timeout,
            pod_selector,
        } => {
            let opts = DrainOptions {
                timeout: std::time::Duration::from_secs(timeout),
                pod_selector,
            };
            cli_drain(&client, &node, &opts).await?
        }
        Commands::Trigger { target, namespace } => {
            cli_action(
                &client,
                &target,
                namespace.as_deref(),
                ActionId::Trigger,
                None,
            )
            .await?
        }
        Commands::Suspend { target, namespace } => {
            cli_action(
                &client,
                &target,
                namespace.as_deref(),
                ActionId::Suspend,
                None,
            )
            .await?
        }
        Commands::Resume { target, namespace } => {
            cli_action(
                &client,
                &target,
                namespace.as_deref(),
                ActionId::Resume,
                None,
            )
            .await?
        }
        Commands::Delete {
            target,
            namespace,
            force,
        } => cli_delete(&client, &target, namespace.as_deref(), force).await?,
        Commands::Apply {
            file,
            namespace,
            dry_run,
        } => kubeloupe::resources::cli_apply(&client, &file, namespace.as_deref(), dry_run).await?,
        Commands::Yaml {
            target,
            namespace,
            reveal,
        } => {
            // A bare name is a pod, as before generic kinds were supported.
            let (kind, name) = match split_target(&target, None)? {
                (kind, Some(name)) => (kind, name),
                (name, None) => ("pod", name),
            };
            print_yaml(&client, kind, name, namespace.as_deref(), reveal).await?
        }
        Commands::PortForward {
            target,
            ports,
            namespace,
        } => run_port_forward(&client, namespace.as_deref(), &target, &ports).await?,
        Commands::Debug {
            pod,
            namespace,
            container,
            image,
        } => {
            kubeloupe::terminal::cli_debug(
                &client,
                namespace.as_deref().unwrap_or("default"),
                &pod,
                container.as_deref(),
                &image,
            )
            .await?
        }
        Commands::NodeShell {
            node,
            namespace,
            image,
        } => kubeloupe::terminal::cli_node_shell(&client, &node, &namespace, &image).await?,
        Commands::Exec {
            name,
            namespace,
            container,
            command,
        } => {
            exec_in_pod(
                &client,
                namespace.as_deref(),
                &name,
                container.as_deref(),
                &command,
            )
            .await?
        }
    }
    Ok(())
}
