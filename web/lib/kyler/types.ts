export type Session = {
  cwd?: string | null;
  title?: string | null;
  draft?: boolean;
  session_id: string;
  project: string | null;
  updated_at?: number;
  status?: string | null;
};
export type ThinkingLevel = "off" | "low" | "medium" | "high" | "xhigh" | "max" | "ultra";
export type ThinkingState = {
  status: "supported" | "unsupported" | "unknown";
  source: string | null;
  profile: string;
  revision: number;
  follows_default: boolean;
  supported: boolean;
  enabled: boolean;
  level: ThinkingLevel;
  levels: ThinkingLevel[];
};
export type ModelInfo = {
  revision?: number;
  protocol?:
    | "auto"
    | "responses"
    | "chat_completions"
    | "anthropic_messages"
    | "gemini_generate_content";
  profile: string;
  model: string;
  provider: string;
  capabilities?: {
    reasoning?: { levels: ThinkingLevel[]; default: ThinkingLevel; source: string } | null;
  };
};
export type ModelCatalog = {
  models: ModelInfo[];
  default_profile: string;
  providers?: { provider: string; status: string; error?: string | null }[];
};
export type SessionModel = {
  thinking: ThinkingState | null;
  profile: string | null;
  selected: ModelInfo | null;
  active: ModelInfo | null;
  error: string | null;
};
export type PermissionMode = "read_only" | "workspace_access" | "full_access";
export type Permissions = {
  selected: { mode: PermissionMode };
  active: { mode: PermissionMode } | null;
  options: { mode: PermissionMode; available: boolean; description: string }[];
};
export const permissionLabels: Record<PermissionMode, string> = {
  read_only: "Read-only",
  workspace_access: "Workspace access",
  full_access: "Full access",
};
export type Skill = {
  path: string;
  resources: string[];
  id: string;
  name: string;
  description: string;
  source: "builtin" | "user" | "project";
  status: string;
  fingerprint: string;
  error: string | null;
};
export type Connector = {
  removable?: boolean;
  id: string;
  transport: string;
  status: string;
  tools: { name: string; description: string }[];
  error: string | null;
};

export type Workspace = { id: string; path: string; title: string };

export type AttachmentInfo = {
  id: string;
  name: string;
  kind: "image" | "text" | "pdf" | "document" | "spreadsheet";
  bytes: number;
  sha256: string;
  created_at: number;
};

export type WorkspaceInput = { session_id: string; attachment: AttachmentInfo };
