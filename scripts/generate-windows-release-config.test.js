import { describe, expect, it } from "vitest";
import {
  buildWindowsReleaseConfig,
  toMsiVersion,
} from "./generate-windows-release-config.mjs";

describe("Windows release configuration", () => {
  it("maps ordered prereleases below their eventual stable MSI version", () => {
    expect(toMsiVersion("0.1.0-alpha.1")).toBe("0.0.65535.10001");
    expect(toMsiVersion("1.2.3-beta.4")).toBe("1.2.2.20004");
    expect(toMsiVersion("1.0.0-rc.7")).toBe("0.255.65535.30007");
    expect(toMsiVersion("1.2.3")).toBe("1.2.3");
  });

  it("preserves updater settings while adding the WiX override", () => {
    expect(buildWindowsReleaseConfig({
      bundle: { createUpdaterArtifacts: true },
    }, "0.1.0-alpha.1")).toEqual({
      bundle: {
        createUpdaterArtifacts: true,
        windows: {
          wix: { version: "0.0.65535.10001" },
        },
      },
    });
  });

  it("fails closed for unsupported or non-representable prereleases", () => {
    expect(() => toMsiVersion("0.1.0-preview.1")).toThrow(/Unsupported release version/);
    expect(() => toMsiVersion("0.0.0-alpha.1")).toThrow(/cannot sort below/);
    expect(() => toMsiVersion("0.1.0-alpha.10000")).toThrow(/exceeds the MSI allocation/);
  });
});
