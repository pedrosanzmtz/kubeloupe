use anyhow::Result;
use clap::{Parser, Subcommand};
use kubeloupe::{list_namespaces, list_nodes, list_pods};

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
    }
    Ok(())
}
