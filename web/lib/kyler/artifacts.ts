export type Artifact = {
  id: string;
  session_id: string;
  execution_id: string;
  reference: string;
  resource?: boolean;
  primary?: boolean;
  name: string;
  mime_type: string;
  size_bytes: number;
  available: boolean;
  error: string | null;
};

export function artifactReference(value?: string) {
  if (
    !value ||
    value.startsWith("//") ||
    /^[#?]/.test(value) ||
    value.split(/[/?#]/, 1)[0].includes(":")
  )
    return null;
  try {
    return decodeURIComponent(value.split(/[?#]/, 1)[0])
      .split("/")
      .filter((part) => part !== ".")
      .join("/");
  } catch {
    return null;
  }
}
export function findArtifact(
  items: Artifact[],
  session: string,
  execution: unknown,
  reference: string,
) {
  const key = artifactReference(reference);
  return key === null
    ? undefined
    : items.find(
        (item) =>
          item.session_id === session &&
          item.execution_id === execution &&
          artifactReference(item.reference) === key,
      );
}
export function artifactSize(bytes: number) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

/** Sort session files by their execution's position in the conversation, newest first. */
export function orderedArtifacts(items: Artifact[], session: string, executions: string[]) {
  const order = new Map(executions.map((id, index) => [id, index]));
  return items
    .filter((item) => item.session_id === session)
    .toSorted(
      (a, b) =>
        (order.get(b.execution_id) ?? -1) - (order.get(a.execution_id) ?? -1) ||
        a.name.localeCompare(b.name) ||
        a.id.localeCompare(b.id),
    );
}
