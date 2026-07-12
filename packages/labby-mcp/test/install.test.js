"use strict";

const test = require("node:test");
const assert = require("node:assert/strict");
const {
  powershellExpandArchiveCommand,
  powershellLiteral,
} = require("../scripts/install");

test("quotes PowerShell literals for archive extraction", () => {
  assert.equal(powershellLiteral("C:\\Temp\\labby's.zip"), "'C:\\Temp\\labby''s.zip'");
});

test("builds Windows zip extraction command without PowerShell args", () => {
  const command = powershellExpandArchiveCommand(
    "C:\\Temp\\labby's.zip",
    "C:\\Users\\Docker\\vendor",
  );

  assert.match(command, /^Expand-Archive -LiteralPath /);
  assert.match(command, /'C:\\Temp\\labby''s.zip'/);
  assert.match(command, /'C:\\Users\\Docker\\vendor'/);
  assert.doesNotMatch(command, /\$args/);
});
