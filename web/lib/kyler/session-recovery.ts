import { api } from "./client.ts";
export type ActiveSessionRun = {
  run: { run_id: string; execution_id: string | null; status: string };
  input: string;
};
export async function activeSessionRun(session: string) {
  const state = await api<ActiveSessionRun | null>(`sessions/${session}/active-run`);
  return state && ["starting", "running", "cancelling"].includes(state.run.status) ? state : null;
}
