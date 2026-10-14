type Package = { id: string; name: string; version: string; links: string | null; manifest_path: string };
type Dependency = { pkg: string; dep_kinds: { kind: "dev" | "build" | null; target: string | null }[] };
type Node = { id: string; deps: Dependency[]; features: string[] };
type Metadata = { version: number; packages: Package[]; resolve: { nodes: Node[] } | null; workspace_members: string[] };

export function audit(metadata: Metadata, names: string[]) {
  if (metadata.version !== 1 || !metadata.resolve || !names.length) {
    throw new Error("c-surface-metadata: full cargo metadata --format-version 1 is required; --no-deps is insufficient");
  }
  const packages = new Map(metadata.packages.map(pkg => [pkg.id, pkg]));
  const nodes = new Map(metadata.resolve.nodes.map(node => [node.id, node]));
  const roots = names.map(name => {
    const matches = metadata.packages.filter(pkg => pkg.name === name);
    if (matches.length !== 1) throw new Error(`c-surface-root: ${name} matched ${matches.length} packages`);
    return matches[0];
  });
  function walk(start: Package, includeBuild: boolean) {
    const paths = new Map<string, string[]>([[start.id, [start.name]]]);
    const pending = [start.id];
    for (let index = 0; index < pending.length; index++) {
      const id = pending[index];
      const node = nodes.get(id);
      if (!node) throw new Error(`c-surface-node: missing resolved package ${id}`);
      for (const dependency of node.deps) {
        if (!dependency.dep_kinds.some(kind => kind.kind === null || (includeBuild && kind.kind === "build"))) continue;
        if (paths.has(dependency.pkg)) continue;
        const pkg = packages.get(dependency.pkg);
        if (!pkg) throw new Error(`c-surface-package: unknown dependency ${dependency.pkg}`);
        paths.set(pkg.id, [...paths.get(id)!, pkg.name]);
        pending.push(pkg.id);
      }
    }
    return paths;
  }
  const runtime = roots.map(root => ({ root: root.name, paths: walk(root, false) }));
  const builds = roots.map(root => ({ root: root.name, paths: walk(root, true) }));
  const forbidden = new Set(["openssl-sys", "openssl-src", "native-tls", "dav1d-sys", "libsqlite3-sys"]);
  const surface = metadata.packages.filter(pkg => pkg.name.endsWith("-sys") || pkg.links || forbidden.has(pkg.name))
    .sort((a, b) => a.name.localeCompare(b.name) || a.version.localeCompare(b.version))
    .map(pkg => ({
      name: pkg.name, version: pkg.version, links: pkg.links,
      features: nodes.get(pkg.id)?.features ?? [],
      runtime: runtime.flatMap(root => root.paths.has(pkg.id) ? [{ root: root.root, route: root.paths.get(pkg.id) }] : []),
      build: builds.flatMap(root => root.paths.has(pkg.id) ? [{ root: root.root, route: root.paths.get(pkg.id) }] : []),
      workspace_consumers: metadata.workspace_members.flatMap(id => {
        const member = packages.get(id);
        if (!member || !nodes.has(id)) return [];
        const route = walk(member, true).get(pkg.id);
        return route ? [{ member: member.name, route }] : [];
      }),
    }));
  const violations = surface.filter(pkg => forbidden.has(pkg.name) && (pkg.runtime.length || pkg.build.length));
  return { roots: names, surface, violations, passed: violations.length === 0 };
}

if (import.meta.main) {
  const [path, ...roots] = Bun.argv.slice(2);
  if (!path || !roots.length) throw new Error("usage: bun scripts/audit-c-surface.ts METADATA.json ROOT_PACKAGE...");
  const report = audit(await Bun.file(path).json(), roots);
  console.log(JSON.stringify(report, null, 2));
  if (!report.passed) throw new Error(`c-surface-forbidden: ${report.violations.map(pkg => pkg.name).join(", ")}`);
}
