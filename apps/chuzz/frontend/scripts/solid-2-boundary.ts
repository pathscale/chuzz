/**
 * Rewrite the generated layout bundle's boundary import to the Solid 2 arm.
 *
 * `solid-layouts-oxc` knows about both majors: `boundaryFor()` returns
 * `solid-layouts/application-boundary` for Solid 1 and
 * `solid-layouts/solid-2/application-boundary` for Solid 2, and a `solid: 2`
 * library build emits the latter. Its own `validateComponent()` then greps the
 * generated entry for the *hardcoded* Solid 1 string rather than asking
 * `boundaryFor()`, so the pairing it generates is the pairing it rejects:
 *
 *   @chuzz/ui: AddressBar entry has no application compiler boundary
 *
 * Neither arm works alone. Generating for Solid 1 emits `solid-js/web`
 * imports, a subpath Solid 2 does not export, and the build stops earlier at
 * "Package subpath './web' is not defined by exports". Generating for Solid 2
 * fails the validator above.
 *
 * A bundler alias cannot settle it either: the validator reads the file from
 * disk during plugin setup, before any resolution happens. So the fix has to
 * be on disk, which is what this does.
 *
 * The entry keeps the spelling the validator wants and the alias in
 * `rsbuild.config.ts` sends that specifier to the Solid 2 implementation. The
 * bundle is generated output and gitignored, so nothing here edits a tracked
 * file. Delete this script when the upstream validator consults `boundaryFor`.
 */
import { readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";

const entry = resolve(import.meta.dirname, "..", "local-ui", "bundle", "index.ts");
const SOLID_2 = "solid-layouts/solid-2/application-boundary";
const SOLID_1 = "solid-layouts/application-boundary";

const source = readFileSync(entry, "utf8");
if (source.includes(SOLID_2)) {
  writeFileSync(entry, source.replaceAll(SOLID_2, SOLID_1));
}
