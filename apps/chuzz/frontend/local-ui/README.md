# Chuzz Local complex components

This directory is Chuzz's private Layout component library. It owns application-specific presentation that is intentionally not part of the reusable `@pathscale/ui` package.

## Where is the component manifest?

There is no authored component manifest. `solid-layouts-library` discovers every `*.layout.tsx` under `local-ui/src`, resolves the recipe named by its `Layout<typeof recipe, Props>` annotation, and requires the matching `NameLayout` export:

```text
local-ui/src + local-ui/package.json + solid-layouts-library
  -> local-ui/bundle (@chuzz/ui)
```

Component names, paths, recipe exports, Layout exports, props, and public types are derived from their colocated source instead of being repeated in JSON. The package name, version, dependencies, and `private` flag come from `local-ui/package.json`.

## Does it ship?

The authored `local-ui/src` tree and this README do not ship in Chuzz's executable or runtime assets. They participate only in the package-generation step.

The generated `local-ui/bundle` package is a build input. Rsbuild consumes its generated TSX, recipe tables, CSS, and `layouts.manifest.json` while producing the normal Chuzz JavaScript and assets. Chuzz does not perform runtime template or manifest lookup.

## Why is the bundle not committed?

Generated implementation files under `local-ui/bundle` are ignored so a compiler run does not fill the PR with duplicated CSS and generated TSX. They remain locally inspectable after running:

```sh
bun run layouts:local
```

`local-ui/bundle/package.json` is the single exception. It is a private bootstrap manifest that lets Bun establish the `@chuzz/ui` workspace link before the first generation. The compiler overwrites it deterministically and preserves `"private": true`, so the package cannot be published accidentally.

## When does generation run?

Chuzz runs `layouts:local` before development, start, build, typecheck, and tests. The application compiler then resolves both generated packages by their exact package names:

```ts
layouts: ["@pathscale/ui", "@chuzz/ui"]
```

Application code imports `@chuzz/ui`. It must not import `local-ui/src`, use a source alias, or manually bypass the generated package.

## What fails?

Generation fails when a component has a missing or ambiguous recipe import, an invalid Layout export, an unexported props type, a duplicate name, or a slot mismatch. Application setup fails when `@chuzz/ui`, its generated manifest, or an imported component export cannot be resolved. There is no graceful fallback to uncompiled Layout syntax.

To add a component:

1. Add its recipe and `.layout.tsx` source under `local-ui/src/components`.
2. Name the pair `Name.recipe.ts` and `Name.layout.tsx`, import the recipe into the Layout, and export `NameLayout`.
3. Run `bun run layouts:local` and inspect `local-ui/bundle` if needed.
4. Import the generated public component from `@chuzz/ui` in application source.
