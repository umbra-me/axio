import { overview } from "@/lib/product-admin-data";
import {
  authorizeProductAdmin,
  productAdminError,
  requireCorrelationId,
} from "@/lib/product-admin";

export const dynamic = "force-dynamic";
export const runtime = "nodejs";

type RouteContext = {
  params: Promise<{ module: string; path?: string[] }>;
};

export async function GET(request: Request, context: RouteContext) {
  try {
    const { module, path } = await context.params;
    if (module !== "overview" || (path?.length ?? 0) > 0) {
      return Response.json(
        { error: { code: "NOT_FOUND", message: "Product Admin module not found" } },
        { status: 404, headers: { "Cache-Control": "private, no-store" } },
      );
    }
    authorizeProductAdmin(request, "overview.read");
    const correlationId = requireCorrelationId(request);
    return Response.json(
      { ...overview, correlation_id: correlationId },
      { headers: { "Cache-Control": "private, no-store" } },
    );
  } catch (reason) {
    const error = productAdminError(reason);
    return Response.json(
      { error: { code: error.code, message: error.message } },
      { status: error.status, headers: { "Cache-Control": "private, no-store" } },
    );
  }
}
