import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

const prereleaseOffsets = {
  alpha: 10_000,
  beta: 20_000,
  rc: 30_000,
};

export function toMsiVersion(version) {
  const match = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-(alpha|beta|rc)\.(0|[1-9]\d*))?(?:\+[0-9A-Za-z.-]+)?$/.exec(version);
  if (!match) {
    throw new Error(`Unsupported release version for MSI: ${version}`);
  }

  let major = Number(match[1]);
  let minor = Number(match[2]);
  let patch = Number(match[3]);
  const prereleaseKind = match[4];
  const prereleaseNumber = match[5] === undefined ? undefined : Number(match[5]);

  if (major > 255 || minor > 255 || patch > 65_535) {
    throw new Error(`Release version exceeds MSI numeric limits: ${version}`);
  }
  if (!prereleaseKind) return `${major}.${minor}.${patch}`;
  if (prereleaseNumber > 9_999) {
    throw new Error(`Prerelease sequence exceeds the MSI allocation: ${version}`);
  }

  if (patch > 0) {
    patch -= 1;
  } else if (minor > 0) {
    minor -= 1;
    patch = 65_535;
  } else if (major > 0) {
    major -= 1;
    minor = 255;
    patch = 65_535;
  } else {
    throw new Error("An MSI prerelease cannot sort below version 0.0.0");
  }

  const build = prereleaseOffsets[prereleaseKind] + prereleaseNumber;
  return `${major}.${minor}.${patch}.${build}`;
}

export function buildWindowsReleaseConfig(baseConfig, version) {
  return {
    ...baseConfig,
    bundle: {
      ...baseConfig.bundle,
      windows: {
        ...baseConfig.bundle?.windows,
        wix: {
          ...baseConfig.bundle?.windows?.wix,
          version: toMsiVersion(version),
        },
      },
    },
  };
}

export function writeWindowsReleaseConfig({ baseConfigPath, version, destination }) {
  const baseConfig = JSON.parse(fs.readFileSync(baseConfigPath, "utf8"));
  const config = buildWindowsReleaseConfig(baseConfig, version);
  fs.mkdirSync(path.dirname(destination), { recursive: true });
  fs.writeFileSync(destination, `${JSON.stringify(config, null, 2)}\n`, {
    encoding: "utf8",
    flag: "wx",
  });
  return destination;
}

const invokedFile = process.argv[1] ? pathToFileURL(path.resolve(process.argv[1])).href : "";
if (import.meta.url === invokedFile) {
  const [baseConfigPath, version, destination] = process.argv.slice(2);
  if (!baseConfigPath || !version || !destination) {
    throw new Error(
      "Usage: node scripts/generate-windows-release-config.mjs <base-config> <version> <destination>",
    );
  }
  writeWindowsReleaseConfig({ baseConfigPath, version, destination });
}
