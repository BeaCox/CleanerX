import { createHash } from "node:crypto";
import { lstat, readFile, readlink } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const expectedWindowSizeMarker = "760, 440";

function digest(contents) {
  return createHash("sha256").update(contents).digest("hex");
}

async function pathKind(target) {
  try {
    const stat = await lstat(target);
    if (stat.isDirectory()) return "directory";
    if (stat.isSymbolicLink()) return "symbolic-link";
    if (stat.isFile()) return "file";
    return "other";
  } catch (error) {
    if (error?.code === "ENOENT") return "missing";
    throw error;
  }
}

export function validateDmgLayout(snapshot) {
  const problems = [];

  if (snapshot.appKind !== "directory") {
    problems.push("CleanerX.app must be a real directory");
  }
  if (snapshot.applicationsKind !== "symbolic-link") {
    problems.push("Applications must be a symbolic link");
  } else if (snapshot.applicationsTarget !== "/Applications") {
    problems.push("Applications must link exactly to /Applications");
  }
  if (snapshot.dsStoreKind !== "file" || snapshot.dsStoreBytes < 512) {
    problems.push(".DS_Store must be a non-empty Finder layout file");
  }
  if (!snapshot.dsStoreContainsBackground) {
    problems.push(".DS_Store must select dmg-background.png");
  }
  if (!snapshot.dsStoreContainsWindowSize) {
    problems.push(".DS_Store must retain the configured 760 x 440 window size");
  }
  if (snapshot.backgroundKind !== "file") {
    problems.push("the DMG background must be a real file");
  } else if (snapshot.backgroundDigest !== snapshot.expectedBackgroundDigest) {
    problems.push("the staged DMG background must match the tracked background");
  }

  if (problems.length > 0) {
    throw new Error(`Invalid macOS DMG layout:\n- ${problems.join("\n- ")}`);
  }

  return snapshot;
}

export async function inspectMountedDmg(mountRoot, expectedBackgroundPath) {
  const dsStorePath = path.join(mountRoot, ".DS_Store");
  const backgroundPath = path.join(mountRoot, ".background", "dmg-background.png");
  const applicationsPath = path.join(mountRoot, "Applications");
  const dsStoreKind = await pathKind(dsStorePath);
  const backgroundKind = await pathKind(backgroundPath);
  const dsStore = dsStoreKind === "file" ? await readFile(dsStorePath) : Buffer.alloc(0);
  const background = backgroundKind === "file" ? await readFile(backgroundPath) : Buffer.alloc(0);
  const expectedBackground = await readFile(expectedBackgroundPath);
  const dsStoreText = dsStore.toString("latin1");
  const applicationsKind = await pathKind(applicationsPath);

  return validateDmgLayout({
    appKind: await pathKind(path.join(mountRoot, "CleanerX.app")),
    applicationsKind,
    applicationsTarget:
      applicationsKind === "symbolic-link" ? await readlink(applicationsPath) : null,
    dsStoreKind,
    dsStoreBytes: dsStore.byteLength,
    dsStoreContainsBackground: dsStoreText.includes("dmg-background.png"),
    dsStoreContainsWindowSize: dsStoreText.includes(expectedWindowSizeMarker),
    backgroundKind,
    backgroundDigest: digest(background),
    expectedBackgroundDigest: digest(expectedBackground),
  });
}

const isMain = process.argv[1] && fileURLToPath(import.meta.url) === path.resolve(process.argv[1]);

if (isMain) {
  if (process.argv.length !== 4) {
    console.error("Usage: node scripts/verify-macos-dmg-layout.mjs <mounted-dmg-root> <expected-background>");
    process.exit(2);
  }

  try {
    const snapshot = await inspectMountedDmg(process.argv[2], process.argv[3]);
    console.log(
      `Verified Finder layout (${snapshot.dsStoreBytes} byte .DS_Store, background ${snapshot.backgroundDigest}).`,
    );
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exit(1);
  }
}
