import { describe, expect, it } from "vitest";
import { validateDmgLayout } from "./verify-macos-dmg-layout.mjs";

const validSnapshot = {
  appKind: "directory",
  applicationsKind: "symbolic-link",
  applicationsTarget: "/Applications",
  dsStoreKind: "file",
  dsStoreBytes: 10_240,
  dsStoreContainsBackground: true,
  dsStoreContainsWindowSize: true,
  backgroundKind: "file",
  backgroundDigest: "expected-digest",
  expectedBackgroundDigest: "expected-digest",
};

describe("macOS DMG layout validation", () => {
  it("accepts the intended Finder layout snapshot", () => {
    expect(validateDmgLayout(validSnapshot)).toEqual(validSnapshot);
  });

  it("rejects the unstyled CI layout", () => {
    expect(() => validateDmgLayout({
      ...validSnapshot,
      dsStoreKind: "missing",
      dsStoreBytes: 0,
      dsStoreContainsBackground: false,
      dsStoreContainsWindowSize: false,
    })).toThrow(/Finder layout file[\s\S]*select dmg-background[\s\S]*760 x 440/);
  });

  it("rejects a changed background or unsafe Applications entry", () => {
    expect(() => validateDmgLayout({
      ...validSnapshot,
      applicationsKind: "directory",
      applicationsTarget: null,
      backgroundDigest: "different-digest",
    })).toThrow(/symbolic link[\s\S]*match the tracked background/);
  });
});
