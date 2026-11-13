#!/usr/bin/env bash
# Sets up the kubeloupe test clusters with kwok (Kubernetes WithOut Kubelet):
# real API servers (kube-apiserver + etcd) with simulated nodes and pods.
# No Docker, no VM, no cgroup support needed — that is why kwok, and not
# kind/k3s/minikube, is the test route for this repo.
#
# Creates:
#   kubeloupe-test     — 2 nodes + the demo workload (demo.yaml)
#   kubeloupe-staging  — 1 node, empty (for testing the context switcher)
# Merges both contexts into ~/.kube/config and leaves kubeloupe-test current.
#
# Usage: bash scripts/test-cluster/setup.sh
# Tear down: kwokctl delete cluster --name kubeloupe-test
#            kwokctl delete cluster --name kubeloupe-staging
set -euo pipefail

KWOK_VERSION="${KWOK_VERSION:-v0.8.0}"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

# kubectl: prefer the one on PATH, else the copy kwokctl downloaded.
if command -v kubectl >/dev/null 2>&1; then
  KUBECTL="$(command -v kubectl)"
else
  KUBECTL="$(find "$HOME/.kwok/cache" -name kubectl -type f 2>/dev/null | head -1)"
fi

# 1. Install kwokctl + kwok if missing.
if ! command -v kwokctl >/dev/null 2>&1; then
  OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
  ARCH="$(uname -m | sed 's/x86_64/amd64/;s/aarch64/arm64/')"
  echo "Installing kwokctl ${KWOK_VERSION} (${OS}/${ARCH})..."
  curl -sfL "https://github.com/kubernetes-sigs/kwok/releases/download/${KWOK_VERSION}/kwokctl-${OS}-${ARCH}" -o /tmp/kwokctl
  curl -sfL "https://github.com/kubernetes-sigs/kwok/releases/download/${KWOK_VERSION}/kwok-${OS}-${ARCH}" -o /tmp/kwok
  chmod +x /tmp/kwokctl /tmp/kwok
  if [ -w /usr/local/bin ]; then
    mv /tmp/kwokctl /tmp/kwok /usr/local/bin/
  else
    sudo mv /tmp/kwokctl /tmp/kwok /usr/local/bin/
  fi
fi

# 2. Create the clusters (skip ones that already exist).
for cluster in kubeloupe-test kubeloupe-staging; do
  if kwokctl get clusters 2>/dev/null | grep -qx "$cluster"; then
    echo "Cluster $cluster already exists, skipping create."
  else
    kwokctl create cluster --name "$cluster"
  fi
done
kwokctl scale node --replicas 2 --name kubeloupe-test
kwokctl scale node --replicas 1 --name kubeloupe-staging

# kubectl is downloaded by kwokctl during cluster creation; find it now
# if it was not on PATH before.
if [ -z "$KUBECTL" ]; then
  KUBECTL="$(find "$HOME/.kwok/cache" -name kubectl -type f 2>/dev/null | head -1)"
fi
[ -n "$KUBECTL" ] || { echo "kubectl not found (expected in ~/.kwok/cache)" >&2; exit 1; }

# 3. Demo workload on the test cluster.
"$KUBECTL" --kubeconfig="$HOME/.kwok/clusters/kubeloupe-test/kubeconfig.yaml" \
  apply -f "$SCRIPT_DIR/demo.yaml"
sleep 5
# Seed a Failed pod: kwok simulates pods to Running, so patch the status
# of `crashed` to Failed with a terminated container and 3 restarts.
"$KUBECTL" --kubeconfig="$HOME/.kwok/clusters/kubeloupe-test/kubeconfig.yaml" \
  patch pod crashed -n demo --subresource=status --type=merge \
  -p '{"status":{"phase":"Failed","containerStatuses":[{"name":"app","ready":false,"restartCount":3,"state":{"terminated":{"reason":"Error","exitCode":1}}}]}}'

# 4. Merge both contexts into ~/.kube/config, test cluster current.
mkdir -p "$HOME/.kube"
kwokctl get kubeconfig --name kubeloupe-test > "$HOME/.kube/config"
KUBECONFIG="$HOME/.kube/config:$HOME/.kwok/clusters/kubeloupe-staging/kubeconfig.yaml" \
  "$KUBECTL" config view --flatten > /tmp/kubeloupe-kubeconfig.yaml
cp /tmp/kubeloupe-kubeconfig.yaml "$HOME/.kube/config"
chmod 600 "$HOME/.kube/config"
"$KUBECTL" --kubeconfig="$HOME/.kube/config" config use-context kwok-kubeloupe-test

echo
echo "Test clusters ready:"
"$KUBECTL" --kubeconfig="$HOME/.kube/config" config get-contexts
echo "Run the app with: cargo run -p kubeloupe-desktop"
