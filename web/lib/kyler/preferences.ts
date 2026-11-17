import { api } from "./client.ts";
export type UiPreferences = {
  last_session_id: string | null;
  sidebar_collapsed: boolean;
  revision: number;
};
export type PreferencePatch = { last_session_id?: string | null; sidebar_collapsed?: boolean };
export const readPreferences = () => api<UiPreferences>("preferences");
let writes: Promise<unknown> = Promise.resolve();
// Preserve this browser's operation order; the server atomically patches supplied fields.
export function patchPreferences(patch: PreferencePatch): Promise<UiPreferences> {
  const result = writes
    .catch(() => {})
    .then(() => api<UiPreferences>("preferences", { method: "PUT", body: JSON.stringify(patch) }));
  writes = result;
  return result;
}
export function initialSession(explicit: string | null, preferences: UiPreferences) {
  return explicit ?? preferences.last_session_id;
}
