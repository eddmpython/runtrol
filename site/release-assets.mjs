function normalized(value) {
  return typeof value === "string" ? value.toLowerCase() : "";
}

export function inferNativeTarget(environment = {}) {
  const platform = [environment.userAgentDataPlatform, environment.platform, environment.userAgent]
    .map(normalized)
    .join(" ");
  const os = platform.includes("win")
    ? "win32"
    : platform.includes("mac")
      ? "darwin"
      : platform.includes("linux")
        ? "linux"
        : null;
  if (!os) {
    return null;
  }

  const architecture = [environment.architecture, environment.bitness, environment.userAgent]
    .map(normalized)
    .join(" ");
  const isArm64 = /arm64|aarch64/u.test(architecture)
    || (/\barm\b/u.test(architecture) && /\b64\b/u.test(architecture));
  if (isArm64) {
    return `${os}-arm64`;
  }
  const isX64 = /x86_64|x86-64|amd64|\bx64\b|win64|wow64/u.test(architecture)
    || (/\bx86\b/u.test(architecture) && /\b64\b/u.test(architecture));
  return isX64 ? `${os}-x64` : null;
}

export function selectTargetVsix(assets, target) {
  if (!Array.isArray(assets) || typeof target !== "string") {
    return null;
  }
  const suffix = `-${target.toLowerCase()}.vsix`;
  return assets.find((asset) => (
    typeof asset?.name === "string"
    && typeof asset?.browser_download_url === "string"
    && asset.name.toLowerCase().endsWith(suffix)
  )) ?? null;
}
