import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

const targets = [
  ["darwin-aarch64", (version) => `CleanerX-${version}-macos-arm64.tar.gz`],
  ["darwin-x86_64", (version) => `CleanerX-${version}-macos-x64.tar.gz`],
  ["linux-aarch64", (version) => `CleanerX-${version}-linux-arm64.AppImage`],
  ["linux-x86_64", (version) => `CleanerX-${version}-linux-x64.AppImage`],
  ["windows-aarch64", (version) => `CleanerX-${version}-windows-arm64.exe`],
  ["windows-x86_64", (version) => `CleanerX-${version}-windows-x64.exe`],
];

export function buildUpdateManifest({ assetsDirectory, version, repository, tag, publishedAt }) {
  if (!/^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/.test(version)) {
    throw new Error(`Invalid updater version: ${version}`);
  }
  if (!/^[^/\s]+\/[^/\s]+$/.test(repository)) {
    throw new Error(`Invalid GitHub repository: ${repository}`);
  }
  if (!tag) throw new Error("The release tag is required");

  const platforms = Object.fromEntries(targets.map(([target, assetName]) => {
    const filename = assetName(version);
    const assetPath = path.join(assetsDirectory, filename);
    const signaturePath = `${assetPath}.sig`;
    if (!isFile(assetPath) || !isFile(signaturePath)) {
      throw new Error(`Missing updater artifact or signature for ${target}: ${filename}`);
    }
    const signature = fs.readFileSync(signaturePath, "utf8").trim();
    if (!signature) throw new Error(`Empty updater signature for ${target}: ${filename}.sig`);
    const encodedTag = encodeURIComponent(tag);
    const encodedFilename = encodeURIComponent(filename);
    return [target, {
      signature,
      url: `https://github.com/${repository}/releases/download/${encodedTag}/${encodedFilename}`,
    }];
  }));

  return {
    version,
    notes: `CleanerX ${tag}. See the GitHub Release for reviewed release notes.`,
    pub_date: publishedAt,
    platforms,
  };
}

function isFile(filename) {
  return fs.existsSync(filename) && fs.statSync(filename).isFile();
}

export function writeUpdateManifest(options) {
  const manifest = buildUpdateManifest(options);
  const destination = path.join(options.assetsDirectory, "latest.json");
  fs.writeFileSync(destination, `${JSON.stringify(manifest, null, 2)}\n`, { encoding: "utf8", flag: "wx" });
  return destination;
}

const invokedFile = process.argv[1] ? pathToFileURL(path.resolve(process.argv[1])).href : "";
if (import.meta.url === invokedFile) {
  const [assetsDirectory, version, repository, tag, publishedAt = new Date().toISOString()] = process.argv.slice(2);
  if (!assetsDirectory || !version || !repository || !tag) {
    throw new Error("Usage: node scripts/generate-update-manifest.mjs <assets-dir> <version> <owner/repo> <tag> [published-at]");
  }
  writeUpdateManifest({ assetsDirectory, version, repository, tag, publishedAt });
}
