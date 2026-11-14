use anyhow::Result;
use clap::{Parser, Subcommand};
use kubeloupe::{
    client, exec_in_pod, list_namespaces, list_nodes, list_pods, parse_log_target, print_get,
    print_pod_logs, print_pod_yaml, print_top_nodes, print_top_pods, print_workload_logs,
    run_port_forward, watch_and_print, LogOptions, LogTarget,
};

#[derive(Parser)]
#[command(name = "kubeloupe", about = "A Lens alternative written in Rust")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// List pods (all namespaces by default)
    Pods {
        /// Only list pods in this namespace
        #[arg(short, long)]
        namespace: Option<String>,
    },
    /// List namespaces
    Namespaces,
    /// List any resource type, including CRDs (like kubectl get)
    Get {
        /// Resource type: plural, kind or short name (deploy, svc, widgets.example.com)
        kind: String,
        /// Only list this namespace (default: all namespaces)
        #[arg(short, long)]
        namespace: Option<String>,
    },
    /// Stream live changes to any resource type (until Ctrl-C)
    Watch {
        /// What to watch: pods, nodes, deployments, a CRD, ...
        #[arg(default_value = "pods")]
        resource: String,
        /// Only watch this namespace
        #[arg(short, long)]
        namespace: Option<String>,
    },
    /// List nodes
    Nodes,
    /// Show the last lines of a pod's logs, or of every pod of a workload
    Logs {
        /// Pod name, or `deploy/<name>`, `sts/<name>`, `ds/<name>`, `job/<name>`
        /// to merge the logs of all its pods (`[pod] line`)
        name: String,
        /// Pod namespace
        #[arg(short, long)]
        namespace: Option<String>,
        /// Number of lines from the end of the logs
        #[arg(long, default_value_t = 200)]
        tail: i64,
        /// Container name (including init containers); default container if omitted
        #[arg(short, long)]
        container: Option<String>,
        /// Show the logs of the previous (crashed) container instance
        #[arg(short, long)]
        previous: bool,
        /// Stream new log lines until interrupted
        #[arg(short, long)]
        follow: bool,
    },
    /// Show CPU/memory usage (like kubectl top)
    Top {
        /// What to show: pods or nodes
        #[arg(default_value = "pods")]
        resource: String,
    },
    /// Print a pod as YAML
    Yaml {
        /// Pod name
        name: String,
        /// Pod namespace
        #[arg(short, long)]
        namespace: Option<String>,
    },
    /// Forward a pod port to localhost (until Ctrl-C)
    #[command(name = "port-forward")]
    PortForward {
        /// Pod name
        name: String,
        /// Pod port to forward
        port: u16,
        /// Pod namespace
        #[arg(short, long)]
        namespace: Option<String>,
    },
    /// Run a command in a pod (non-interactive)
    Exec {
        /// Pod name
        name: String,
        /// Pod namespace
        #[arg(short, long)]
        namespace: Option<String>,
        /// Container name (defaults to the pod's only container)
        #[arg(short, long)]
        container: Option<String>,
        /// Command to run (after --)
        #[arg(last = true, required = true)]
        command: Vec<String>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("kubeloupe=info")
        .init();

    let cli = Cli::parse();
    let client = client().await?;
    match cli.command {
        Commands::Pods { namespace } => list_pods(&client, namespace.as_deref()).await?,
        Commands::Get { kind, namespace } => {
            print_get(&client, &kind, namespace.as_deref()).await?
        }
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
        Commands::Top { resource } => match resource.as_str() {
            "pods" => print_top_pods(&client).await?,
            "nodes" => print_top_nodes(&client).await?,
            other => anyhow::bail!("unknown resource for top: {other} (use pods or nodes)"),
        },
        Commands::Yaml { name, namespace } => {
            print_pod_yaml(&client, namespace.as_deref(), &name).await?
        }
        Commands::PortForward {
            name,
            port,
            namespace,
        } => run_port_forward(&client, namespace.as_deref(), &name, port).await?,
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
