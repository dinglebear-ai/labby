#!/usr/bin/env node
"use strict";

const crypto = require("node:crypto");
const fs = require("node:fs");
const http = require("node:http");
const https = require("node:https");
const os = require("node:os");
const path = require("node:path");
const { spawnSync } = require("node:child_process");
const {
  binaryPath,
  downloadUrl,
  installRoot,
  releaseVersion,
  targetFor,
} = require("../lib/platform");

function log(message) {
  process.stderr.write(`labby: ${message}\n`);
}

function download(url, destination) {
  return new Promise((resolve, reject) => {
    const client = url.startsWith("http:") ? http : https;
    const request = client.get(url, (response) => {
      if ([301, 302, 303, 307, 308].includes(response.statusCode)) {
        response.resume();
        download(response.headers.location, destination).then(resolve, reject);
        return;
      }

      if (response.statusCode !== 200) {
        response.resume();
        reject(new Error(`download failed (${response.statusCode}) from ${url}`));
        return;
      }

      const file = fs.createWriteStream(destination, { mode: 0o600 });
      response.pipe(file);
      file.on("finish", () => file.close(resolve));
      file.on("error", reject);
    });

    request.on("error", reject);
  });
}

function sha256(file) {
  const hash = crypto.createHash("sha256");
  hash.update(fs.readFileSync(file));
  return hash.digest("hex");
}

function checksumFromText(text, asset) {
  const lines = text.trim().split(/\r?\n/).filter(Boolean);
  for (const line of lines) {
    const parts = line.trim().split(/\s+/);
    const hash = parts[0] && parts[0].toLowerCase();
    const name = parts.slice(1).join(" ").replace(/^\*/, "");
    if (/^[a-f0-9]{64}$/.test(hash) && (!asset || !name || path.basename(name) === asset)) {
      return hash;
    }
  }
  throw new Error("checksum file does not contain a SHA-256 entry for " + asset);
}

async function verifyChecksum(url, archive) {
  const asset = path.basename(archive);
  const sidecarFile = archive + ".sha256";
  let expected;

  try {
    await download(url + ".sha256", sidecarFile);
    expected = checksumFromText(fs.readFileSync(sidecarFile, "utf8"), asset);
  } catch (sidecarError) {
    const manifestUrl = url.replace(/\/[^/]+$/, "/SHA256SUMS");
    const manifestFile = archive + ".SHA256SUMS";
    await download(manifestUrl, manifestFile);
    expected = checksumFromText(fs.readFileSync(manifestFile, "utf8"), asset);
  }

  const actual = sha256(archive);
  if (actual !== expected) {
    throw new Error("checksum mismatch for " + asset + ": expected " + expected + ", got " + actual);
  }

  log("verified checksum for " + asset);
}

function run(command, args, failureMessage) {
  const result = spawnSync(command, args, {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
  });

  if (result.status !== 0) {
    throw new Error((result.stderr || result.stdout || failureMessage).trim());
  }
}

function extractTarGz(archive, destination) {
  run("tar", ["-xzf", archive, "-C", destination], "tar extraction failed");
}

function powershellLiteral(value) {
  return `'${String(value).replaceAll("'", "''")}'`;
}

function powershellExpandArchiveCommand(archive, destination) {
  return `Expand-Archive -LiteralPath ${powershellLiteral(archive)} -DestinationPath ${powershellLiteral(destination)} -Force`;
}

function extractZip(archive, destination) {
  if (process.platform === "win32") {
    run(
      "powershell.exe",
      [
        "-NoLogo",
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
        powershellExpandArchiveCommand(archive, destination),
      ],
      "zip extraction failed",
    );
    return;
  }

  run("unzip", ["-q", archive, "-d", destination], "zip extraction failed");
}

function extract(archive, destination, archiveType) {
  fs.rmSync(destination, { recursive: true, force: true });
  fs.mkdirSync(destination, { recursive: true });

  if (archiveType === "tar.gz") {
    extractTarGz(archive, destination);
    return;
  }

  if (archiveType === "zip") {
    extractZip(archive, destination);
    return;
  }

  throw new Error(`unsupported archive type: ${archiveType}`);
}

async function main() {
  if (process.env.LABBY_SKIP_DOWNLOAD === "1") {
    log("skipping binary download because LABBY_SKIP_DOWNLOAD=1");
    return;
  }

  const target = targetFor();
  const destination = binaryPath();

  recoverInstallation(installRoot());
  if (installationValid(installRoot(), target, releaseVersion())) {
    log(`${path.basename(destination)} already installed for ${releaseVersion()}`);
    return;
  }

  const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), "labby-mcp-install-"));
  const archive = path.join(tempDir, target.asset);

  try {
    const url = downloadUrl(target);
    log(`downloading ${url}`);
    await download(url, archive);
    await verifyChecksum(url, archive);
    installArchive(archive, installRoot(), target, releaseVersion());
    log(`installed ${destination}`);
  } finally {
    fs.rmSync(tempDir, { recursive: true, force: true });
  }
}

