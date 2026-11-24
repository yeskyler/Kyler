"use client";

import { useEffect, useState } from "react";
import {
  BotIcon,
  CheckIcon,
  ChevronDownIcon,
  CpuIcon,
  MessagesSquareIcon,
  SlidersHorizontalIcon,
  UserIcon,
} from "lucide-react";
import { Menu } from "@base-ui/react/menu";
import { Button } from "@/components/ui/button";
import { api, redirectToLogin } from "@/lib/kyler/client";
import type { Workspace } from "@/lib/kyler/types";
import { SubagentsPanel } from "./subagents-panel";
import { ProvidersPanel } from "./providers-panel";
import { ChannelsSettings } from "./channels-dialog";

const sections = [
  { id: "account", label: "Account", icon: UserIcon, description: "Manage your access to Kyler." },
  {
    id: "general",
    label: "General",
    icon: SlidersHorizontalIcon,
    description: "Personalize your workspace appearance.",
  },
  {
    id: "models",
    label: "Providers",
    icon: CpuIcon,
    description: "Configure model provider connections.",
  },
  {
    id: "subagents",
    label: "Subagents",
    icon: BotIcon,
    description: "Configure delegation and child task limits.",
  },
  {
    id: "channels",
    label: "Channels",
    icon: MessagesSquareIcon,
    description: "Connect external chats to your workspaces.",
  },
] as const;
type Section = (typeof sections)[number]["id"];

export function SettingsPanel({ defaultWorkspace }: { defaultWorkspace?: string }) {
  const [section, setSection] = useState<Section>("general");
  const [workspaces, setWorkspaces] = useState<Workspace[]>([]);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [theme, setTheme] = useState("light");

  useEffect(() => {
    const saved = localStorage.getItem("kyler.settings.section");
    if (sections.some((item) => item.id === saved)) setSection(saved as Section);
    setTheme(document.documentElement.classList.contains("dark") ? "dark" : "light");
  }, []);

  useEffect(() => {
    const controller = new AbortController();
    setError("");
    if (section === "channels") {
      api<Workspace[]>("workspaces", { signal: controller.signal })
        .then((value) => {
          if (!controller.signal.aborted) setWorkspaces(value);
        })
        .catch((error: Error) => {
          if (!controller.signal.aborted) setError(error.message);
        });
    }
    return () => controller.abort();
  }, [section]);

  const current = sections.find((item) => item.id === section)!;
  return (
    <div className="flex min-h-0 flex-1 flex-col border-t md:flex-row">
      <nav
        aria-label="Settings categories"
        className="flex shrink-0 gap-1 overflow-x-auto border-b p-3 md:w-48 md:flex-col md:border-r md:border-b-0 md:p-4"
      >
        {sections.map(({ id, label, icon: Icon }) => (
          <Button
            key={id}
            variant="ghost"
            aria-current={section === id ? "page" : undefined}
            className="shrink-0 justify-start gap-3 aria-[current=page]:bg-accent"
            onClick={() => {
              setSection(id);
              localStorage.setItem("kyler.settings.section", id);
            }}
          >
            <Icon className="size-4" />
            {label}
          </Button>
        ))}
      </nav>
      <div className="min-w-0 flex-1 overflow-y-auto p-5 sm:p-8">
        <div className="mx-auto max-w-3xl space-y-7">
          <header className="space-y-2">
            <h2 className="text-2xl font-semibold tracking-tight">{current.label}</h2>
            <p className="text-sm text-muted-foreground">{current.description}</p>
          </header>
          {section === "account" && (
            <section className="space-y-4 rounded-xl border p-5">
              <h3 className="font-medium">Session access</h3>
              <p className="text-sm text-muted-foreground">Sign out of Kyler on this browser.</p>
              <Button
                variant="outline"
                disabled={busy}
                onClick={async () => {
                  setBusy(true);
                  setError("");
                  try {
                    const response = await fetch("/api/auth/logout", { method: "POST" });
                    if (!response.ok) throw new Error("Cannot sign out. Try again.");
                    redirectToLogin();
                  } catch (error) {
                    setError((error as Error).message);
                  } finally {
                    setBusy(false);
                  }
                }}
              >
                {busy ? "Signing out…" : "Sign out"}
              </Button>
            </section>
          )}
          {section === "general" && (
            <section className="flex flex-wrap items-center justify-between gap-4 rounded-xl border p-5">
              <div>
                <h3 className="font-medium">Appearance</h3>
                <p className="mt-1 text-sm text-muted-foreground">
                  Choose a light or dark workspace.
                </p>
              </div>
              <Menu.Root>
                <Menu.Trigger
                  render={
                    <Button
                      variant="outline"
                      className="min-w-32 justify-between bg-background"
                      aria-label="Appearance"
                    />
                  }
                >
                  {theme === "dark" ? "Dark" : "Light"}
                  <ChevronDownIcon className="size-4 text-muted-foreground" />
                </Menu.Trigger>
                <Menu.Portal>
                  <Menu.Positioner side="bottom" align="end" sideOffset={6} className="z-50">
                    <Menu.Popup
                      aria-label="Appearance"
                      className="min-w-40 rounded-xl border bg-popover p-1.5 text-popover-foreground shadow-lg outline-none"
                    >
                      <Menu.RadioGroup
                        value={theme}
                        onValueChange={(value) => {
                          setTheme(value);
                          localStorage.setItem("kyler.theme", value);
                          document.documentElement.classList.toggle("dark", value === "dark");
                        }}
                      >
                        {["light", "dark"].map((value) => (
                          <Menu.RadioItem
                            key={value}
                            value={value}
                            closeOnClick
                            className="flex min-h-9 cursor-default items-center justify-between gap-3 rounded-lg px-3 py-2 text-sm outline-none data-[highlighted]:bg-accent data-[highlighted]:text-accent-foreground"
                          >
                            {value === "dark" ? "Dark" : "Light"}
                            <Menu.RadioItemIndicator>
                              <CheckIcon className="size-3.5" />
                            </Menu.RadioItemIndicator>
                          </Menu.RadioItem>
                        ))}
                      </Menu.RadioGroup>
                    </Menu.Popup>
                  </Menu.Positioner>
                </Menu.Portal>
              </Menu.Root>
            </section>
          )}
          {section === "models" && <ProvidersPanel />}
          {section === "subagents" && <SubagentsPanel />}
          {section === "channels" && (
            <ChannelsSettings workspaces={workspaces} defaultWorkspace={defaultWorkspace} />
          )}
          {error && (
            <p role="alert" className="text-sm text-destructive">
              {error}
            </p>
          )}
        </div>
      </div>
    </div>
  );
}
