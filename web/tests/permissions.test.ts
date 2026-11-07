import { test } from "node:test";
import assert from "node:assert/strict";
import {
  parsePermissions,
  effectivePermission,
  acceptPermissions,
  scopes,
} from "../src/permissions";
const snapshot = (extra: Record<string, unknown> = {}) =>
  parsePermissions({
    available: scopes,
    granted: [],
    pending: null,
    last_decision: null,
    revision: 0,
    mode: "attended",
    ...extra,
  });
test("permissions fail closed before grant, during request and without effective routes", () => {
  const caps = {
    desktop: true,
    clipboard: true,
    clipboard_read: false,
    clipboard_write: true,
    routes: ["control"],
  };
  assert.equal(effectivePermission(undefined, caps, "desktop"), false);
  assert.equal(
    effectivePermission(
      snapshot({ pending: { id: 1, scopes: ["desktop"] } }),
      caps,
      "desktop",
    ),
    false,
  );
  const granted = snapshot({
    granted: ["desktop", "clipboard_read", "clipboard_write"],
  });
  assert.equal(effectivePermission(granted, caps, "desktop"), false);
  assert.equal(effectivePermission(granted, caps, "clipboard_read"), false);
  assert.equal(effectivePermission(granted, caps, "clipboard_write"), true);
  assert.equal(
    effectivePermission(granted, { ...caps, routes: ["desktop"] }, "desktop"),
    true,
  );
});
test("late permission replies cannot restore revoked access", () => {
  const revoked = snapshot({ revision: 4 });
  assert.equal(
    acceptPermissions(revoked, snapshot({ revision: 3, granted: ["shell"] })),
    false,
  );
  assert.equal(acceptPermissions(revoked, snapshot({ revision: 4 })), true);
  assert.equal(
    acceptPermissions(revoked, snapshot({ revision: 5, granted: ["shell"] })),
    true,
  );
});
test("permission parser validates scopes, ceilings, requests and u32 revisions", () => {
  for (const extra of [
    { revision: -1 },
    { revision: 0x100000000 },
    { revision: 1.1 },
    { available: ["other"] },
    { available: [], granted: ["shell"] },
    { granted: ["shell", "shell"] },
    { pending: {} },
    { pending: { id: 1, scopes: [] } },
    { last_decision: { id: 2, outcome: "maybe" } },
  ])
    assert.throws(() => snapshot(extra), /Invalid host permissions/);
  for (const outcome of ["approved", "denied", "cancelled"])
    assert.equal(
      snapshot({ last_decision: { id: 3, outcome } }).last_decision?.outcome,
      outcome,
    );
});
