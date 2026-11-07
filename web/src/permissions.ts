/** Host approval is authoritative; a request never implies a grant. */
export const permissionDetails = {
  desktop: ["View desktop", "See the host screen and save screenshots."],
  input: [
    "Control keyboard and pointer",
    "Send clicks, keystrokes and text to the host.",
  ],
  shell: ["Run a terminal", "Run commands as the host user."],
  file_read: [
    "Read shared files",
    "Browse and download files in the shared directory.",
  ],
  file_write: [
    "Change shared files",
    "Upload files, create folders and remove shared items.",
  ],
  clipboard_read: [
    "Read host clipboard",
    "Read clipboard text only when you choose.",
  ],
  clipboard_write: [
    "Write host clipboard",
    "Replace host clipboard text only when you choose.",
  ],
} as const;
export type Scope = keyof typeof permissionDetails;
export const scopes = Object.keys(permissionDetails) as Scope[];
export interface PermissionSnapshot {
  available: Scope[];
  granted: Scope[];
  pending: { id: number; scopes: Scope[] } | null;
  last_decision: {
    id: number;
    outcome: "approved" | "denied" | "cancelled";
  } | null;
  revision: number;
  mode: "attended" | "unattended";
}
export function parsePermissions(value: unknown): PermissionSnapshot {
  const bad = () => {
    throw new Error("Invalid host permissions");
  };
  if (!value || typeof value !== "object") return bad();
  const data = value as Record<string, unknown>;
  function list(value: unknown): Scope[] {
    if (
      !Array.isArray(value) ||
      value.some(
        (s) => typeof s !== "string" || !scopes.includes(s as Scope),
      ) ||
      new Set(value).size !== value.length
    )
      return bad();
    return [...value] as Scope[];
  }
  const available = list(data.available),
    granted = list(data.granted);
  if (
    !Number.isSafeInteger(data.revision) ||
    Number(data.revision) < 0 ||
    Number(data.revision) > 0xffffffff ||
    !["attended", "unattended"].includes(String(data.mode)) ||
    granted.some((s) => !available.includes(s))
  )
    return bad();
  let pending: PermissionSnapshot["pending"] = null;
  if (data.pending !== null) {
    if (!data.pending || typeof data.pending !== "object") return bad();
    const request = data.pending as Record<string, unknown>;
    if (!Number.isSafeInteger(request.id) || Number(request.id) < 0)
      return bad();
    pending = { id: Number(request.id), scopes: list(request.scopes) };
    if (
      !pending.scopes.length ||
      pending.scopes.some((s) => !available.includes(s))
    )
      return bad();
  }
  let last_decision: PermissionSnapshot["last_decision"] = null;
  if (data.last_decision != null) {
    if (typeof data.last_decision !== "object") return bad();
    const decision = data.last_decision as Record<string, unknown>;
    if (
      !Number.isSafeInteger(decision.id) ||
      Number(decision.id) < 0 ||
      !["approved", "denied", "cancelled"].includes(String(decision.outcome))
    )
      return bad();
    last_decision = {
      id: Number(decision.id),
      outcome: decision.outcome as NonNullable<
        PermissionSnapshot["last_decision"]
      >["outcome"],
    };
  }
  return {
    available,
    granted,
    pending,
    last_decision,
    revision: Number(data.revision),
    mode: data.mode as PermissionSnapshot["mode"],
  };
}
const route: Partial<Record<Scope, string>> = {
  desktop: "desktop",
  input: "input",
  shell: "shell",
  file_read: "file.get",
  file_write: "file.put",
};
export function effectivePermission(
  snapshot: PermissionSnapshot | undefined,
  capabilities: Record<string, unknown> | undefined,
  scope: Scope,
): boolean {
  if (!snapshot?.granted.includes(scope) || !capabilities) return false;
  const enabled = capabilities[scope] === true;
  const requiredRoute = route[scope];
  return (
    enabled &&
    (!requiredRoute ||
      !Array.isArray(capabilities.routes) ||
      capabilities.routes.includes(requiredRoute))
  );
}
/** Ignore late replies so a revoked permission cannot be restored by an older response. */
export function acceptPermissions(
  current: PermissionSnapshot | undefined,
  next: PermissionSnapshot,
): boolean {
  return !current || next.revision >= current.revision;
}
