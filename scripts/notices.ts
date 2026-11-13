import { createHash } from "node:crypto";
import { resolve } from "node:path";
import { mkdir } from "node:fs/promises";

process.chdir(resolve(import.meta.dir, ".."));
const inventory = await Bun.file("licenses/inventory.json").json();
const inputs = [...new Set<string>([
  ...inventory.inputs,
  ...inventory.components.flatMap((component: { files: string[] }) => component.files),
])].sort();

const metadata = Bun.spawnSync([
  "cargo", "metadata", "--no-deps", "--format-version", "1", "--locked",
]);
if (metadata.exitCode !== 0) throw new Error(metadata.stderr.toString());
const workspace = JSON.parse(metadata.stdout.toString());
const workspaceNames = new Set(workspace.packages
  .filter((pkg: { id: string }) => workspace.workspace_members.includes(pkg.id))
  .map((pkg: { name: string }) => pkg.name));

function lockInputs(content: string): string {
  const lock = Bun.TOML.parse(content) as {
    package: { name: string; version: string; source?: string; checksum?: string }[];
  };
  return lock.package
    .filter(pkg => pkg.source !== undefined || !workspaceNames.has(pkg.name))
    .map(pkg => [pkg.name, pkg.version, pkg.source ?? "", pkg.checksum ?? ""].join("\t"))
    .sort().join("\n");
}

function manifestInputs(content: string): string {
  let inPackage = false;
  return content.split("\n").map(line => {
    if (line.startsWith("[")) inPackage = line === "[package]";
    return inPackage && line.startsWith("version = ") ? 'version = "<workspace version>"' : line;
  }).join("\n");
}

const tool = process.env.CARGO_ABOUT ?? "cargo-about";
const version = Bun.spawnSync([tool, "--version"]);
if (version.exitCode !== 0 || version.stdout.toString().trim() !== "cargo-about 0.9.3") {
  throw new Error("Install cargo-about 0.9.3 (see licenses/README.md)");
}
// cargo-about rejects redirected stdout under PowerShell. A file also keeps
// license text encoding independent of the invoking shell on every platform.
await mkdir("target", { recursive: true });
const generatedPath = resolve("target/notices-generated.txt");
const generated = Bun.spawnSync([
  tool, "generate", "--locked", "--fail", "scripts/notices.hbs", "--output-file", generatedPath,
], { stdout: "inherit", stderr: "inherit" });
if (generated.exitCode !== 0) throw new Error("cargo-about failed");

let text = "Herdr third-party notices\n=========================\n"
  + "Regenerate with just notices. See licenses/README.md.\n\n";
for (const path of inputs) {
  // Normalize checkout line endings so the same check works on Windows.
  const content = (await Bun.file(path).text()).replaceAll("\r\n", "\n");
  const input = path === "Cargo.lock" ? lockInputs(content)
    : path === "Cargo.toml" ? manifestInputs(content) : content;
  const hash = createHash("sha256").update(input).digest("hex");
  text += `Input-SHA256: ${hash} ${path}\n`;
}
text += "\n" + (await Bun.file(generatedPath).text()).replaceAll("\r\n", "\n");
for (const component of inventory.components) {
  text += `\n----------------------------------------------------------------------\n${component.name}\n${component.source}\n\n`;
  for (const path of component.files) {
    text += `--- ${path} ---\n${(await Bun.file(path).text()).replaceAll("\r\n", "\n").trim()}\n\n`;
  }
}
await Bun.write("THIRD-PARTY-NOTICES", text);
console.log("Wrote THIRD-PARTY-NOTICES");
