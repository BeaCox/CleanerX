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
    `CleanerX-${version}-macos-arm64.tar.gz`,
    `CleanerX-${version}-macos-x64.tar.gz`,
    `CleanerX-${version}-linux-arm64.AppImage`,
    `CleanerX-${version}-linux-x64.AppImage`,
    `CleanerX-${version}-windows-arm64.exe`,
    `CleanerX-${version}-windows-x64.exe`,
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
      "linux-aarch64",
      "linux-x86_64",
      "windows-aarch64",
      "windows-x86_64",
    ]);
    expect(manifest.platforms["linux-aarch64"]).toEqual({
      signature: "signature-2",
      url: "https://github.com/BeaCox/CleanerX/releases/download/v1.2.3/CleanerX-1.2.3-linux-arm64.AppImage",
    });
    expect(manifest.platforms["windows-x86_64"]).toEqual({
      signature: "signature-5",
      url: "https://github.com/BeaCox/CleanerX/releases/download/v1.2.3/CleanerX-1.2.3-windows-x64.exe",
    });
  });

  it("fails closed when an artifact signature is missing", () => {
    const directory = updaterAssets();
    fs.rmSync(path.join(directory, "CleanerX-1.2.3-linux-x64.AppImage.sig"));

    expect(() => buildUpdateManifest({
      assetsDirectory: directory,
      version: "1.2.3",
      repository: "BeaCox/CleanerX",
      tag: "v1.2.3",
      publishedAt: "2026-08-28T00:00:00Z",
    })).toThrow(/Missing updater artifact or signature for linux-x86_64/);
  });
});
