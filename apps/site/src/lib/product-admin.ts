import { createHmac, timingSafeEqual } from "node:crypto";

const MAX_TOKEN_LIFETIME_SECONDS = 180;
const MAX_CLOCK_SKEW_SECONDS = 30;
const UUID_PATTERN =
  /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;

interface ProductAdminClaims {
  iss: string;
  sub: string;
  aud: string;
  umbra_user_id: string;
  permissions: string[];
  session_id: string;
  jti: string;
  iat: number;
  exp: number;
}

export class ProductAdminError extends Error {
  readonly status: number;
  readonly code: string;

  constructor(message: string, status: number, code: string) {
    super(message);
    this.name = "ProductAdminError";
    this.status = status;
    this.code = code;
  }
}

function rejectToken(): never {
  throw new ProductAdminError("Invalid Product Admin token", 401, "INVALID_TOKEN");
}

function parseJsonSegment(value: string): unknown {
  try {
    return JSON.parse(Buffer.from(value, "base64url").toString("utf8"));
  } catch {
    return rejectToken();
  }
}

function isClaims(value: unknown): value is ProductAdminClaims {
  if (typeof value !== "object" || value === null) return false;
  const claims = value as Partial<ProductAdminClaims>;
  return (
    claims.iss === "admin.umbra.me" &&
    typeof claims.sub === "string" &&
    UUID_PATTERN.test(claims.sub) &&
    claims.aud === "axio" &&
    typeof claims.umbra_user_id === "string" &&
    UUID_PATTERN.test(claims.umbra_user_id) &&
    Array.isArray(claims.permissions) &&
    claims.permissions.every((permission) => typeof permission === "string") &&
    typeof claims.session_id === "string" &&
    UUID_PATTERN.test(claims.session_id) &&
    typeof claims.jti === "string" &&
    UUID_PATTERN.test(claims.jti) &&
    Number.isInteger(claims.iat) &&
    Number.isInteger(claims.exp)
  );
}

export function authorizeProductAdmin(request: Request, permission: string): ProductAdminClaims {
  const signingKey = process.env.PRODUCT_ADMIN_SIGNING_KEY;
  if (!signingKey || signingKey.length < 32) {
    throw new ProductAdminError("Product Admin is not configured", 500, "NOT_CONFIGURED");
  }

  const raw = request.headers.get("authorization")?.replace(/^Bearer /, "");
  if (!raw) {
    throw new ProductAdminError("Missing Product Admin token", 401, "UNAUTHORIZED");
  }

  const segments = raw.split(".");
  if (segments.length !== 3) return rejectToken();
  const [encodedHeader, encodedPayload, encodedSignature] = segments;
  const header = parseJsonSegment(encodedHeader);
  if (
    typeof header !== "object" ||
    header === null ||
    (header as { alg?: unknown }).alg !== "HS256"
  ) {
    return rejectToken();
  }

  const expected = createHmac("sha256", signingKey)
    .update(`${encodedHeader}.${encodedPayload}`)
    .digest();
  let received: Buffer;
  try {
    received = Buffer.from(encodedSignature, "base64url");
  } catch {
    return rejectToken();
  }
  if (received.length !== expected.length || !timingSafeEqual(received, expected)) {
    return rejectToken();
  }

  const claims = parseJsonSegment(encodedPayload);
  if (!isClaims(claims)) return rejectToken();
  const now = Math.floor(Date.now() / 1000);
  if (
    claims.iat > now + MAX_CLOCK_SKEW_SECONDS ||
    claims.exp <= now ||
    claims.exp <= claims.iat ||
    claims.exp - claims.iat > MAX_TOKEN_LIFETIME_SECONDS
  ) {
    return rejectToken();
  }
  if (!claims.permissions.includes(permission)) {
    throw new ProductAdminError("Product Admin permission denied", 403, "FORBIDDEN");
  }
  return claims;
}

export function requireCorrelationId(request: Request): string {
  const value = request.headers.get("X-Umbra-Correlation-Id")?.trim();
  if (!value || !UUID_PATTERN.test(value)) {
    throw new ProductAdminError(
      "X-Umbra-Correlation-Id is required",
      400,
      "INVALID_CORRELATION_ID",
    );
  }
  return value;
}

export function productAdminError(error: unknown) {
  return error instanceof ProductAdminError
    ? error
    : new ProductAdminError("Product Admin request failed", 500, "INTERNAL_ERROR");
}
