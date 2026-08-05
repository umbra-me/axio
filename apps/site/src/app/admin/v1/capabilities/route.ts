import { capabilities } from "@/lib/product-admin-data";
import {
  authorizeProductAdmin,
  productAdminError,
  requireCorrelationId,
} from "@/lib/product-admin";

export const dynamic = "force-dynamic";
export const runtime = "nodejs";

export async function GET(request: Request) {
  try {
    authorizeProductAdmin(request, "capabilities.read");
    requireCorrelationId(request);
    return Response.json(capabilities, {
      headers: { "Cache-Control": "private, no-store" },
    });
  } catch (reason) {
    const error = productAdminError(reason);
    return Response.json(
      { error: { code: error.code, message: error.message } },
      { status: error.status, headers: { "Cache-Control": "private, no-store" } },
    );
  }
}
