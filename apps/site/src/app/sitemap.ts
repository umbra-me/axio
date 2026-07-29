import type { MetadataRoute } from "next";
import { SITE } from "./brand";

// One page. A sitemap for a single URL earns nothing on its own — it is here so
// that the file exists and grows with the site rather than being remembered
// later, and so robots.txt points at something real.
export default function sitemap(): MetadataRoute.Sitemap {
  return [{ url: SITE, changeFrequency: "weekly", priority: 1 }];
}
