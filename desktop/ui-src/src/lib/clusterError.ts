/**
 * Status text and banner message for an error from loading cluster data.
 *
 * Connecting to a cluster fails with a readable message (see `src/auth.rs`);
 * its prefix tells a failed login or a certificate problem apart from an
 * unreachable cluster. Keep the prefixes in sync with that file.
 */
export function clusterError(e: unknown): { status: string; message: string } {
  const message = String(e);
  if (message.startsWith("Login to `")) return { status: "Login failed", message };
  if (message.startsWith("Certificate problem")) return { status: "Certificate problem", message };
  if (message.startsWith("Cannot reach the cluster")) {
    return { status: "Cluster unreachable", message };
  }
  return { status: "Cluster unreachable", message: "Could not reach the cluster: " + message };
}
