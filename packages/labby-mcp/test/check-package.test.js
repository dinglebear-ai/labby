"use strict";

const test = require("node:test");
const assert = require("node:assert/strict");
const crypto = require("node:crypto");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { localChecksumMatches } = require("../scripts/check-package");

test("validates a local release asset against its checksum sidecar", (t) => {
  const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), "labby-release-assets-"));
  t.after(() => fs.rmSync(tempDir, { recursive: true, force: true }));

  const asset = "lab-x86_64-unknown-linux-gnu.tar.gz";
  const assetPath = path.join(tempDir, asset);
  const contents = Buffer.from("verified release artifact");
  const digest = crypto.createHash("sha256").update(contents).digest("hex");

  fs.writeFileSync(assetPath, contents);
  fs.writeFileSync(`${assetPath}.sha256`, `${digest}  ${asset}\n`);

  assert.equal(localChecksumMatches(assetPath, asset), true);

  fs.appendFileSync(assetPath, " corrupted");
  assert.equal(localChecksumMatches(assetPath, asset), false);
});

test("accepts a matching aggregate checksum manifest", (t) => {
  const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), "labby-release-assets-"));
  t.after(() => fs.rmSync(tempDir, { recursive: true, force: true }));

  const asset = "lab-x86_64-pc-windows-msvc.zip";
  const assetPath = path.join(tempDir, asset);
  const contents = Buffer.from("verified windows artifact");
  const digest = crypto.createHash("sha256").update(contents).digest("hex");

  fs.writeFileSync(assetPath, contents);
  fs.writeFileSync(path.join(tempDir, "SHA256SUMS"), `${digest} *${asset}\n`);

  assert.equal(localChecksumMatches(assetPath, asset), true);
});
