import assert from "node:assert/strict";
import { createHmac, randomUUID } from "node:crypto";
import test from "node:test";
import {
  ProductAdminError,
  authorizeProductAdmin,
  requireCorrelationId,
} from "./product-admin.ts";

const signingKey = "axio-test-signing-key-that-is-long-enough";

function token(overrides: Record<string, unknown> = {}) {
  const now = Math.floor(Date.now() / 1000);
  const header = Buffer.from(JSON.stringify({ alg: "HS256", typ: "JWT" })).toString("base64url");
  const payload = Buffer.from(JSON.stringify({
    iss: "admin.umbra.me",
    sub: randomUUID(),
    aud: "axio",
    umbra_user_id: randomUUID(),
    permissions: ["capabilities.read", "overview.read"],
    session_id: randomUUID(),
    jti: randomUUID(),
    iat: now,
    exp: now + 120,
    ...overrides,
  })).toString("base64url");
  const signature = createHmac("sha256", signingKey)
    .update(`${header}.${payload}`)
    .digest("base64url");
  return `${header}.${payload}.${signature}`;
}

function request(rawToken = token(), correlationId: string = randomUUID()) {
  return new Request("https://axio.sh/admin/v1/capabilities", {
    headers: {
      Authorization: `Bearer ${rawToken}`,
      "X-Umbra-Correlation-Id": correlationId,
    },
  });
}

test.beforeEach(() => {
  process.env.PRODUCT_ADMIN_SIGNING_KEY = signingKey;
});

test("accepts the exact audience, subject, permission, and correlation id", () => {
  const input = request();
  assert.equal(authorizeProductAdmin(input, "overview.read").aud, "axio");
  assert.match(requireCorrelationId(input), /^[0-9a-f-]{36}$/i);
});

test("rejects another product audience", () => {
  assert.throws(
    () => authorizeProductAdmin(request(token({ aud: "exploit" })), "overview.read"),
    (error: unknown) => error instanceof ProductAdminError && error.code === "INVALID_TOKEN",
  );
});

test("rejects missing permissions", () => {
  assert.throws(
    () => authorizeProductAdmin(request(token({ permissions: ["capabilities.read"] })), "overview.read"),
    (error: unknown) => error instanceof ProductAdminError && error.code === "FORBIDDEN",
  );
});

test("rejects tokens without a stable Umbra subject", () => {
  assert.throws(
    () => authorizeProductAdmin(request(token({ umbra_user_id: undefined })), "overview.read"),
    (error: unknown) => error instanceof ProductAdminError && error.code === "INVALID_TOKEN",
  );
});

test("rejects an invalid correlation id", () => {
  assert.throws(
    () => requireCorrelationId(request(token(), "not-a-uuid")),
    (error: unknown) =>
      error instanceof ProductAdminError && error.code === "INVALID_CORRELATION_ID",
  );
});

test("rejects tokens whose original lifetime was too long", () => {
  const now = Math.floor(Date.now() / 1000);
  assert.throws(
    () => authorizeProductAdmin(request(token({ iat: now, exp: now + 181 })), "overview.read"),
    (error: unknown) => error instanceof ProductAdminError && error.code === "INVALID_TOKEN",
  );
});