function requiresCompanions(version) {
  const match = /^v?(\d+)\.(\d+)\.(\d+)$/.exec(version);
  if (!match) throw new Error("invalid release version");
  return Number(match[1]) > 2 || (Number(match[1]) === 2 && Number(match[2]) >= 5);
}

function validateInstallation(root, target, version) {
  const binary = path.join(root, target.binary);
  if (!fs.lstatSync(binary).isFile()) throw new Error("release binary is not regular");
  const companions = path.join(root, "tailcat");
  if (!fs.existsSync(companions)) {
    if (requiresCompanions(version)) throw new Error("release companions missing");
    return;
  }
  if (!fs.lstatSync(companions).isDirectory()) throw new Error("invalid companion directory");
  const manifestPath = path.join(companions, "manifest.json");
  if (!fs.lstatSync(manifestPath).isFile() || fs.statSync(manifestPath).size > 2 * 1024 * 1024) throw new Error("invalid companion manifest");
  const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
  if (manifest.schemaVersion !== 1 || manifest.protocol !== 1 || manifest.version !== version.replace(/^v/, "") || manifest.target !== target.asset.replace(/^lab-/, "").replace(/\.tar\.gz$/, "") || !Array.isArray(manifest.components) || manifest.components.length > 8192) throw new Error("companion release mismatch");
  const expected = new Set();
  let total = 0;
  for (const entry of manifest.components) {
    if (typeof entry.path !== "string" || !entry.path || entry.path.includes("\\") || entry.path.split("/").some(part => !part || part === "." || part === "..") || path.isAbsolute(entry.path) || expected.has(entry.path) || !/^[a-f0-9]{64}$/.test(entry.sha256)) throw new Error("invalid companion entry");
    expected.add(entry.path);
    let file = companions;
    for (const part of entry.path.split("/")) {
      file = path.join(file, part);
      if (fs.lstatSync(file).isSymbolicLink()) throw new Error("symlink companion");
    }
    const stat = fs.statSync(file);
    total += stat.size;
    if (!stat.isFile() || stat.size > 128 * 1024 * 1024 || total > 512 * 1024 * 1024 || crypto.createHash("sha256").update(fs.readFileSync(file)).digest("hex") !== entry.sha256) throw new Error("companion checksum mismatch");
  }
  let count = 0;
  function inventory(directory, prefix = "", depth = 0) {
    if (depth > 64) throw new Error("companion depth exceeded");
    for (const entry of fs.readdirSync(directory, { withFileTypes: true })) {
      if (++count > 16384) throw new Error("companion inventory exceeded");
      const relative = prefix + entry.name;
      if (entry.isDirectory()) inventory(path.join(directory, entry.name), relative + "/", depth + 1);
      else if (!entry.isFile() || (relative !== "manifest.json" && !expected.has(relative))) throw new Error("unlisted companion");
    }
  }
  inventory(companions);
  for (const required of ["native/tailcat-bridge", "browser/hook.mjs", "browser/transport.mjs", "browser/http.mjs", "browser/wasm.mjs", "browser/tailcat.wasm", "browser/tailcat.wasm.gz", "browser/wasm_exec.js", "browser/THIRD_PARTY_NOTICES.md", "adapter/server.mjs", "adapter/cleanup.mjs", "adapter/package.json", "adapter/package-lock.json"]) {
    if (!expected.has(required)) throw new Error("incomplete companion inventory");
  }
}

function installationValid(root, target, version) {
  try { validateInstallation(root, target, version); return true; } catch { return false; }
}

function recoverInstallation(root) {
  const backup = root + ".previous";
  if (!fs.existsSync(root) && fs.existsSync(backup)) fs.renameSync(backup, root);
}

function installArchive(archive, root, target, version) {
  fs.mkdirSync(path.dirname(root), { recursive: true });
  const stage = fs.mkdtempSync(path.join(path.dirname(root), ".labby-install-"));
  const backup = root + ".previous";
  try {
    extract(archive, stage, target.archiveType);
    validateInstallation(stage, target, version);
    fs.chmodSync(path.join(stage, target.binary), 0o755);
    // Preserve the prior complete directory until the new pair has been published.
    if (fs.existsSync(backup)) fs.rmSync(backup, { recursive: true, force: true });
    if (fs.existsSync(root)) fs.renameSync(root, backup);
    try { fs.renameSync(stage, root); }
    catch (error) { recoverInstallation(root); throw error; }
    fs.rmSync(backup, { recursive: true, force: true });
  } finally { fs.rmSync(stage, { recursive: true, force: true }); }
}

if (require.main === module) {
  main().catch((error) => {
    log(error.message);
    process.exitCode = 1;
  });
}

module.exports = {
  extract,
  installArchive,
  installationValid,
  recoverInstallation,
  powershellExpandArchiveCommand,
  powershellLiteral,
};
