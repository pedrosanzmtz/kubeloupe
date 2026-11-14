//! Command-line arguments of the `kubeloupe` binary (not part of the
//! library).

use clap::{Parser, Subcommand};
use clap_complete::Shell;
use kubeloupe::OutputFormat;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "kubeloupe", about = "A Lens alternative written in Rust")]
pub struct Cli {
    /// Kubeconfig context to use (default: the current context)
    #[arg(long, global = true, value_name = "NAME")]
    pub context: Option<String>,
    /// Kubeconfig file to read (default: KUBECONFIG, then ~/.kube/config)
    #[arg(long, global = true, value_name = "PATH")]
    pub kubeconfig: Option<PathBuf>,
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// List pods (all namespaces by default)
    Pods {
        /// Only list pods in this namespace
        #[arg(short, long)]
        namespace: Option<String>,
    },
    /// List namespaces
    Namespaces,
    /// List or show any resource type, including CRDs (like kubectl get)
    Get {
        /// Resource type: plural, kind or short name (deploy, svc,
        /// widgets.example.com), or `<kind>/<name>` for one object
        kind: String,
        /// Show only this object
        name: Option<String>,
        /// Only list this namespace (default: all namespaces; for one named
        /// object, the context's namespace)
        #[arg(short, long)]
        namespace: Option<String>,
        /// List all namespaces (the default; accepted for kubectl habits)
        #[arg(short = 'A', long, conflicts_with = "namespace")]
        all_namespaces: bool,
        /// Output format
        #[arg(short, long, value_enum, default_value_t = OutputFormat::Table)]
        output: OutputFormat,
        /// Label selector, e.g. `app=web,tier!=db`
        #[arg(short = 'l', long)]
        selector: Option<String>,
        /// Field selector, e.g. `status.phase=Running`
        #[arg(long)]
        field_selector: Option<String>,
        /// Show Secret values in yaml and json output
        #[arg(long)]
        reveal: bool,
    },
    /// Show one object of any kind like kubectl describe: metadata, owners,
    /// labels, conditions and events
    Describe {
        /// `<kind>/<name>`, or the kind followed by the name
        target: String,
        name: Option<String>,
        /// Namespace (default: the context's namespace)
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
        /// Only show pods in this namespace
        #[arg(short, long)]
        namespace: Option<String>,
    },
    /// Set the replica count of a Deployment, StatefulSet or ReplicaSet
    Scale {
        /// `<kind>/<name>`, e.g. `deploy/web`
        target: String,
        #[arg(long)]
        replicas: i32,
        /// Namespace (default: `default`)
        #[arg(short, long)]
        namespace: Option<String>,
    },
    /// Restart the pods of a Deployment, StatefulSet or DaemonSet
    Restart {
        /// `<kind>/<name>`, e.g. `deploy/web`
        target: String,
        #[arg(short, long)]
        namespace: Option<String>,
    },
    /// Mark a node unschedulable
    Cordon { node: String },
    /// Mark a node schedulable again
    Uncordon { node: String },
    /// Cordon a node and evict its pods (respects PodDisruptionBudgets)
    Drain {
        node: String,
        /// Give up after this many seconds
        #[arg(long, default_value_t = 300)]
        timeout: u64,
        /// Only evict pods matching this label selector
        #[arg(long)]
        pod_selector: Option<String>,
    },
    /// Create a Job from a CronJob now
    Trigger {
        /// `cronjob/<name>`
        target: String,
        #[arg(short, long)]
        namespace: Option<String>,
    },
    /// Suspend a CronJob
    Suspend {
        /// `cronjob/<name>`
        target: String,
        #[arg(short, long)]
        namespace: Option<String>,
    },
    /// Resume a suspended CronJob
    Resume {
        /// `cronjob/<name>`
        target: String,
        #[arg(short, long)]
        namespace: Option<String>,
    },
    /// Delete an object of any kind
    Delete {
        /// `<kind>/<name>`
        target: String,
        #[arg(short, long)]
        namespace: Option<String>,
        /// Grace period 0, for pods stuck terminating (pods only)
        #[arg(long)]
        force: bool,
    },
    /// Print an object of any kind as YAML (Secret values hidden)
    Yaml {
        /// `<kind>/<name>` (`deploy/web`), or a pod name
        target: String,
        /// Namespace (default: the context's namespace)
        #[arg(short, long)]
        namespace: Option<String>,
        /// Show Secret values instead of a placeholder
        #[arg(long)]
        reveal: bool,
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
    /// Print a shell completion script, e.g.
    /// `kubeloupe completions bash > ~/.local/share/bash-completion/completions/kubeloupe`
    Completions {
        #[arg(value_enum)]
        shell: Shell,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(std::iter::once("kubeloupe").chain(args.iter().copied()))
            .unwrap_or_else(|e| panic!("{args:?}: {e}"))
    }

    #[test]
    fn definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn global_flags_work_before_and_after_the_subcommand() {
        let cli = parse(&["--context", "a", "nodes"]);
        assert_eq!(cli.context.as_deref(), Some("a"));
        let cli = parse(&["pods", "--context", "b", "--kubeconfig", "/tmp/kc"]);
        assert_eq!(cli.context.as_deref(), Some("b"));
        assert_eq!(cli.kubeconfig, Some(PathBuf::from("/tmp/kc")));
    }

    #[test]
    fn get_flags() {
        let cli = parse(&[
            "get",
            "pods",
            "-n",
            "demo",
            "-o",
            "name",
            "-l",
            "app=web",
            "--field-selector",
            "status.phase=Running",
        ]);
        let Commands::Get {
            kind,
            name,
            namespace,
            output,
            selector,
            field_selector,
            reveal,
            all_namespaces,
        } = cli.command
        else {
            panic!("not get");
        };
        assert_eq!(kind, "pods");
        assert_eq!(name, None);
        assert_eq!(namespace.as_deref(), Some("demo"));
        assert_eq!(output, OutputFormat::Name);
        assert_eq!(selector.as_deref(), Some("app=web"));
        assert_eq!(field_selector.as_deref(), Some("status.phase=Running"));
        assert!(!reveal && !all_namespaces);
        assert!(Cli::try_parse_from(["kubeloupe", "get", "pods", "-A", "-n", "x"]).is_err());
        let Commands::Get { output, name, .. } =
            parse(&["get", "deploy", "web", "-o", "yaml"]).command
        else {
            panic!("not get");
        };
        assert_eq!(output, OutputFormat::Yaml);
        assert_eq!(name.as_deref(), Some("web"));
    }

    #[test]
    fn rejects_unknown_output_format() {
        assert!(Cli::try_parse_from(["kubeloupe", "get", "pods", "-o", "xml"]).is_err());
    }

    #[test]
    fn yaml_describe_top_and_completions() {
        let Commands::Yaml { target, reveal, .. } =
            parse(&["yaml", "secret/creds", "--reveal"]).command
        else {
            panic!("not yaml");
        };
        assert_eq!(target, "secret/creds");
        assert!(reveal);
        let Commands::Describe { target, name, .. } =
            parse(&["describe", "pod", "hungry", "-n", "default"]).command
        else {
            panic!("not describe");
        };
        assert_eq!((target.as_str(), name.as_deref()), ("pod", Some("hungry")));
        let Commands::Top {
            resource,
            namespace,
        } = parse(&["top", "pods", "-n", "demo"]).command
        else {
            panic!("not top");
        };
        assert_eq!(
            (resource.as_str(), namespace.as_deref()),
            ("pods", Some("demo"))
        );
        assert!(matches!(
            parse(&["completions", "zsh"]).command,
            Commands::Completions { shell: Shell::Zsh }
        ));
    }
}
