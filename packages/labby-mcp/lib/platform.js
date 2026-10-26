"use strict";

const path = require("node:path");

function packageVersion() {
  return require("../package.json").version;
}

function targetFor(platform = process.platform, arch = process.arch) {
  if (platform === "linux" && arch === "x64") {
    return {
      asset: "lab-x86_64-unknown-linux-gnu.tar.gz",
      binary: "labby",
      archiveType: "tar.gz",
    };
  }

  if (platform === "linux" && arch === "arm64") {
    return {
      asset: "lab-aarch64-unknown-linux-gnu.tar.gz",
      binary: "labby",
      archiveType: "tar.gz",
    };
  }

  if (platform === "darwin" && arch === "arm64") {
    return {
      asset: "lab-aarch64-apple-darwin.tar.gz",
      binary: "labby",
      archiveType: "tar.gz",
    };
  }

  throw new Error(
    `Unsupported platform ${platform}/${arch}. Supported targets: linux/x64, linux/arm64, darwin/arm64.`,
  );
}

function releaseVersion(env = process.env) {
  const raw = env.LABBY_VERSION || env.LABBY_BINARY_VERSION || packageVersion();
  const tag = raw.startsWith("v") ? raw : `v${raw}`;
  const match = /^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-dev\.[1-9][0-9]*)?$/.exec(tag);
  if (!match || match[0] !== tag) throw new Error(`Invalid immutable Labby release tag: ${tag}`);
  const channel = match[4] ? "dev" : "stable";
  // The package version is itself an explicit exact-version choice. Ambient
  // channel settings cannot turn a stable npm package into a dev downloader.
  if (env.LABBY_INSTALL_CHANNEL && env.LABBY_INSTALL_CHANNEL !== channel) {
    throw new Error(`Release tag ${tag} does not match LABBY_INSTALL_CHANNEL=${env.LABBY_INSTALL_CHANNEL}`);
  }
  return tag;
}

function releaseBaseUrl(env = process.env) {
  const repo = env.LABBY_REPO || "dinglebear-ai/labby";
  return env.LABBY_RELEASE_BASE_URL || `https://github.com/${repo}/releases/download`;
}

function downloadUrl(target, env = process.env) {
  return `${releaseBaseUrl(env)}/${releaseVersion(env)}/${target.asset}`;
}

function installRoot() {
  return path.resolve(__dirname, "..", "vendor");
}

function companionRoot() {
  return path.join(installRoot(), "tailcat");
}

function binaryPath(platform = process.platform, arch = process.arch) {
  const target = targetFor(platform, arch);
  return path.join(installRoot(), target.binary);
}

module.exports = {
  binaryPath,
  companionRoot,
  downloadUrl,
  installRoot,
  packageVersion,
  releaseBaseUrl,
  releaseVersion,
  targetFor,
};
