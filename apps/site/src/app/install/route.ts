import { readFileSync } from "node:fs";
import { join } from "node:path";

// Served at https://axio.sh/install for `curl -fsSL https://axio.sh/install | sh`.
//
// The script is a real file on disk rather than a string in this module, so it
// stays shellcheck-able, diffable and readable in the repository. It is read at
// build time — the route is static, so nothing touches the filesystem per
// request.
export const dynamic = "force-static";

const script = readFileSync(
  join(process.cwd(), "scripts", "install.sh"),
  "utf8",
);

export function GET() {
  return new Response(script, {
    headers: {
      // text/plain so a browser shows it instead of downloading it: anyone
      // about to pipe this into a shell should be able to read it first.
      "content-type": "text/plain; charset=utf-8",
      "cache-control": "public, max-age=300, must-revalidate",
      "x-content-type-options": "nosniff",
    },
  });
}
