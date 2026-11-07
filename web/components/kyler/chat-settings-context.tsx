"use client";

import { createContext, useContext, type ReactNode } from "react";
import type { Session } from "@/lib/kyler/types";

type ChatSettings = {
  session: Session;
  initialModel: string;
  startSkillCreation: () => Promise<void>;
  ensureSession: () => Promise<Session>;
  createSession: (workspace?: string | null) => Promise<void>;
};
const Context = createContext<ChatSettings | null>(null);

export function ChatSettingsProvider({
  children,
  ...settings
}: ChatSettings & { children: ReactNode }) {
  return <Context.Provider value={settings}>{children}</Context.Provider>;
}
export function useChatSettings() {
  const value = useContext(Context);
  if (!value) throw new Error("Chat settings require a session.");
  return value;
}
