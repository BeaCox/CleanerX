import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { buildUpdateManifest } from "./generate-update-manifest.mjs";

const temporaryDirectories = [];

afterEach(() => {
  for (const directory of temporaryDirectories.splice(0)) {
    fs.rmSync(directory, { recursive: true, force: true });
  }
});

function updaterAssets(version = "1.2.3") {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "cleanerx-updater-manifest-"));
  temporaryDirectories.push(directory);
  const names = [
    `CleanerX_${version}_macos_arm64_unsigned.app.tar.gz`,
    `CleanerX_${version}_macos_x86_64_unsigned.app.tar.gz`,
    `CleanerX_${version}_linux_x86_64_unsigned.AppImage`,
    `CleanerX_${version}_windows_x86_64_unsigned_setup.exe`,
  ];
  names.forEach((name, index) => {
    fs.writeFileSync(path.join(directory, name), `artifact-${index}`);
    fs.writeFileSync(path.join(directory, `${name}.sig`), `signature-${index}\n`);
  });
  return directory;
}

describe("update manifest generation", () => {
  it("maps every release artifact to the platform key expected by Tauri", () => {
    const manifest = buildUpdateManifest({
      assetsDirectory: updaterAssets(),
      version: "1.2.3",
      repository: "BeaCox/CleanerX",
      tag: "v1.2.3",
      publishedAt: "2026-08-28T00:00:00Z",
    });

    expect(Object.keys(manifest.platforms)).toEqual([
      "darwin-aarch64",
      "darwin-x86_64",
      "linux-x86_64",
      "windows-x86_64",
    ]);
    expect(manifest.platforms["windows-x86_64"]).toEqual({
      signature: "signature-3",
      url: "https://github.com/BeaCox/CleanerX/releases/download/v1.2.3/CleanerX_1.2.3_windows_x86_64_unsigned_setup.exe",
    });
  });

  it("fails closed when an artifact signature is missing", () => {
    const directory = updaterAssets();
    fs.rmSync(path.join(directory, "CleanerX_1.2.3_linux_x86_64_unsigned.AppImage.sig"));

    expect(() => buildUpdateManifest({
      assetsDirectory: directory,
      version: "1.2.3",
      repository: "BeaCox/CleanerX",
      tag: "v1.2.3",
      publishedAt: "2026-08-28T00:00:00Z",
    })).toThrow(/Missing updater artifact or signature for linux-x86_64/);
  });
});
