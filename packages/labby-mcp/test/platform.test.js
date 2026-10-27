"use strict";

const test = require("node:test");
const assert = require("node:assert/strict");
const {
  downloadUrl,
  releaseVersion,
  targetFor,
} = require("../lib/platform");
const { version: packageVersion } = require("../package.json");

test("maps supported platforms to release assets", () => {
  assert.deepEqual(targetFor("linux", "x64"), {
    asset: "lab-x86_64-unknown-linux-gnu.tar.gz",
    binary: "labby",
    archiveType: "tar.gz",
  });
  assert.deepEqual(targetFor("linux", "arm64"), {
    asset: "lab-aarch64-unknown-linux-gnu.tar.gz",
    binary: "labby",
    archiveType: "tar.gz",
  });
  assert.deepEqual(targetFor("darwin", "arm64"), {
    asset: "lab-aarch64-apple-darwin.tar.gz",
    binary: "labby",
    archiveType: "tar.gz",
  });
});

test("rejects unsupported platforms", () => {
  assert.throws(() => targetFor("linux", "ppc64"), /Unsupported platform/);
  assert.throws(() => targetFor("win32", "x64"), /Supported targets: linux\/x64, linux\/arm64, darwin\/arm64/);
});

test("uses npm package version as the binary tag by default", () => {
  assert.equal(releaseVersion({}), `v${packageVersion}`);
});

test("allows release tag override", () => {
  const env = {
    LABBY_BINARY_VERSION: "v9.9.9",
    LABBY_RELEASE_BASE_URL: "https://example.test/releases",
  };
  assert.equal(
    downloadUrl(targetFor("linux", "x64"), env),
    "https://example.test/releases/v9.9.9/lab-x86_64-unknown-linux-gnu.tar.gz",
  );
});

test("companion root is anchored beside the npm installed binary", () => {
  const { binaryPath, companionRoot } = require("../lib/platform");
  const path = require("node:path");
  assert.equal(
    companionRoot(),
    path.join(path.dirname(binaryPath("linux", "x64")), "tailcat"),
  );
});

test("dev exact versions retain the matching immutable dev release tag", () => {
  assert.equal(releaseVersion({ LABBY_VERSION: "2.6.3-dev.10" }), "v2.6.3-dev.10");
  assert.equal(releaseVersion({ LABBY_VERSION: "v2.6.3-dev.2", LABBY_INSTALL_CHANNEL: "dev" }), "v2.6.3-dev.2");
});

test("rejects malformed versions and channel mismatches", () => {
  for (const version of ["latest", "2.6.3-dev.0", "2.6.3-dev.01", "02.6.3", "2.6.3-rc.1", "2.6.3-dev.1+build", "2.6.3\n", "2.6.3-dev.1\r\n", "2.6.3\nforged=value"]) {
    assert.throws(() => releaseVersion({ LABBY_VERSION: version }), /Invalid immutable/);
  }
  assert.throws(() => releaseVersion({ LABBY_VERSION: "2.6.3", LABBY_INSTALL_CHANNEL: "dev" }), /does not match/);
  assert.throws(() => releaseVersion({ LABBY_VERSION: "2.6.3-dev.1", LABBY_INSTALL_CHANNEL: "stable" }), /does not match/);
  const currentChannel = /-dev\./.test(packageVersion) ? "dev" : "stable";
  const otherChannel = currentChannel === "dev" ? "stable" : "dev";
  assert.equal(releaseVersion({ LABBY_INSTALL_CHANNEL: currentChannel }), `v${packageVersion}`);
  assert.throws(() => releaseVersion({ LABBY_INSTALL_CHANNEL: otherChannel }), /does not match/);
});

test("an npm dev package defaults to its exact matching release", () => {
  const entry = require.cache[require.resolve("../package.json")];
  const previous = entry.exports;
  try {
    entry.exports = { ...previous, version: "2.6.3-dev.10" };
    assert.equal(releaseVersion({}), "v2.6.3-dev.10");
    assert.throws(() => releaseVersion({ LABBY_INSTALL_CHANNEL: "stable" }), /does not match/);
  } finally { entry.exports = previous; }
});
