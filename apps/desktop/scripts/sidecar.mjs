// Builds ofa.exe and puts it where Tauri bundles it from. Tauri wants the
// target triple in the file name and installs it next to the app as ofa.exe.
import { execFileSync } from "node:child_process";
import { copyFileSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const desktop = join(dirname(fileURLToPath(import.meta.url)), "..");
const root = join(desktop, "..", "..");
const triple = execFileSync("rustc", ["--print", "host-tuple"], { encoding: "utf8" }).trim();

execFileSync("cargo", ["build", "--release", "-p", "ofa-cli"], { cwd: root, stdio: "inherit" });

const out = join(desktop, "src-tauri", "binaries");
mkdirSync(out, { recursive: true });
copyFileSync(join(root, "target", "release", "ofa.exe"), join(out, `ofa-${triple}.exe`));
console.log(`sidecar: ofa-${triple}.exe ready`);
