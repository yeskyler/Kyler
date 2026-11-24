"use client";

import { useEffect, useState } from "react";
import {
  BotIcon,
  CheckIcon,
  ChevronDownIcon,
  CpuIcon,
  GitBranchIcon,
  InfoIcon,
  ArrowUpRightIcon,
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
  {
    id: "about",
    label: "About",
    icon: InfoIcon,
    description: "Project information and source code.",
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
          {section === "about" && (
            <div className="space-y-6">
              <a
                href="https://github.com/yeskyler/Kyler"
                target="_blank"
                rel="noopener noreferrer"
                className="flex items-center gap-4 rounded-xl border bg-card p-5 outline-none transition-colors hover:bg-accent/50 focus-visible:ring-2 focus-visible:ring-ring"
              >
                <GitBranchIcon className="size-6 shrink-0" aria-hidden />
                <div className="min-w-0 flex-1">
                  <h3 className="font-medium">GitHub repository</h3>
                  <p className="mt-1 break-all text-sm text-muted-foreground">
                    https://github.com/yeskyler/Kyler
                  </p>
                </div>
                <ArrowUpRightIcon className="size-4 shrink-0 text-muted-foreground" aria-hidden />
                <span className="sr-only">Opens in a new tab</span>
              </a>
              <section className="space-y-2 px-1">
                <h3 className="text-lg font-semibold tracking-tight">Kyler</h3>
                <p className="text-sm text-muted-foreground">
                  An AI assistant for coding, research, and everyday work.
                </p>
              </section>
            </div>
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
