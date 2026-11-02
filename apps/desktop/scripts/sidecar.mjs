// Builds the OFA hook helper and places Tauri's target-specific sidecar name.
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
const extension = process.platform === "win32" ? ".exe" : "";
copyFileSync(join(root, "target", "release", `ofa${extension}`), join(out, `ofa-${triple}${extension}`));
console.log(`sidecar: ofa-${triple}${extension} ready`);
