use anyhow::Result;
use clap::{Parser, Subcommand};
use kubeloupe::{list_namespaces, list_nodes, list_pods, print_pod_logs};

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
    /// List nodes
    Nodes,
    /// Show the last lines of a pod's logs
    Logs {
        /// Pod name
        name: String,
        /// Pod namespace
        #[arg(short, long)]
        namespace: Option<String>,
        /// Number of lines from the end of the logs
        #[arg(long, default_value_t = 200)]
        tail: i64,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("kubeloupe=info")
        .init();

    let cli = Cli::parse();
    match cli.command {
        Commands::Pods { namespace } => list_pods(namespace.as_deref()).await?,
        Commands::Namespaces => list_namespaces().await?,
        Commands::Nodes => list_nodes().await?,
        Commands::Logs { name, namespace, tail } => {
            print_pod_logs(namespace.as_deref(), &name, tail).await?
        }
    }
    Ok(())
}
