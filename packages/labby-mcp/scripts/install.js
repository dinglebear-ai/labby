#!/usr/bin/env node
"use strict";

const fs = require("node:fs");
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
    const request = https.get(url, (response) => {
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

  if (fs.existsSync(destination)) {
    log(`${path.basename(destination)} already installed for ${releaseVersion()}`);
    return;
  }

  const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), "labby-mcp-install-"));
  const archive = path.join(tempDir, target.asset);

  try {
    const url = downloadUrl(target);
    log(`downloading ${url}`);
    await download(url, archive);
    extract(archive, installRoot(), target.archiveType);
    fs.chmodSync(destination, 0o755);
    log(`installed ${destination}`);
  } finally {
    fs.rmSync(tempDir, { recursive: true, force: true });
  }
}

if (require.main === module) {
  main().catch((error) => {
    log(error.message);
    process.exitCode = 1;
  });
}

module.exports = {
  powershellExpandArchiveCommand,
  powershellLiteral,
};
