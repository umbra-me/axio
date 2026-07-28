import { readFileSync } from "node:fs";
import { join } from "node:path";

// Served at https://axio.sh/install.ps1 for `irm https://axio.sh/install.ps1 | iex`.
// See ../install/route.ts for why the script is a file rather than a string.
export const dynamic = "force-static";

const script = readFileSync(
  join(process.cwd(), "scripts", "install.ps1"),
  "utf8",
);

export function GET() {
  return new Response(script, {
    headers: {
      "content-type": "text/plain; charset=utf-8",
      "cache-control": "public, max-age=300, must-revalidate",
      "x-content-type-options": "nosniff",
    },
  });
}
