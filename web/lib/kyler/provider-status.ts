export function providerConnection(status: string): { label: string; tone: string } {
  switch (status) {
    case "connected":
      return { label: "Connected", tone: "bg-emerald-500" };
    case "reachable":
      return { label: "Reachable · authentication unverified", tone: "bg-amber-500" };
    case "authentication_failed":
      return { label: "Authentication failed", tone: "bg-destructive" };
    case "permission_denied":
      return { label: "Permission denied", tone: "bg-destructive" };
    case "network_error":
      return { label: "Network error", tone: "bg-destructive" };
    case "timeout":
      return { label: "Connection timed out", tone: "bg-destructive" };
    case "rate_limited":
      return { label: "Rate limited", tone: "bg-amber-500" };
    case "unavailable":
      return { label: "Provider unavailable", tone: "bg-destructive" };
    default:
      return { label: "Not verified", tone: "bg-muted-foreground/50" };
  }
}
export function modelDiscovery(status: string): string {
  switch (status) {
    case "ready":
      return "Model list is up to date";
    case "stale":
      return "Model list refresh failed; saved models remain available";
    case "disabled":
      return "Automatic model discovery is disabled";
    default:
      return "Model list has not been refreshed";
  }
}
