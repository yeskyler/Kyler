export type Session = {
  title?: string | null;
  draft?: boolean;
  session_id: string;
  workspace_root: string | null;
  updated_at?: number;
  status?: string | null;
};
export type ModelInfo = { profile: string; model: string; provider: string };
export type ModelCatalog = { models: ModelInfo[]; default_profile: string };
export type SessionModel = {
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
