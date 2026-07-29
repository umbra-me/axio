import type { MetadataRoute } from "next";
import { SITE } from "./brand";

// The two install routes are served as text/plain for a shell to consume, not
// for a reader, and a crawler that indexes them turns a search result into a
// page whose entire content is a script. They stay reachable; they are just not
// advertised.
export default function robots(): MetadataRoute.Robots {
  return {
    rules: { userAgent: "*", allow: "/", disallow: ["/install", "/install.ps1"] },
    sitemap: `${SITE}/sitemap.xml`,
    host: SITE,
  };
}
